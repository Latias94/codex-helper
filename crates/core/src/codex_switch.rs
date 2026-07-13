use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use toml::Value as TomlValue;
use toml_edit::{DocumentMut, Item, Table, Value as EditableValue, value as editable_value};
use uuid::Uuid;

const STATE_VERSION: u32 = 1;
const PROVIDER_ID: &str = "codex_proxy";
const PROVIDER_NAME: &str = "codex-helper";
const STATE_FILE_NAME: &str = "codex-switch.json";
const LOCK_FILE_NAME: &str = "codex-switch.lock";
const LEGACY_STATE_FILE_NAME: &str = "codex-helper-switch-state.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCodexBaseUrl(String);

impl ValidatedCodexBaseUrl {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, CodexSwitchError> {
        let value = value.as_ref().trim();
        let parsed = Url::parse(value).map_err(|error| CodexSwitchError::InvalidBaseUrl {
            reason: error.to_string(),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(CodexSwitchError::InvalidBaseUrl {
                reason: "scheme must be http or https".to_string(),
            });
        }
        if parsed.host_str().is_none() {
            return Err(CodexSwitchError::InvalidBaseUrl {
                reason: "host is required".to_string(),
            });
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(CodexSwitchError::InvalidBaseUrl {
                reason: "userinfo credentials are not allowed".to_string(),
            });
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(CodexSwitchError::InvalidBaseUrl {
                reason: "query strings and fragments are not allowed".to_string(),
            });
        }

        Ok(Self(parsed.as_str().trim_end_matches('/').to_string()))
    }

    pub fn local(port: u16) -> Self {
        Self(crate::proxy::local_proxy_base_url(port))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexSwitchIntent {
    On {
        validated_base_url: ValidatedCodexBaseUrl,
    },
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexSwitchPhase {
    Off,
    Prepared,
    Applied,
    RecoveryRequired,
}

impl CodexSwitchPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Prepared => "prepared",
            Self::Applied => "applied",
            Self::RecoveryRequired => "recovery_required",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexSwitchChange {
    Applied,
    Removed,
    Unchanged,
    Recovered,
}

impl CodexSwitchChange {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Removed => "removed",
            Self::Unchanged => "unchanged",
            Self::Recovered => "recovered",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSwitchStatus {
    pub phase: CodexSwitchPhase,
    pub enabled: bool,
    pub managed: bool,
    pub base_url: Option<String>,
    pub recovery_reason: Option<String>,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSwitchOutcome {
    pub change: CodexSwitchChange,
    pub status: CodexSwitchStatus,
}

#[derive(Debug, Error)]
pub enum CodexSwitchError {
    #[error("invalid Codex helper base URL: {reason}")]
    InvalidBaseUrl { reason: String },
    #[error("failed to {action} {path:?}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse Codex config {path:?}: {reason}")]
    InvalidConfig { path: PathBuf, reason: String },
    #[error("failed to parse Codex switch state {path:?}: {reason}")]
    InvalidState { path: PathBuf, reason: String },
    #[error("Codex switch operation is already running; lock is held at {path:?}")]
    LockBusy { path: PathBuf },
    #[error(
        "model_providers.codex_proxy already exists without helper ownership state; refusing to overwrite it"
    )]
    ForeignProviderStanza,
    #[error(
        "Codex config already selects codex_proxy without helper ownership state; manual reconciliation is required"
    )]
    OrphanedActiveProvider,
    #[error(
        "legacy Codex switch state exists at {path:?}. Use codex-helper v0.20.3 (or the older binary that created it) to run `switch off` before upgrading. Do not run old and new switch commands concurrently. Do not delete, edit, or share this file because it may contain authentication recovery data"
    )]
    LegacySwitchState { path: PathBuf },
    #[error("unsupported Codex config file topology at {path:?}: {reason}")]
    UnsupportedConfigTopology { path: PathBuf, reason: String },
    #[error(
        "Codex helper is already applied to {current}; run explicit switch off before switching to {requested}"
    )]
    AlreadyAppliedToDifferentTarget { current: String, requested: String },
    #[error("Codex switch recovery is required: {reason}; config was not modified")]
    RecoveryRequired { reason: String },
    #[error("Codex switch state changed repeatedly while it was being inspected")]
    UnstableInspection,
    #[error("restoring the helper stanza would not reproduce the original config fingerprint")]
    RestoreFingerprintMismatch,
    #[cfg(test)]
    #[error("injected Codex switch failure at {0}")]
    InjectedFailure(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum JournalPhase {
    Prepared,
    Applied,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum JournalOperation {
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
struct SwitchJournal {
    version: u32,
    operation_id: String,
    phase: JournalPhase,
    operation: JournalOperation,
    config_path_fingerprint: String,
    original_config_present: bool,
    original_fingerprint: String,
    applied_fingerprint: String,
    original_model_provider: Option<String>,
    original_model_provider_repr: Option<String>,
    original_helper_stanza: Option<TomlValue>,
    original_model_providers_present: bool,
    target_base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct ConfigSnapshot {
    present: bool,
    text: String,
    fingerprint: String,
}

struct JournalSnapshot {
    raw: String,
    journal: SwitchJournal,
}

impl ConfigSnapshot {
    fn from_text(present: bool, text: String) -> Self {
        let fingerprint = fingerprint(text.as_bytes());
        Self {
            present,
            text,
            fingerprint,
        }
    }

    fn matches_original(&self, journal: &SwitchJournal) -> bool {
        self.present == journal.original_config_present
            && self.fingerprint == journal.original_fingerprint
    }

    fn matches_applied(&self, journal: &SwitchJournal) -> bool {
        self.present && self.fingerprint == journal.applied_fingerprint
    }
}

#[derive(Debug)]
enum ConfigEdit {
    Write(String),
    Remove,
}

#[derive(Debug, Clone, Copy)]
enum ExpectedConfigState {
    Original,
    Applied,
}

#[derive(Clone, Copy)]
struct ConfigCommitExpectation<'a> {
    journal: &'a SwitchJournal,
    state: ExpectedConfigState,
}

impl ConfigEdit {
    fn matches_original(&self, journal: &SwitchJournal) -> bool {
        match self {
            Self::Write(text) => {
                journal.original_config_present
                    && fingerprint(text.as_bytes()) == journal.original_fingerprint
            }
            Self::Remove => {
                !journal.original_config_present && fingerprint(&[]) == journal.original_fingerprint
            }
        }
    }
}

struct OperationLock {
    _file: File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyFailpoint {
    None,
    AfterPrepared,
    AfterConfigWrite,
}

impl ApplyFailpoint {
    #[cfg(test)]
    fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::AfterPrepared => "after_prepared",
            Self::AfterConfigWrite => "after_config_write",
        }
    }
}

struct OnPatch {
    text: String,
    original_model_provider_repr: Option<String>,
}

struct PlannedOnWrite {
    original_text: String,
    applied_text: String,
}

pub fn apply(intent: CodexSwitchIntent) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    apply_with_failpoint(intent, ApplyFailpoint::None)
}

pub fn inspect() -> Result<CodexSwitchStatus, CodexSwitchError> {
    let paths = SwitchPaths::resolve()?;
    if legacy_state_present(paths.legacy_state.as_path())? {
        return legacy_switch_status(&paths);
    }
    for _ in 0..3 {
        let before = read_journal_snapshot(paths.state.as_path())?;
        let current = read_config_snapshot(paths.config.as_path())?;
        let after = read_optional_text(paths.state.as_path())?;
        match (before, after) {
            (None, None) => return status_after_legacy_recheck(&paths, &current, None),
            (Some(before), Some(after_raw)) if before.raw == after_raw => {
                return status_after_legacy_recheck(&paths, &current, Some(&before.journal));
            }
            (Some(before), Some(after_raw)) => {
                let after = parse_journal(paths.state.as_path(), after_raw)?;
                if before.journal == after.journal {
                    return status_after_legacy_recheck(&paths, &current, Some(&after.journal));
                }
            }
            _ => {}
        }
    }
    Err(CodexSwitchError::UnstableInspection)
}

fn apply_with_failpoint(
    intent: CodexSwitchIntent,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    let paths = SwitchPaths::resolve()?;
    reject_legacy_switch_state(&paths)?;
    let _lock = OperationLock::acquire(paths.lock.as_path())?;
    reject_legacy_switch_state(&paths)?;
    let journal = read_journal(paths.state.as_path())?;
    if let Some(journal) = journal.as_ref() {
        ensure_journal_config_matches(&paths, journal)?;
    }
    let current = read_config_snapshot(paths.config.as_path())?;

    match intent {
        CodexSwitchIntent::On { validated_base_url } => {
            apply_on(&paths, current, journal, validated_base_url, failpoint)
        }
        CodexSwitchIntent::Off => apply_off(&paths, current, journal, failpoint),
    }
}

fn legacy_state_present(path: &Path) -> Result<bool, CodexSwitchError> {
    switch_path_entry_present(path, "inspect legacy switch state at")
}

fn switch_path_entry_present(path: &Path, action: &'static str) -> Result<bool, CodexSwitchError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(action, path, error)),
    }
}

fn legacy_switch_error(paths: &SwitchPaths) -> CodexSwitchError {
    CodexSwitchError::LegacySwitchState {
        path: paths.legacy_state.clone(),
    }
}

fn reject_legacy_switch_state(paths: &SwitchPaths) -> Result<(), CodexSwitchError> {
    if legacy_state_present(paths.legacy_state.as_path())? {
        return Err(legacy_switch_error(paths));
    }
    Ok(())
}

fn legacy_switch_status(paths: &SwitchPaths) -> Result<CodexSwitchStatus, CodexSwitchError> {
    let current_state_present =
        switch_path_entry_present(paths.state.as_path(), "inspect current switch state at")?;
    let config = read_config_snapshot(paths.config.as_path())
        .and_then(|current| inspect_config(paths.config.as_path(), current.text.as_str()))
        .ok();
    let enabled = config.as_ref().is_some_and(|config| {
        config.model_provider.as_deref() == Some(PROVIDER_ID) && config.helper_stanza.is_some()
    });
    let base_url = config.and_then(|config| config.helper_base_url);
    let mut recovery_reason = legacy_switch_error(paths).to_string();
    if current_state_present {
        recovery_reason.push_str(
            format!(
                ". A current switch journal also exists at {:?}; neither journal was modified",
                paths.state
            )
            .as_str(),
        );
    }

    Ok(CodexSwitchStatus {
        phase: CodexSwitchPhase::RecoveryRequired,
        enabled,
        managed: current_state_present,
        base_url,
        recovery_reason: Some(recovery_reason),
        config_path: paths.config.clone(),
        state_path: paths.legacy_state.clone(),
    })
}

fn status_after_legacy_recheck(
    paths: &SwitchPaths,
    current: &ConfigSnapshot,
    journal: Option<&SwitchJournal>,
) -> Result<CodexSwitchStatus, CodexSwitchError> {
    if legacy_state_present(paths.legacy_state.as_path())? {
        return legacy_switch_status(paths);
    }
    status_from_snapshot(paths, current, journal)
}

fn write_current_journal(
    paths: &SwitchPaths,
    journal: &SwitchJournal,
) -> Result<(), CodexSwitchError> {
    reject_legacy_switch_state(paths)?;
    write_journal(paths.state.as_path(), journal)
}

fn remove_current_journal(paths: &SwitchPaths) -> Result<(), CodexSwitchError> {
    reject_legacy_switch_state(paths)?;
    remove_journal(paths.state.as_path())
}

fn write_current_config_edit(
    paths: &SwitchPaths,
    edit: ConfigEdit,
    journal: &SwitchJournal,
    expected: ExpectedConfigState,
) -> Result<(), CodexSwitchError> {
    reject_legacy_switch_state(paths)?;
    write_config_edit(paths.config.as_path(), edit, journal, expected)
}

fn apply_on(
    paths: &SwitchPaths,
    current: ConfigSnapshot,
    journal: Option<SwitchJournal>,
    target: ValidatedCodexBaseUrl,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    match journal {
        None => begin_on(paths, current, target, failpoint),
        Some(mut journal) => match journal.phase {
            JournalPhase::RecoveryRequired => Err(CodexSwitchError::RecoveryRequired {
                reason: journal
                    .recovery_reason
                    .unwrap_or_else(|| "stored switch state requires reconciliation".to_string()),
            }),
            JournalPhase::Applied => {
                if !current.matches_applied(&journal) {
                    return mark_recovery(
                        paths,
                        journal,
                        "Codex config changed after helper applied its provider stanza",
                    );
                }
                ensure_target_matches(&journal, &target)?;
                outcome(paths, CodexSwitchChange::Unchanged)
            }
            JournalPhase::Prepared => match journal.operation {
                JournalOperation::On if current.matches_applied(&journal) => {
                    journal.phase = JournalPhase::Applied;
                    write_current_journal(paths, &journal)?;
                    ensure_target_matches(&journal, &target)?;
                    outcome(paths, CodexSwitchChange::Recovered)
                }
                JournalOperation::On if current.matches_original(&journal) => {
                    ensure_target_matches(&journal, &target)?;
                    resume_on(paths, journal, failpoint, None)
                }
                JournalOperation::Off if current.matches_original(&journal) => {
                    remove_current_journal(paths)?;
                    begin_on(paths, current, target, failpoint)
                }
                JournalOperation::Off if current.matches_applied(&journal) => {
                    journal.phase = JournalPhase::Applied;
                    journal.operation = JournalOperation::On;
                    write_current_journal(paths, &journal)?;
                    ensure_target_matches(&journal, &target)?;
                    outcome(paths, CodexSwitchChange::Recovered)
                }
                _ => mark_recovery(
                    paths,
                    journal,
                    "Codex config matches neither the prepared original nor applied fingerprint",
                ),
            },
        },
    }
}

fn ensure_target_matches(
    journal: &SwitchJournal,
    target: &ValidatedCodexBaseUrl,
) -> Result<(), CodexSwitchError> {
    if journal.target_base_url == target.as_str() {
        return Ok(());
    }
    Err(CodexSwitchError::AlreadyAppliedToDifferentTarget {
        current: journal.target_base_url.clone(),
        requested: target.0.clone(),
    })
}

fn ensure_journal_config_matches(
    paths: &SwitchPaths,
    journal: &SwitchJournal,
) -> Result<(), CodexSwitchError> {
    if journal.config_path_fingerprint == paths.config_fingerprint {
        return Ok(());
    }
    Err(CodexSwitchError::RecoveryRequired {
        reason: "switch state belongs to a different Codex config path".to_string(),
    })
}

fn begin_on(
    paths: &SwitchPaths,
    current: ConfigSnapshot,
    target: ValidatedCodexBaseUrl,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    validate_config_topology(paths.config.as_path(), current.present)?;
    let original = inspect_config(paths.config.as_path(), &current.text)?;
    reject_unowned_helper_config(&original)?;

    let patch = patch_on(paths.config.as_path(), &current.text, target.as_str())?;
    let applied_fingerprint = fingerprint(patch.text.as_bytes());
    let planned_write = PlannedOnWrite {
        original_text: current.text,
        applied_text: patch.text,
    };
    let journal = SwitchJournal {
        version: STATE_VERSION,
        operation_id: Uuid::new_v4().to_string(),
        phase: JournalPhase::Prepared,
        operation: JournalOperation::On,
        config_path_fingerprint: paths.config_fingerprint.clone(),
        original_config_present: current.present,
        original_fingerprint: current.fingerprint,
        applied_fingerprint,
        original_model_provider: original.model_provider,
        original_model_provider_repr: patch.original_model_provider_repr,
        original_helper_stanza: original.helper_stanza,
        original_model_providers_present: original.model_providers_present,
        target_base_url: target.0,
        recovery_reason: None,
    };
    write_current_journal(paths, &journal)?;
    resume_on(paths, journal, failpoint, Some(planned_write))
}

fn resume_on(
    paths: &SwitchPaths,
    mut journal: SwitchJournal,
    failpoint: ApplyFailpoint,
    planned_write: Option<PlannedOnWrite>,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    fail_if_requested(failpoint, ApplyFailpoint::AfterPrepared)?;

    let current = read_config_snapshot(paths.config.as_path())?;
    if !current.matches_original(&journal) {
        return mark_recovery(
            paths,
            journal,
            "Codex config changed between switch preparation and write",
        );
    }
    if let Err(error) = validate_config_topology(paths.config.as_path(), current.present) {
        return mark_recovery(paths, journal, error.to_string());
    }
    let applied_text = match planned_write {
        Some(planned) if planned.original_text == current.text => planned.applied_text,
        Some(_) | None => {
            patch_on(
                paths.config.as_path(),
                current.text.as_str(),
                journal.target_base_url.as_str(),
            )?
            .text
        }
    };
    if fingerprint(applied_text.as_bytes()) != journal.applied_fingerprint {
        return mark_recovery(
            paths,
            journal,
            "prepared Codex patch no longer produces the planned fingerprint",
        );
    }

    match write_current_config_edit(
        paths,
        ConfigEdit::Write(applied_text),
        &journal,
        ExpectedConfigState::Original,
    ) {
        Ok(()) => {}
        Err(CodexSwitchError::RecoveryRequired { reason }) => {
            return mark_recovery(paths, journal, reason);
        }
        Err(error) => return Err(error),
    }
    fail_if_requested(failpoint, ApplyFailpoint::AfterConfigWrite)?;
    let written = read_config_snapshot(paths.config.as_path())?;
    if !written.matches_applied(&journal) {
        return mark_recovery(
            paths,
            journal,
            "Codex config changed before the applied switch state was committed",
        );
    }

    journal.phase = JournalPhase::Applied;
    write_current_journal(paths, &journal)?;
    outcome(paths, CodexSwitchChange::Applied)
}

fn apply_off(
    paths: &SwitchPaths,
    current: ConfigSnapshot,
    journal: Option<SwitchJournal>,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    let Some(mut journal) = journal else {
        let config = inspect_config(paths.config.as_path(), &current.text)?;
        reject_unowned_helper_config(&config)?;
        return outcome(paths, CodexSwitchChange::Unchanged);
    };

    match journal.phase {
        JournalPhase::RecoveryRequired => Err(CodexSwitchError::RecoveryRequired {
            reason: journal
                .recovery_reason
                .unwrap_or_else(|| "stored switch state requires reconciliation".to_string()),
        }),
        JournalPhase::Applied => {
            if !current.matches_applied(&journal) {
                return mark_recovery(
                    paths,
                    journal,
                    "Codex config changed after helper applied its provider stanza",
                );
            }
            begin_off(paths, journal, failpoint)
        }
        JournalPhase::Prepared => {
            if current.matches_original(&journal) {
                remove_current_journal(paths)?;
                return outcome(paths, CodexSwitchChange::Recovered);
            }
            match journal.operation {
                JournalOperation::On if current.matches_applied(&journal) => {
                    journal.phase = JournalPhase::Applied;
                    write_current_journal(paths, &journal)?;
                    begin_off(paths, journal, failpoint)
                }
                JournalOperation::Off if current.matches_applied(&journal) => {
                    resume_off(paths, journal, failpoint)
                }
                _ => mark_recovery(
                    paths,
                    journal,
                    "Codex config matches neither the prepared original nor applied fingerprint",
                ),
            }
        }
    }
}

fn begin_off(
    paths: &SwitchPaths,
    mut journal: SwitchJournal,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    journal.phase = JournalPhase::Prepared;
    journal.operation = JournalOperation::Off;
    journal.operation_id = Uuid::new_v4().to_string();
    journal.recovery_reason = None;
    write_current_journal(paths, &journal)?;
    resume_off(paths, journal, failpoint)
}

fn resume_off(
    paths: &SwitchPaths,
    mut journal: SwitchJournal,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    fail_if_requested(failpoint, ApplyFailpoint::AfterPrepared)?;

    let current = read_config_snapshot(paths.config.as_path())?;
    if !current.matches_applied(&journal) {
        return mark_recovery(
            paths,
            journal,
            "Codex config changed between switch-off preparation and write",
        );
    }
    if let Err(error) = validate_config_topology(paths.config.as_path(), current.present) {
        return mark_recovery(paths, journal, error.to_string());
    }
    let edit = patch_off(paths.config.as_path(), current.text.as_str(), &journal)?;
    if !edit.matches_original(&journal) {
        journal.phase = JournalPhase::RecoveryRequired;
        journal.recovery_reason = Some(
            "restoring the helper stanza would not reproduce the original fingerprint".to_string(),
        );
        write_current_journal(paths, &journal)?;
        return Err(CodexSwitchError::RestoreFingerprintMismatch);
    }

    match write_current_config_edit(paths, edit, &journal, ExpectedConfigState::Applied) {
        Ok(()) => {}
        Err(CodexSwitchError::RecoveryRequired { reason }) => {
            return mark_recovery(paths, journal, reason);
        }
        Err(error) => return Err(error),
    }
    fail_if_requested(failpoint, ApplyFailpoint::AfterConfigWrite)?;
    let written = read_config_snapshot(paths.config.as_path())?;
    if !written.matches_original(&journal) {
        return mark_recovery(
            paths,
            journal,
            "Codex config changed before switch-off completion was committed",
        );
    }
    remove_current_journal(paths)?;
    outcome(paths, CodexSwitchChange::Removed)
}

fn mark_recovery<T>(
    paths: &SwitchPaths,
    mut journal: SwitchJournal,
    reason: impl Into<String>,
) -> Result<T, CodexSwitchError> {
    let reason = reason.into();
    journal.phase = JournalPhase::RecoveryRequired;
    journal.recovery_reason = Some(reason.clone());
    write_current_journal(paths, &journal)?;
    Err(CodexSwitchError::RecoveryRequired { reason })
}

fn fail_if_requested(
    actual: ApplyFailpoint,
    expected: ApplyFailpoint,
) -> Result<(), CodexSwitchError> {
    #[cfg(test)]
    if actual == expected {
        return Err(CodexSwitchError::InjectedFailure(expected.name()));
    }
    #[cfg(not(test))]
    let _ = (actual, expected);
    Ok(())
}

fn outcome(
    paths: &SwitchPaths,
    change: CodexSwitchChange,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    reject_legacy_switch_state(paths)?;
    let current = read_config_snapshot(paths.config.as_path())?;
    let journal = read_journal(paths.state.as_path())?;
    Ok(CodexSwitchOutcome {
        change,
        status: status_from_snapshot(paths, &current, journal.as_ref())?,
    })
}

fn status_from_snapshot(
    paths: &SwitchPaths,
    current: &ConfigSnapshot,
    journal: Option<&SwitchJournal>,
) -> Result<CodexSwitchStatus, CodexSwitchError> {
    if let Some(journal) = journal
        && journal.config_path_fingerprint != paths.config_fingerprint
    {
        return Ok(CodexSwitchStatus {
            phase: CodexSwitchPhase::RecoveryRequired,
            enabled: false,
            managed: true,
            base_url: Some(journal.target_base_url.clone()),
            recovery_reason: Some(
                "switch state belongs to a different Codex config path".to_string(),
            ),
            config_path: paths.config.clone(),
            state_path: paths.state.clone(),
        });
    }

    let config = inspect_config(paths.config.as_path(), current.text.as_str())?;
    let enabled =
        config.model_provider.as_deref() == Some(PROVIDER_ID) && config.helper_stanza.is_some();
    let config_base_url = config.helper_base_url;

    let Some(journal) = journal else {
        let orphaned =
            config.model_provider.as_deref() == Some(PROVIDER_ID) || config.helper_stanza.is_some();
        return Ok(CodexSwitchStatus {
            phase: if orphaned {
                CodexSwitchPhase::RecoveryRequired
            } else {
                CodexSwitchPhase::Off
            },
            enabled,
            managed: false,
            base_url: config_base_url,
            recovery_reason: orphaned.then(|| {
                "helper provider config exists without helper-owned switch state".to_string()
            }),
            config_path: paths.config.clone(),
            state_path: paths.state.clone(),
        });
    };

    if let Err(error) = validate_config_topology(paths.config.as_path(), current.present) {
        return Ok(CodexSwitchStatus {
            phase: CodexSwitchPhase::RecoveryRequired,
            enabled,
            managed: true,
            base_url: config_base_url.or_else(|| Some(journal.target_base_url.clone())),
            recovery_reason: Some(error.to_string()),
            config_path: paths.config.clone(),
            state_path: paths.state.clone(),
        });
    }

    let (phase, recovery_reason) = match journal.phase {
        JournalPhase::RecoveryRequired => (
            CodexSwitchPhase::RecoveryRequired,
            journal.recovery_reason.clone(),
        ),
        JournalPhase::Applied if current.matches_applied(journal) => {
            (CodexSwitchPhase::Applied, None)
        }
        JournalPhase::Prepared
            if current.matches_original(journal) || current.matches_applied(journal) =>
        {
            (CodexSwitchPhase::Prepared, None)
        }
        _ => (
            CodexSwitchPhase::RecoveryRequired,
            Some("current Codex config does not match switch journal fingerprints".to_string()),
        ),
    };

    Ok(CodexSwitchStatus {
        phase,
        enabled,
        managed: true,
        base_url: config_base_url.or_else(|| Some(journal.target_base_url.clone())),
        recovery_reason,
        config_path: paths.config.clone(),
        state_path: paths.state.clone(),
    })
}

struct ConfigInspection {
    model_provider: Option<String>,
    helper_stanza: Option<TomlValue>,
    helper_base_url: Option<String>,
    model_providers_present: bool,
}

fn reject_unowned_helper_config(config: &ConfigInspection) -> Result<(), CodexSwitchError> {
    if config.model_provider.as_deref() == Some(PROVIDER_ID) {
        return Err(CodexSwitchError::OrphanedActiveProvider);
    }
    if config.helper_stanza.is_some() {
        return Err(CodexSwitchError::ForeignProviderStanza);
    }
    Ok(())
}

fn inspect_config(path: &Path, text: &str) -> Result<ConfigInspection, CodexSwitchError> {
    if text.trim().is_empty() {
        return Ok(ConfigInspection {
            model_provider: None,
            helper_stanza: None,
            helper_base_url: None,
            model_providers_present: false,
        });
    }
    let value =
        toml::from_str::<TomlValue>(text).map_err(|error| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: sanitized_toml_parse_reason(error.message(), error.span(), text),
        })?;
    let root = value
        .as_table()
        .ok_or_else(|| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: "root must be a TOML table".to_string(),
        })?;
    let model_provider = match root.get("model_provider") {
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| CodexSwitchError::InvalidConfig {
                    path: path.to_path_buf(),
                    reason: "model_provider must be a string".to_string(),
                })?
                .to_string(),
        ),
        None => None,
    };
    let providers = match root.get("model_providers") {
        Some(value) => Some(
            value
                .as_table()
                .ok_or_else(|| CodexSwitchError::InvalidConfig {
                    path: path.to_path_buf(),
                    reason: "model_providers must be a table".to_string(),
                })?,
        ),
        None => None,
    };
    let helper_stanza = providers.and_then(|providers| providers.get(PROVIDER_ID).cloned());
    let helper_base_url = helper_stanza
        .as_ref()
        .and_then(TomlValue::as_table)
        .and_then(|table| table.get("base_url"))
        .and_then(TomlValue::as_str)
        .map(ToOwned::to_owned);

    Ok(ConfigInspection {
        model_provider,
        helper_stanza,
        helper_base_url,
        model_providers_present: providers.is_some(),
    })
}

fn patch_on(path: &Path, text: &str, base_url: &str) -> Result<OnPatch, CodexSwitchError> {
    let mut document = editable_document(path, text)?;
    let original_model_provider_repr = model_provider_repr_from_document(path, &document)?;
    let root = document.as_table_mut();
    if !root.contains_key("model_providers") {
        root.insert("model_providers", Item::Table(Table::new()));
    }
    let providers = root
        .get_mut("model_providers")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: "model_providers must be a table".to_string(),
        })?;
    let mut helper = Table::new();
    helper.insert("name", editable_value(PROVIDER_NAME));
    helper.insert("base_url", editable_value(base_url));
    helper.insert("wire_api", editable_value("responses"));
    helper.insert("request_max_retries", editable_value(0));
    providers.insert(PROVIDER_ID, Item::Table(helper));
    set_string_preserving_decor(root, "model_provider", PROVIDER_ID);
    Ok(OnPatch {
        text: document.to_string(),
        original_model_provider_repr,
    })
}

fn patch_off(
    path: &Path,
    text: &str,
    journal: &SwitchJournal,
) -> Result<ConfigEdit, CodexSwitchError> {
    let mut document = editable_document(path, text)?;
    let root = document.as_table_mut();
    match (
        journal.original_model_provider.as_deref(),
        journal.original_model_provider_repr.as_deref(),
    ) {
        (Some(provider), Some(repr)) => {
            let replacement = editable_string_from_repr(repr, provider, path)?;
            set_value_preserving_decor(root, "model_provider", replacement);
        }
        (None, None) => {
            root.remove("model_provider");
        }
        _ => {
            return Err(CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: "original model_provider value and representation must agree".to_string(),
            });
        }
    }

    let remove_model_providers =
        if let Some(providers) = root.get_mut("model_providers").and_then(Item::as_table_mut) {
            if let Some(original) = journal.original_helper_stanza.as_ref() {
                providers.insert(PROVIDER_ID, editable_item_from_toml_value(original, path)?);
            } else {
                providers.remove(PROVIDER_ID);
            }
            !journal.original_model_providers_present && providers.is_empty()
        } else {
            false
        };
    if remove_model_providers {
        root.remove("model_providers");
    }

    if !journal.original_config_present && root.is_empty() {
        Ok(ConfigEdit::Remove)
    } else {
        Ok(ConfigEdit::Write(document.to_string()))
    }
}

fn editable_document(path: &Path, text: &str) -> Result<DocumentMut, CodexSwitchError> {
    if text.is_empty() {
        return Ok(DocumentMut::new());
    }
    text.parse::<DocumentMut>()
        .map_err(|error| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: sanitized_toml_parse_reason(error.message(), error.span(), text),
        })
}

fn set_string_preserving_decor(table: &mut Table, key: &str, value: &str) {
    set_value_preserving_decor(table, key, EditableValue::from(value));
}

fn set_value_preserving_decor(table: &mut Table, key: &str, mut replacement: EditableValue) {
    let item = table.entry(key).or_insert(Item::None);
    if let Some(current) = item.as_value_mut() {
        *replacement.decor_mut() = current.decor().clone();
        *current = replacement;
    } else {
        *item = Item::Value(replacement);
    }
}

fn model_provider_repr_from_document(
    path: &Path,
    document: &DocumentMut,
) -> Result<Option<String>, CodexSwitchError> {
    let Some(value) = document
        .as_table()
        .get("model_provider")
        .and_then(Item::as_value)
    else {
        return Ok(None);
    };
    match value {
        EditableValue::String(formatted) => Ok(Some(formatted.display_repr().into_owned())),
        _ => Err(CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: "model_provider must be a string".to_string(),
        }),
    }
}

fn editable_string_from_repr(
    repr: &str,
    expected: &str,
    path: &Path,
) -> Result<EditableValue, CodexSwitchError> {
    let document = format!("model_provider = {repr}\n")
        .parse::<DocumentMut>()
        .map_err(|error| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: error.message().to_string(),
        })?;
    let value = document
        .as_table()
        .get("model_provider")
        .and_then(Item::as_value)
        .filter(|value| value.as_str() == Some(expected))
        .cloned()
        .ok_or_else(|| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: "original model_provider representation does not match its value".to_string(),
        })?;
    Ok(value)
}

fn editable_item_from_toml_value(value: &TomlValue, path: &Path) -> Result<Item, CodexSwitchError> {
    let table = value
        .as_table()
        .ok_or_else(|| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: "original helper stanza must be a TOML table".to_string(),
        })?;
    let body = toml::to_string(table).map_err(|error| CodexSwitchError::InvalidState {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let document = format!("[{PROVIDER_ID}]\n{body}")
        .parse::<DocumentMut>()
        .map_err(|error| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: error.message().to_string(),
        })?;
    document
        .as_table()
        .get(PROVIDER_ID)
        .cloned()
        .ok_or_else(|| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: "original helper stanza is missing".to_string(),
        })
}

fn read_config_snapshot(path: &Path) -> Result<ConfigSnapshot, CodexSwitchError> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let text =
                String::from_utf8(bytes).map_err(|error| CodexSwitchError::InvalidConfig {
                    path: path.to_path_buf(),
                    reason: error.to_string(),
                })?;
            Ok(ConfigSnapshot::from_text(true, text))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ConfigSnapshot::from_text(false, String::new()))
        }
        Err(source) => Err(io_error("read", path, source)),
    }
}

fn validate_config_topology(path: &Path, expected_present: bool) -> Result<(), CodexSwitchError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !expected_present => {
            return Ok(());
        }
        Err(source) => return Err(io_error("inspect file topology for", path, source)),
    };
    if !expected_present {
        return Err(CodexSwitchError::UnsupportedConfigTopology {
            path: path.to_path_buf(),
            reason: "config appeared after the original snapshot".to_string(),
        });
    }
    if metadata.file_type().is_symlink() {
        return Err(CodexSwitchError::UnsupportedConfigTopology {
            path: path.to_path_buf(),
            reason: "symbolic links are not replaced because their topology cannot be restored"
                .to_string(),
        });
    }
    if !metadata.is_file() {
        return Err(CodexSwitchError::UnsupportedConfigTopology {
            path: path.to_path_buf(),
            reason: "config must be a regular file".to_string(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err(CodexSwitchError::UnsupportedConfigTopology {
                path: path.to_path_buf(),
                reason:
                    "hard-linked configs are not replaced because their topology cannot be restored"
                        .to_string(),
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
        };

        let file =
            File::open(path).map_err(|source| io_error("open for topology check", path, source))?;
        let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        let read =
            unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
        if read == 0 {
            return Err(io_error(
                "read hard-link count for",
                path,
                std::io::Error::last_os_error(),
            ));
        }
        let information = unsafe { information.assume_init() };
        if information.nNumberOfLinks > 1 {
            return Err(CodexSwitchError::UnsupportedConfigTopology {
                path: path.to_path_buf(),
                reason:
                    "hard-linked configs are not replaced because their topology cannot be restored"
                        .to_string(),
            });
        }
    }
    Ok(())
}

fn verify_config_before_commit(
    path: &Path,
    expectation: ConfigCommitExpectation<'_>,
) -> Result<(), CodexSwitchError> {
    let (present, current_fingerprint) = read_config_identity(path)?;
    let matches = match expectation.state {
        ExpectedConfigState::Original => {
            present == expectation.journal.original_config_present
                && current_fingerprint == expectation.journal.original_fingerprint
        }
        ExpectedConfigState::Applied => {
            present && current_fingerprint == expectation.journal.applied_fingerprint
        }
    };
    if !matches {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "Codex config changed after the final switch fingerprint check".to_string(),
        });
    }
    validate_config_topology(path, present).map_err(|error| CodexSwitchError::RecoveryRequired {
        reason: error.to_string(),
    })
}

fn read_config_identity(path: &Path) -> Result<(bool, String), CodexSwitchError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok((true, fingerprint(bytes.as_slice()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((false, fingerprint(&[]))),
        Err(source) => Err(io_error("read", path, source)),
    }
}

fn read_journal(path: &Path) -> Result<Option<SwitchJournal>, CodexSwitchError> {
    Ok(read_journal_snapshot(path)?.map(|snapshot| snapshot.journal))
}

fn read_journal_snapshot(path: &Path) -> Result<Option<JournalSnapshot>, CodexSwitchError> {
    let Some(raw) = read_optional_text(path)? else {
        return Ok(None);
    };
    parse_journal(path, raw).map(Some)
}

fn parse_journal(path: &Path, raw: String) -> Result<JournalSnapshot, CodexSwitchError> {
    let journal = serde_json::from_str::<SwitchJournal>(&raw).map_err(|error| {
        CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: error.to_string(),
        }
    })?;
    if journal.version != STATE_VERSION {
        return Err(CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: format!(
                "unsupported state version {}; expected {STATE_VERSION}",
                journal.version
            ),
        });
    }
    Ok(JournalSnapshot { raw, journal })
}

fn read_optional_text(path: &Path) -> Result<Option<String>, CodexSwitchError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(io_error("read", path, source)),
    }
}

fn write_journal(path: &Path, journal: &SwitchJournal) -> Result<(), CodexSwitchError> {
    let text =
        serde_json::to_string_pretty(journal).map_err(|error| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    atomic_write_text(path, text.as_str(), FilePermissions::Secure, None)
}

fn remove_journal(path: &Path) -> Result<(), CodexSwitchError> {
    remove_file_durable(path)
}

fn write_config_edit(
    path: &Path,
    edit: ConfigEdit,
    journal: &SwitchJournal,
    expected: ExpectedConfigState,
) -> Result<(), CodexSwitchError> {
    let expectation = ConfigCommitExpectation {
        journal,
        state: expected,
    };
    match edit {
        ConfigEdit::Write(text) => atomic_write_text(
            path,
            text.as_str(),
            FilePermissions::PreserveOrSecure,
            Some(expectation),
        ),
        ConfigEdit::Remove => {
            verify_config_before_commit(path, expectation)?;
            remove_file_durable(path)
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum FilePermissions {
    Secure,
    PreserveOrSecure,
}

fn atomic_write_text(
    path: &Path,
    text: &str,
    permissions: FilePermissions,
    expectation: Option<ConfigCommitExpectation<'_>>,
) -> Result<(), CodexSwitchError> {
    let parent = path.parent().ok_or_else(|| {
        io_error(
            "resolve parent for",
            path,
            std::io::Error::other("path has no parent directory"),
        )
    })?;
    match permissions {
        FilePermissions::Secure => prepare_state_directory(path)?,
        FilePermissions::PreserveOrSecure => prepare_config_directory(path)?,
    }
    let temp_path = parent.join(format!(".codex-switch-{}.tmp", Uuid::new_v4()));

    let result = (|| {
        #[cfg(unix)]
        let desired_mode = {
            use std::os::unix::fs::PermissionsExt;
            match permissions {
                FilePermissions::Secure => 0o600,
                FilePermissions::PreserveOrSecure => match std::fs::metadata(path) {
                    Ok(metadata) => metadata.permissions().mode() & 0o777,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0o600,
                    Err(source) => return Err(io_error("read metadata for", path, source)),
                },
            }
        };
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(desired_mode);
        }
        #[cfg(not(unix))]
        let _ = permissions;

        let mut file = options
            .open(temp_path.as_path())
            .map_err(|source| io_error("create temporary file for", path, source))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(desired_mode))
                .map_err(|source| io_error("set temporary file permissions for", path, source))?;
        }
        file.write_all(text.as_bytes())
            .map_err(|source| io_error("write temporary file for", path, source))?;
        file.sync_all()
            .map_err(|source| io_error("sync temporary file for", path, source))?;
        drop(file);
        if let Some(expectation) = expectation {
            verify_config_before_commit(path, expectation)?;
        }
        replace_file(temp_path.as_path(), path)?;
        sync_parent_directory(path)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(temp_path);
    }
    result
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), CodexSwitchError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        return Err(io_error(
            "atomically replace",
            destination,
            std::io::Error::last_os_error(),
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), CodexSwitchError> {
    std::fs::rename(source, destination)
        .map_err(|source| io_error("atomically replace", destination, source))
}

fn remove_file_durable(path: &Path) -> Result<(), CodexSwitchError> {
    match std::fs::remove_file(path) {
        Ok(()) => sync_parent_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error("remove", path, source)),
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<(), CodexSwitchError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error("sync parent directory for", path, source))
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<(), CodexSwitchError> {
    Ok(())
}

impl OperationLock {
    fn acquire(path: &Path) -> Result<Self, CodexSwitchError> {
        prepare_state_directory(path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|source| io_error("open", path, source))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(CodexSwitchError::LockBusy {
                path: path.to_path_buf(),
            }),
            Err(TryLockError::Error(source)) => Err(io_error("lock", path, source)),
        }
    }
}

fn prepare_state_directory(path: &Path) -> Result<(), CodexSwitchError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    #[cfg(unix)]
    let missing_directories = missing_directories(parent)?;
    std::fs::create_dir_all(parent)
        .map_err(|source| io_error("create directory", parent, source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|source| io_error("secure directory", parent, source))?;
        for directory in missing_directories.iter().rev() {
            sync_parent_directory(directory)?;
        }
    }
    Ok(())
}

fn prepare_config_directory(path: &Path) -> Result<(), CodexSwitchError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    #[cfg(unix)]
    let missing_directories = missing_directories(parent)?;
    std::fs::create_dir_all(parent)
        .map_err(|source| io_error("create directory", parent, source))?;
    #[cfg(unix)]
    for directory in missing_directories.iter().rev() {
        sync_parent_directory(directory)?;
    }
    Ok(())
}

#[cfg(unix)]
fn missing_directories(path: &Path) -> Result<Vec<PathBuf>, CodexSwitchError> {
    let mut missing = Vec::new();
    let mut current = Some(path);
    while let Some(directory) = current {
        match std::fs::symlink_metadata(directory) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(directory.to_path_buf());
                current = directory.parent();
            }
            Err(source) => return Err(io_error("inspect directory", directory, source)),
        }
    }
    Ok(missing)
}

fn sanitized_toml_parse_reason(
    message: &str,
    span: Option<std::ops::Range<usize>>,
    input: &str,
) -> String {
    let Some(offset) = span.map(|span| span.start.min(input.len())) else {
        return message.to_string();
    };
    let prefix = &input.as_bytes()[..offset];
    let line = prefix.iter().filter(|byte| **byte == b'\n').count() + 1;
    let column = prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(offset + 1, |newline| offset - newline);
    format!("{message} at line {line}, column {column}")
}

fn fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

fn io_error(action: &'static str, path: &Path, source: std::io::Error) -> CodexSwitchError {
    CodexSwitchError::Io {
        action,
        path: path.to_path_buf(),
        source,
    }
}

struct SwitchPaths {
    config: PathBuf,
    config_fingerprint: String,
    state: PathBuf,
    lock: PathBuf,
    legacy_state: PathBuf,
}

impl SwitchPaths {
    fn resolve() -> Result<Self, CodexSwitchError> {
        let state_dir = crate::config::proxy_home_dir().join("state");
        let state_dir = absolute_path(state_dir.as_path())?;
        let state_dir = resolve_existing_ancestor(state_dir.as_path())?;
        let config = resolve_config_path(crate::config::codex_config_path().as_path())?;
        let legacy_state = config.with_file_name(LEGACY_STATE_FILE_NAME);
        Ok(Self {
            config_fingerprint: config_path_fingerprint(config.as_path()),
            config,
            state: state_dir.join(STATE_FILE_NAME),
            lock: state_dir.join(LOCK_FILE_NAME),
            legacy_state,
        })
    }
}

fn resolve_config_path(path: &Path) -> Result<PathBuf, CodexSwitchError> {
    let absolute = absolute_path(path)?;
    let file_name = absolute.file_name().ok_or_else(|| {
        io_error(
            "resolve config file name for",
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "config path has no file name",
            ),
        )
    })?;
    let parent = absolute.parent().ok_or_else(|| {
        io_error(
            "resolve config parent for",
            path,
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "config path has no parent",
            ),
        )
    })?;
    let mut resolved = resolve_existing_ancestor(parent)?;
    resolved.push(file_name);
    Ok(resolved)
}

fn absolute_path(path: &Path) -> Result<PathBuf, CodexSwitchError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|source| io_error("resolve current directory for", path, source))
}

fn config_path_fingerprint(path: &Path) -> String {
    #[cfg(windows)]
    let identity = path.to_string_lossy().to_lowercase();
    #[cfg(not(windows))]
    let identity = path.to_string_lossy();
    fingerprint(identity.as_bytes())
}

fn resolve_existing_ancestor(path: &Path) -> Result<PathBuf, CodexSwitchError> {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or_else(|| {
                    io_error(
                        "resolve existing ancestor for",
                        path,
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "no existing path ancestor",
                        ),
                    )
                })?;
                missing.push(name.to_os_string());
                existing = existing.parent().ok_or_else(|| {
                    io_error(
                        "resolve existing ancestor for",
                        path,
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "no existing path ancestor",
                        ),
                    )
                })?;
            }
            Err(source) => return Err(io_error("inspect path identity for", existing, source)),
        }
    }
    let mut resolved = std::fs::canonicalize(existing)
        .map_err(|source| io_error("canonicalize path identity for", existing, source))?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::*;

    #[test]
    fn switch_wire_labels_are_stable_and_exhaustive() {
        let phases = [
            (CodexSwitchPhase::Off, "off"),
            (CodexSwitchPhase::Prepared, "prepared"),
            (CodexSwitchPhase::Applied, "applied"),
            (CodexSwitchPhase::RecoveryRequired, "recovery_required"),
        ];
        for (phase, expected) in phases {
            assert_eq!(phase.as_str(), expected);
        }

        let changes = [
            (CodexSwitchChange::Applied, "applied"),
            (CodexSwitchChange::Removed, "removed"),
            (CodexSwitchChange::Unchanged, "unchanged"),
            (CodexSwitchChange::Recovered, "recovered"),
        ];
        for (change, expected) in changes {
            assert_eq!(change.as_str(), expected);
        }
    }

    struct TestEnvironment {
        root: PathBuf,
        helper_home: PathBuf,
        codex_home: PathBuf,
        old_helper_home: Option<String>,
        old_codex_home: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl TestEnvironment {
        fn new() -> Self {
            static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
            let guard = LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let root = std::env::temp_dir()
                .join(format!("codex-helper-explicit-switch-{}", Uuid::new_v4()));
            let helper_home = root.join("helper");
            let codex_home = root.join("codex");
            std::fs::create_dir_all(&helper_home).expect("create helper home");
            std::fs::create_dir_all(&codex_home).expect("create Codex home");
            let old_helper_home = std::env::var("CODEX_HELPER_HOME").ok();
            let old_codex_home = std::env::var("CODEX_HOME").ok();
            unsafe {
                std::env::set_var("CODEX_HELPER_HOME", &helper_home);
                std::env::set_var("CODEX_HOME", &codex_home);
            }
            Self {
                root,
                helper_home,
                codex_home,
                old_helper_home,
                old_codex_home,
                _guard: guard,
            }
        }

        fn config_path(&self) -> PathBuf {
            self.codex_home.join("config.toml")
        }

        fn state_path(&self) -> PathBuf {
            self.helper_home.join("state").join(STATE_FILE_NAME)
        }

        fn legacy_state_path(&self) -> PathBuf {
            self.codex_home.join(LEGACY_STATE_FILE_NAME)
        }

        fn lock_path(&self) -> PathBuf {
            self.helper_home.join("state").join(LOCK_FILE_NAME)
        }

        fn write_config(&self, text: &str) {
            std::fs::write(self.config_path(), text).expect("write config");
        }

        fn read_config(&self) -> String {
            std::fs::read_to_string(self.config_path()).expect("read config")
        }
    }

    impl Drop for TestEnvironment {
        fn drop(&mut self) {
            unsafe {
                match self.old_helper_home.take() {
                    Some(value) => std::env::set_var("CODEX_HELPER_HOME", value),
                    None => std::env::remove_var("CODEX_HELPER_HOME"),
                }
                match self.old_codex_home.take() {
                    Some(value) => std::env::set_var("CODEX_HOME", value),
                    None => std::env::remove_var("CODEX_HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn base_url_validation_rejects_credentials_and_ambiguous_suffixes() {
        for value in [
            "file:///tmp/helper.sock",
            "https://user:password@relay.example/v1",
            "https://relay.example/v1?token=secret",
            "https://relay.example/v1#fragment",
        ] {
            assert!(
                ValidatedCodexBaseUrl::parse(value).is_err(),
                "invalid base URL should be rejected: {value}"
            );
        }
        assert_eq!(
            ValidatedCodexBaseUrl::parse("https://relay.example/v1/")
                .expect("valid base URL")
                .as_str(),
            "https://relay.example/v1"
        );
    }

    #[test]
    fn operation_lock_rejects_concurrent_apply_without_mutation() {
        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n";
        env.write_config(original);
        let lock = OperationLock::acquire(env.lock_path().as_path()).expect("hold switch lock");

        assert!(matches!(
            apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            }),
            Err(CodexSwitchError::LockBusy { .. })
        ));
        assert_eq!(env.read_config(), original);
        assert!(!env.state_path().exists());

        drop(lock);
        assert_eq!(
            apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            })
            .expect("apply after releasing lock")
            .change,
            CodexSwitchChange::Applied
        );
    }

    #[test]
    fn malformed_toml_errors_do_not_echo_secret_source_lines() {
        let env = TestEnvironment::new();
        let secret = "never-echo-this-api-key";
        env.write_config(format!("api_key = \"{secret}\"\nmodel_provider = [\n").as_str());

        let error = apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect_err("invalid TOML must fail")
        .to_string();
        assert!(!error.contains(secret));
        assert!(error.contains("line"));
    }

    #[test]
    fn on_and_off_preserve_comments_and_forbidden_codex_files() {
        let env = TestEnvironment::new();
        let original = r#"# top comment
model_provider = "openai"

[model_providers.openai]
# keep this comment
name = "OpenAI"
base_url = "https://api.openai.com/v1"

[projects."/work"]
trust_level = "trusted"
"#;
        env.write_config(original);
        for (name, bytes) in [
            ("auth.json", b"auth sentinel".as_slice()),
            ("models_cache.json", b"cache sentinel".as_slice()),
            ("sqlite/codex-dev.db", b"sqlite sentinel".as_slice()),
        ] {
            let path = env.codex_home.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create sentinel parent");
            }
            std::fs::write(path, bytes).expect("write sentinel");
        }

        let outcome = apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        assert_eq!(outcome.change, CodexSwitchChange::Applied);
        let applied = env.read_config();
        assert!(applied.contains("# top comment"));
        assert!(applied.contains("# keep this comment"));
        assert!(applied.contains("model_provider = \"codex_proxy\""));
        assert!(applied.contains("[model_providers.codex_proxy]"));
        assert!(applied.contains("name = \"codex-helper\""));
        assert!(applied.contains("wire_api = \"responses\""));
        assert!(applied.contains("request_max_retries = 0"));
        assert!(!applied.contains("requires_openai_auth"));
        assert!(!applied.contains("supports_websockets"));

        let outcome = apply(CodexSwitchIntent::Off).expect("switch off");
        assert_eq!(outcome.change, CodexSwitchChange::Removed);
        assert_eq!(env.read_config(), original);
        assert!(!env.state_path().exists());
        assert_eq!(
            std::fs::read(env.codex_home.join("auth.json")).expect("read auth sentinel"),
            b"auth sentinel"
        );
        assert_eq!(
            std::fs::read(env.codex_home.join("models_cache.json")).expect("read cache sentinel"),
            b"cache sentinel"
        );
        assert_eq!(
            std::fs::read(env.codex_home.join("sqlite/codex-dev.db"))
                .expect("read sqlite sentinel"),
            b"sqlite sentinel"
        );
    }

    #[test]
    fn absent_config_round_trips_and_repeated_actions_are_idempotent() {
        let env = TestEnvironment::new();
        std::fs::remove_dir_all(&env.codex_home).expect("remove absent Codex home");
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        assert_eq!(
            apply(on.clone()).expect("first on").change,
            CodexSwitchChange::Applied
        );
        assert_eq!(
            apply(on).expect("second on").change,
            CodexSwitchChange::Unchanged
        );
        assert_eq!(
            apply(CodexSwitchIntent::Off).expect("first off").change,
            CodexSwitchChange::Removed
        );
        assert!(!env.config_path().exists());
        assert_eq!(
            apply(CodexSwitchIntent::Off).expect("second off").change,
            CodexSwitchChange::Unchanged
        );
    }

    #[test]
    fn existing_empty_config_remains_an_existing_empty_file_after_off() {
        let env = TestEnvironment::new();
        env.write_config("");

        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        apply(CodexSwitchIntent::Off).expect("switch off");

        assert!(env.config_path().exists());
        assert_eq!(env.read_config(), "");
    }

    #[test]
    fn whitespace_only_config_round_trips_byte_for_byte() {
        let env = TestEnvironment::new();
        let original = " \n\n  \n";
        env.write_config(original);

        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        apply(CodexSwitchIntent::Off).expect("switch off");

        assert_eq!(env.read_config(), original);
    }

    #[test]
    fn config_without_model_provider_tables_round_trips_byte_for_byte() {
        let env = TestEnvironment::new();
        let original = r#"# retain root comment
model = "gpt-5"

[projects."/work"]
trust_level = "trusted"
"#;
        env.write_config(original);

        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        apply(CodexSwitchIntent::Off).expect("switch off");

        assert_eq!(env.read_config(), original);
    }

    #[test]
    fn literal_selector_representation_round_trips_byte_for_byte() {
        let env = TestEnvironment::new();
        let original = "model_provider  =  'openai' # retain selector comment\n";
        env.write_config(original);

        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        apply(CodexSwitchIntent::Off).expect("switch off");

        assert_eq!(env.read_config(), original);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_writes_preserve_config_mode_and_secure_switch_state() {
        use std::os::unix::fs::PermissionsExt;

        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        std::fs::set_permissions(env.config_path(), std::fs::Permissions::from_mode(0o600))
            .expect("secure original config");

        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        assert_eq!(
            std::fs::metadata(env.config_path())
                .expect("config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(env.state_path())
                .expect("state metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(env.state_path().parent().expect("state parent"))
                .expect("state directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        apply(CodexSwitchIntent::Off).expect("switch off");
        assert_eq!(
            std::fs::metadata(env.config_path())
                .expect("restored config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn foreign_or_orphaned_helper_config_is_never_overwritten() {
        let env = TestEnvironment::new();
        let foreign = r#"model_provider = "openai"

[model_providers.codex_proxy]
name = "foreign"
base_url = "https://foreign.example/v1"
"#;
        env.write_config(foreign);
        assert!(matches!(
            apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            }),
            Err(CodexSwitchError::ForeignProviderStanza)
        ));
        assert_eq!(env.read_config(), foreign);

        let orphaned = r#"model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:3211"
wire_api = "responses"
request_max_retries = 0
"#;
        env.write_config(orphaned);
        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::OrphanedActiveProvider)
        ));
        assert_eq!(env.read_config(), orphaned);
    }

    #[test]
    fn legacy_switch_state_requires_previous_version_recovery_without_being_read() {
        let env = TestEnvironment::new();
        let original = r#"model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:3211"
wire_api = "responses"
request_max_retries = 0
"#;
        let legacy_state = b"\xff\xfe; preserved-auth-material-must-not-appear";
        env.write_config(original);
        std::fs::write(env.legacy_state_path(), legacy_state).expect("write legacy state");
        let resolved_legacy_state =
            std::fs::canonicalize(env.legacy_state_path()).expect("resolve legacy state");
        let legacy_metadata =
            std::fs::symlink_metadata(env.legacy_state_path()).expect("inspect legacy state");

        let status = inspect().expect("legacy state must remain inspectable");
        assert_eq!(status.phase, CodexSwitchPhase::RecoveryRequired);
        assert!(!status.managed);
        assert!(status.enabled);
        assert_eq!(status.base_url.as_deref(), Some("http://127.0.0.1:3211"));
        assert_eq!(status.state_path, resolved_legacy_state);
        let reason = status.recovery_reason.expect("legacy recovery reason");
        assert!(reason.contains("codex-helper-switch-state.json"));
        assert!(reason.contains("v0.20.3"));
        assert!(reason.contains("switch off"));
        assert!(reason.contains("Do not delete, edit, or share"));
        assert!(!reason.contains("preserved-auth-material"));

        for intent in [
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            CodexSwitchIntent::Off,
        ] {
            let error = apply(intent).expect_err("legacy state must block switch mutation");
            assert!(matches!(
                &error,
                CodexSwitchError::LegacySwitchState { path }
                    if path == &resolved_legacy_state
            ));
            let message = error.to_string();
            assert!(message.contains("codex-helper-switch-state.json"));
            assert!(message.contains("v0.20.3"));
            assert!(message.contains("switch off"));
            assert!(!message.contains("preserved-auth-material"));
        }

        assert_eq!(env.read_config(), original);
        let preserved_metadata =
            std::fs::symlink_metadata(env.legacy_state_path()).expect("inspect preserved state");
        assert_eq!(preserved_metadata.len(), legacy_metadata.len());
        assert_eq!(preserved_metadata.file_type(), legacy_metadata.file_type());
        assert!(!env.state_path().exists());
        assert!(!env.lock_path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_legacy_switch_state_symlink_still_blocks_switching() {
        use std::os::unix::fs::symlink;

        let env = TestEnvironment::new();
        symlink(
            env.root.join("missing-legacy-state"),
            env.legacy_state_path(),
        )
        .expect("create dangling legacy state symlink");

        let status = inspect().expect("dangling legacy state must remain diagnosable");
        assert_eq!(status.phase, CodexSwitchPhase::RecoveryRequired);
        assert!(
            status
                .recovery_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("v0.20.3"))
        );
        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::LegacySwitchState { .. })
        ));
        assert!(std::fs::symlink_metadata(env.legacy_state_path()).is_ok());
        assert!(!env.lock_path().exists());
    }

    #[test]
    fn legacy_switch_state_takes_precedence_over_a_new_journal() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("create current switch journal");
        let applied_config = env.read_config();
        let current_state = std::fs::read(env.state_path()).expect("read current journal");

        std::fs::write(env.legacy_state_path(), b"legacy recovery authority")
            .expect("write legacy state");

        let status = inspect().expect("legacy state must remain diagnosable");
        assert_eq!(status.phase, CodexSwitchPhase::RecoveryRequired);
        assert!(status.managed);
        assert!(status.state_path.ends_with(LEGACY_STATE_FILE_NAME));
        assert!(
            status
                .recovery_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("current switch journal also exists"))
        );
        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::LegacySwitchState { .. })
        ));
        assert_eq!(env.read_config(), applied_config);
        assert_eq!(
            std::fs::read(env.state_path()).expect("read preserved current journal"),
            current_state
        );
    }

    #[cfg(unix)]
    #[test]
    fn linked_configs_are_rejected_without_state_or_topology_changes() {
        use std::os::unix::fs::symlink;

        let env = TestEnvironment::new();
        let target = env.root.join("shared-config.toml");
        let original = "model_provider = \"openai\"\n";
        std::fs::write(&target, original).expect("write symlink target");
        symlink(&target, env.config_path()).expect("create config symlink");

        assert!(matches!(
            apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            }),
            Err(CodexSwitchError::UnsupportedConfigTopology { .. })
        ));
        assert!(
            std::fs::symlink_metadata(env.config_path())
                .expect("symlink metadata")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("read target"),
            original
        );
        assert!(!env.state_path().exists());

        std::fs::remove_file(env.config_path()).expect("remove test symlink");
        std::fs::hard_link(&target, env.config_path()).expect("create config hard link");
        assert!(matches!(
            apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            }),
            Err(CodexSwitchError::UnsupportedConfigTopology { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(&target).expect("read target"),
            original
        );
        assert!(!env.state_path().exists());
    }

    #[test]
    fn external_edit_marks_recovery_and_leaves_config_byte_identical() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        let mut edited = env.read_config();
        edited.push_str("\n[projects.\"/external\"]\ntrust_level = \"trusted\"\n");
        env.write_config(edited.as_str());

        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::RecoveryRequired { .. })
        ));
        assert_eq!(env.read_config(), edited);
        let status = inspect().expect("inspect recovery state");
        assert_eq!(status.phase, CodexSwitchPhase::RecoveryRequired);
        assert!(env.state_path().exists());
    }

    #[test]
    fn external_edits_after_prepare_are_never_overwritten() {
        {
            let env = TestEnvironment::new();
            env.write_config("model_provider = \"openai\"\n");
            assert!(matches!(
                apply_with_failpoint(
                    CodexSwitchIntent::On {
                        validated_base_url: ValidatedCodexBaseUrl::local(3211),
                    },
                    ApplyFailpoint::AfterPrepared,
                ),
                Err(CodexSwitchError::InjectedFailure("after_prepared"))
            ));
            let edited = "model_provider = \"openai\"\n# external edit\n";
            env.write_config(edited);

            assert!(matches!(
                apply(CodexSwitchIntent::On {
                    validated_base_url: ValidatedCodexBaseUrl::local(3211),
                }),
                Err(CodexSwitchError::RecoveryRequired { .. })
            ));
            assert_eq!(env.read_config(), edited);
        }

        {
            let env = TestEnvironment::new();
            env.write_config("model_provider = \"openai\"\n");
            apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            })
            .expect("switch on");
            assert!(matches!(
                apply_with_failpoint(CodexSwitchIntent::Off, ApplyFailpoint::AfterPrepared),
                Err(CodexSwitchError::InjectedFailure("after_prepared"))
            ));
            let mut edited = env.read_config();
            edited.push_str("\n# external edit\n");
            env.write_config(edited.as_str());

            assert!(matches!(
                apply(CodexSwitchIntent::Off),
                Err(CodexSwitchError::RecoveryRequired { .. })
            ));
            assert_eq!(env.read_config(), edited);
        }
    }

    #[test]
    fn journal_is_bound_to_the_resolved_codex_config_path() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        let applied = env.read_config();
        let state_before = std::fs::read_to_string(env.state_path()).expect("read state");

        let other_codex_home = env.root.join("other-codex");
        std::fs::create_dir_all(&other_codex_home).expect("create other Codex home");
        let other_config = other_codex_home.join("config.toml");
        std::fs::write(&other_config, &applied).expect("write matching other config");
        unsafe {
            std::env::set_var("CODEX_HOME", &other_codex_home);
        }

        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::RecoveryRequired { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(&other_config).expect("read other config"),
            applied
        );
        assert_eq!(
            std::fs::read_to_string(env.state_path()).expect("re-read state"),
            state_before
        );
        assert_eq!(
            inspect().expect("inspect mismatched path").phase,
            CodexSwitchPhase::RecoveryRequired
        );

        unsafe {
            std::env::set_var("CODEX_HOME", &env.codex_home);
        }
    }

    #[cfg(unix)]
    #[test]
    fn journal_detects_retargeted_codex_home_symlink() {
        use std::os::unix::fs::symlink;

        let env = TestEnvironment::new();
        let codex_home_link = env.root.join("current-codex-home");
        symlink(&env.codex_home, &codex_home_link).expect("link first Codex home");
        unsafe {
            std::env::set_var("CODEX_HOME", &codex_home_link);
        }
        env.write_config("model_provider = \"openai\"\n");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on through linked home");
        let applied = env.read_config();

        let other_codex_home = env.root.join("retargeted-codex");
        std::fs::create_dir_all(&other_codex_home).expect("create retargeted Codex home");
        let other_config = other_codex_home.join("config.toml");
        std::fs::write(&other_config, &applied).expect("write matching retargeted config");
        std::fs::remove_file(&codex_home_link).expect("remove first home link");
        symlink(&other_codex_home, &codex_home_link).expect("retarget Codex home link");

        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::RecoveryRequired { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(other_config).expect("read retargeted config"),
            applied
        );

        unsafe {
            std::env::set_var("CODEX_HOME", &env.codex_home);
        }
    }

    #[test]
    fn switching_to_a_different_target_is_rejected_without_mutation() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        let config_before = env.read_config();
        let state_before = std::fs::read_to_string(env.state_path()).expect("read state");

        assert!(matches!(
            apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(4321),
            }),
            Err(CodexSwitchError::AlreadyAppliedToDifferentTarget { .. })
        ));
        assert_eq!(env.read_config(), config_before);
        assert_eq!(
            std::fs::read_to_string(env.state_path()).expect("re-read state"),
            state_before
        );
    }

    #[test]
    fn prepared_on_can_be_retried_or_cancelled_without_guessing() {
        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n";
        env.write_config(original);
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };

        assert!(matches!(
            apply_with_failpoint(on.clone(), ApplyFailpoint::AfterPrepared),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));
        assert_eq!(env.read_config(), original);
        assert_eq!(
            inspect().expect("prepared status").phase,
            CodexSwitchPhase::Prepared
        );
        assert_eq!(
            apply(on).expect("resume on").change,
            CodexSwitchChange::Applied
        );

        apply(CodexSwitchIntent::Off).expect("reset");
        assert!(matches!(
            apply_with_failpoint(
                CodexSwitchIntent::On {
                    validated_base_url: ValidatedCodexBaseUrl::local(3211),
                },
                ApplyFailpoint::AfterPrepared,
            ),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));
        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("cancel prepared on")
                .change,
            CodexSwitchChange::Recovered
        );
        assert_eq!(env.read_config(), original);
    }

    #[cfg(unix)]
    #[test]
    fn topology_change_after_prepare_marks_recovery_without_replacing_link() {
        use std::os::unix::fs::symlink;

        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n";
        env.write_config(original);
        assert!(matches!(
            apply_with_failpoint(
                CodexSwitchIntent::On {
                    validated_base_url: ValidatedCodexBaseUrl::local(3211),
                },
                ApplyFailpoint::AfterPrepared,
            ),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));

        let target = env.root.join("replacement-config.toml");
        std::fs::write(&target, original).expect("write replacement target");
        std::fs::remove_file(env.config_path()).expect("remove original config");
        symlink(&target, env.config_path()).expect("replace config with symlink");

        assert!(matches!(
            apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            }),
            Err(CodexSwitchError::RecoveryRequired { .. })
        ));
        assert!(
            std::fs::symlink_metadata(env.config_path())
                .expect("symlink metadata")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("read target"),
            original
        );
        assert_eq!(
            inspect().expect("inspect recovery").phase,
            CodexSwitchPhase::RecoveryRequired
        );
    }

    #[cfg(unix)]
    #[test]
    fn inspect_reports_recovery_for_same_byte_linked_config() {
        use std::os::unix::fs::symlink;

        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        let applied = env.read_config();
        let state_before = std::fs::read_to_string(env.state_path()).expect("read state");
        let target = env.root.join("linked-applied-config.toml");
        std::fs::write(&target, applied).expect("write linked target");
        std::fs::remove_file(env.config_path()).expect("remove regular config");
        symlink(&target, env.config_path()).expect("link matching config");

        let status = inspect().expect("inspect linked config");
        assert_eq!(status.phase, CodexSwitchPhase::RecoveryRequired);
        assert!(
            std::fs::symlink_metadata(env.config_path())
                .expect("link metadata")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(env.state_path()).expect("re-read state"),
            state_before
        );
    }

    #[test]
    fn config_write_before_applied_state_is_recovered_from_planned_fingerprint() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        assert!(matches!(
            apply_with_failpoint(on.clone(), ApplyFailpoint::AfterConfigWrite),
            Err(CodexSwitchError::InjectedFailure("after_config_write"))
        ));
        assert!(
            env.read_config()
                .contains("model_provider = \"codex_proxy\"")
        );
        assert_eq!(
            inspect().expect("prepared status").phase,
            CodexSwitchPhase::Prepared
        );
        assert_eq!(
            apply(on).expect("finalize applied state").change,
            CodexSwitchChange::Recovered
        );
        assert_eq!(
            inspect().expect("applied status").phase,
            CodexSwitchPhase::Applied
        );
    }

    #[test]
    fn prepared_off_can_resume_before_or_after_the_config_write() {
        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n";
        env.write_config(original);
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        apply(on.clone()).expect("switch on");

        assert!(matches!(
            apply_with_failpoint(CodexSwitchIntent::Off, ApplyFailpoint::AfterPrepared),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));
        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("resume prepared off")
                .change,
            CodexSwitchChange::Removed
        );
        assert_eq!(env.read_config(), original);

        apply(on).expect("switch on again");
        assert!(matches!(
            apply_with_failpoint(CodexSwitchIntent::Off, ApplyFailpoint::AfterConfigWrite),
            Err(CodexSwitchError::InjectedFailure("after_config_write"))
        ));
        assert_eq!(env.read_config(), original);
        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("finalize written off")
                .change,
            CodexSwitchChange::Recovered
        );
        assert_eq!(env.read_config(), original);
    }

    #[test]
    fn state_contains_only_fingerprints_and_non_secret_switch_metadata() {
        let env = TestEnvironment::new();
        env.write_config(
            r#"model_provider = 'private-provider' # never-copy-selector-comment-secret

[model_providers.private-provider]
name = "private"
base_url = "https://private.example/v1"
api_key = "never-copy-this-secret"
"#,
        );
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");
        let state = std::fs::read_to_string(env.state_path()).expect("read state");
        assert!(state.contains("\"phase\": \"applied\""));
        assert!(state.contains("\"operation_id\""));
        assert!(state.contains("\"config_path_fingerprint\""));
        assert!(state.contains("\"original_fingerprint\""));
        assert!(state.contains("\"applied_fingerprint\""));
        assert!(!state.contains("auth.json"));
        assert!(!state.contains("models_cache.json"));
        assert!(!state.contains("OPENAI_API_KEY"));
        assert!(!state.contains("never-copy-this-secret"));
        assert!(!state.contains("private.example"));
        assert!(!state.contains("never-copy-selector-comment-secret"));
        assert!(!state.contains(env.codex_home.to_string_lossy().as_ref()));
        assert!(
            !env.codex_home
                .join("codex-helper-switch-state.json")
                .exists()
        );
    }
}
