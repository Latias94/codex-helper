use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use toml::Value as TomlValue;
use toml_edit::{DocumentMut, Item, Table, Value as EditableValue, value as editable_value};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::auth_resolution::{
    CodexAuthMetadata, codex_auth_metadata_from_json, read_codex_auth_metadata,
};
use crate::config::{
    CODEX_CLIENT_RUNTIME_PATCH_HEADER, CodexAuthFacadeStrategy, CodexClientPatchConfig,
    CodexClientRuntimePatch, CodexFeatureBoolPatch, CodexTomlBoolPatch,
};
#[cfg(test)]
use crate::config::{CodexClientPreset, CodexCompactionStrategy};

const STATE_VERSION: u32 = 2;
const LEGACY_STATE_VERSION: u32 = 1;
const PROVIDER_ID: &str = "codex_proxy";
const COMPATIBLE_PROVIDER_NAME: &str = "codex-helper";
// Current Codex treats a non-empty actor header as a client capability gate. The proxy consumes
// this exact non-secret marker locally before applying upstream authentication policy.
pub const CODEX_CLIENT_FACADE_ACTOR_HEADER: &str = "x-openai-actor-authorization";
pub const CODEX_CLIENT_FACADE_ACTOR_VALUE: &str = "codex-helper-client-facade-v1";
const UNSCOPED_STATE_FILE_NAME: &str = "codex-switch.json";
const SCOPED_STATE_FILE_PREFIX: &str = "codex-switch-";
const SCOPED_STATE_FILE_SUFFIX: &str = ".json";
const LOCK_FILE_NAME: &str = "codex-switch.lock";
const LEGACY_STATE_FILE_NAME: &str = "codex-helper-switch-state.json";
const SWITCH_TEMP_FILE_PREFIX: &str = ".codex-switch-v1-";
const LEGACY_SWITCH_TEMP_FILE_PREFIX: &str = ".codex-switch-";
const SWITCH_TEMP_FILE_SUFFIX: &str = ".tmp";
const SWITCH_DELETE_TOMBSTONE_PREFIX: &str = ".codex-switch-delete-v1-";
const SWITCH_CAPTURE_FILE_PREFIX: &str = ".codex-switch-capture-v1-";
const AUTH_BACKUP_FILE_PREFIX: &str = "codex-switch-auth-v1-";
const AUTH_BACKUP_FILE_SUFFIX: &str = ".bak";
const HELPER_STANZA_BACKUP_FILE_PREFIX: &str = "codex-switch-helper-stanza-v1-";
const HELPER_STANZA_BACKUP_FILE_SUFFIX: &str = ".bak";
#[cfg(windows)]
const WINDOWS_FILE_OPERATION_ATTEMPTS: usize = 10;
#[cfg(windows)]
const WINDOWS_FILE_OPERATION_MAX_BACKOFF: std::time::Duration =
    std::time::Duration::from_millis(16);

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
    pub client_patch: Option<CodexClientPatchConfig>,
    pub recovery_reason: Option<String>,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSwitchOutcome {
    pub change: CodexSwitchChange,
    pub status: CodexSwitchStatus,
    pub restore_lease: Option<CodexSwitchRestoreLease>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexSwitchTargetRestoreOutcome {
    Restored(CodexSwitchOutcome),
    Unchanged(CodexSwitchStatus),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSwitchRestoreLease {
    state_path: PathBuf,
    config_path_fingerprint: String,
    operation_id: String,
    applied_fingerprint: String,
    auto_restore_generation: String,
}

#[derive(Debug, Error)]
pub enum CodexSwitchError {
    #[error("invalid Codex client patch: {reason}")]
    InvalidClientPatch { reason: String },
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
    #[error("cannot prepare Codex auth facade at {path:?}: {reason}")]
    InvalidAuth { path: PathBuf, reason: String },
    #[error("failed to parse Codex switch state {path:?}: {reason}")]
    InvalidState { path: PathBuf, reason: String },
    #[error("Codex switch operation is already running; lock is held at {path:?}")]
    LockBusy { path: PathBuf },
    #[error(
        "Codex config already selects codex_proxy without helper ownership state; manual reconciliation is required"
    )]
    OrphanedActiveProvider,
    #[error(
        "legacy Codex switch state exists at {path:?}. The next `switch on` or `switch off` will attempt a safe automatic recovery. Do not run old and new switch commands concurrently, and do not delete, edit, or share this file because it may contain authentication recovery data"
    )]
    LegacySwitchState { path: PathBuf },
    #[error(
        "legacy Codex switch state at {legacy_path:?} conflicts with current switch journal at {current_path:?}; neither state was modified"
    )]
    LegacySwitchStateConflict {
        legacy_path: PathBuf,
        current_path: PathBuf,
    },
    #[error(
        "cannot safely recover legacy Codex switch state at {path:?}: {reason}; the legacy recovery state was preserved for reconciliation"
    )]
    LegacyRecoveryRequired { path: PathBuf, reason: String },
    #[error("unsupported Codex config file topology at {path:?}: {reason}")]
    UnsupportedConfigTopology { path: PathBuf, reason: String },
    #[error(
        "Codex helper is already applied to {current}; run explicit switch off before switching to {requested}"
    )]
    AlreadyAppliedToDifferentTarget { current: String, requested: String },
    #[error("Codex switch recovery is required: {reason}")]
    RecoveryRequired { reason: String },
    #[error("Codex switch state changed repeatedly while it was being inspected")]
    UnstableInspection,
    #[error(
        "restoring helper-owned Codex fields would not reproduce the original managed projection"
    )]
    RestoreFingerprintMismatch,
    #[error(
        "ephemeral Codex switch failed ({apply_error}); automatic recovery also failed ({recovery_error})"
    )]
    EphemeralRecoveryFailed {
        apply_error: String,
        recovery_error: String,
    },
    #[cfg(test)]
    #[error("injected Codex switch failure at {0}")]
    InjectedFailure(&'static str),
}

fn invalid_client_patch_error(error: anyhow::Error) -> CodexSwitchError {
    CodexSwitchError::InvalidClientPatch {
        reason: error.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum JournalPhase {
    Prepared,
    Applied,
    Restored,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_config: Option<PendingConfigCommit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auto_restore_generation: Option<String>,
    original_model_provider: Option<String>,
    original_model_provider_repr: Option<String>,
    #[serde(default, skip_serializing)]
    original_helper_stanza: Option<TomlValue>,
    #[serde(default, skip_serializing)]
    original_helper_stanza_repr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    helper_stanza_backup: Option<HelperStanzaJournal>,
    original_model_providers_present: bool,
    target_base_url: String,
    client_patch: CodexClientPatchConfig,
    #[serde(default)]
    original_features_present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_remote_compaction_v2: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_image_generation: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_patch: Option<AuthJournal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auth_recovery_patch: Option<CodexClientPatchConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct PendingConfigCommit {
    source_present: bool,
    source_fingerprint: String,
    destination_present: bool,
    destination_fingerprint: String,
}

impl PendingConfigCommit {
    fn new(source: &ConfigSnapshot, destination: &ConfigSnapshot) -> Self {
        Self {
            source_present: source.present,
            source_fingerprint: source.fingerprint.clone(),
            destination_present: destination.present,
            destination_fingerprint: destination.fingerprint.clone(),
        }
    }

    fn source_matches(&self, snapshot: &ConfigSnapshot) -> bool {
        snapshot.present == self.source_present && snapshot.fingerprint == self.source_fingerprint
    }

    fn destination_matches(&self, snapshot: &ConfigSnapshot) -> bool {
        snapshot.present == self.destination_present
            && snapshot.fingerprint == self.destination_fingerprint
    }
}

impl SwitchJournal {
    fn recovery_client_patch(&self) -> CodexClientPatchConfig {
        self.client_patch
    }

    fn records_complete_client_patch(&self, client_patch: CodexClientPatchConfig) -> bool {
        self.client_patch == client_patch
    }

    fn recorded_auth_client_patch(&self) -> CodexClientPatchConfig {
        self.auth_recovery_patch
            .unwrap_or_else(|| self.recovery_client_patch())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct AuthJournal {
    auth_path_fingerprint: String,
    original_present: bool,
    original_fingerprint: String,
    applied_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backup_file_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct HelperStanzaJournal {
    original_present: bool,
    original_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backup_file_name: Option<String>,
}

#[derive(Debug, Clone)]
struct RetainedAuthJournal {
    client_patch: CodexClientPatchConfig,
    auth: AuthJournal,
}

struct PreparedAuthJournal {
    auth: Option<AuthJournal>,
    recovery_patch: Option<CodexClientPatchConfig>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct LegacySwitchState {
    version: u32,
    #[serde(default)]
    patch_mode: Option<LegacyCodexPatchMode>,
    #[serde(default)]
    responses_websocket: bool,
    #[serde(default)]
    compaction: LegacyCodexCompactionStrategy,
    original_config_absent: bool,
    original_model_provider: Option<String>,
    original_codex_proxy: Option<TomlValue>,
    had_model_providers: bool,
    #[serde(default)]
    original_auth_json_absent: bool,
    #[serde(default)]
    original_auth_json: Option<String>,
    #[serde(default)]
    patched_auth_json: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum LegacyCodexPatchMode {
    Default,
    ChatGptBridge,
    ImagegenBridge,
    #[serde(
        rename = "official-relay",
        alias = "official_relay",
        alias = "official-relay-bridge",
        alias = "official_relay_bridge"
    )]
    OfficialRelay,
    #[serde(
        rename = "official-imagegen",
        alias = "official_imagegen",
        alias = "official-imagegen-bridge",
        alias = "official_imagegen_bridge"
    )]
    OfficialImagegen,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum LegacyCodexCompactionStrategy {
    #[default]
    Auto,
    Local,
    #[serde(alias = "remote_v1")]
    RemoteV1,
    #[serde(alias = "remote_v2")]
    RemoteV2,
}

impl LegacyCodexPatchMode {
    fn uses_official_identity(self) -> bool {
        matches!(self, Self::OfficialRelay | Self::OfficialImagegen)
    }

    fn patches_auth(self) -> bool {
        matches!(
            self,
            Self::ChatGptBridge | Self::ImagegenBridge | Self::OfficialImagegen
        )
    }
}

impl LegacySwitchState {
    fn patch_mode(&self) -> LegacyCodexPatchMode {
        self.patch_mode.unwrap_or(LegacyCodexPatchMode::Default)
    }

    fn uses_official_identity(&self) -> bool {
        match self.compaction {
            LegacyCodexCompactionStrategy::Auto => self.patch_mode().uses_official_identity(),
            LegacyCodexCompactionStrategy::Local => false,
            LegacyCodexCompactionStrategy::RemoteV1 | LegacyCodexCompactionStrategy::RemoteV2 => {
                true
            }
        }
    }
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

struct LocatedJournalSnapshot {
    path: PathBuf,
    snapshot: JournalSnapshot,
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

    fn matches_original_auth(&self, journal: &AuthJournal) -> bool {
        self.present == journal.original_present && self.fingerprint == journal.original_fingerprint
    }

    fn matches_applied_auth(&self, journal: &AuthJournal) -> bool {
        self.present && self.fingerprint == journal.applied_fingerprint
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ManagedConfigMatches {
    original: bool,
    applied: bool,
}

#[derive(Debug)]
enum ConfigEdit {
    Write(String),
    Remove,
}

#[derive(Debug, Clone, Copy)]
enum ExpectedAuthState {
    Original,
    Applied,
}

#[derive(Clone, Copy)]
struct ConfigCommitExpectation<'a> {
    journal: &'a SwitchJournal,
    expected: &'a ConfigSnapshot,
}

#[derive(Clone, Copy)]
struct AuthCommitExpectation<'a> {
    paths: &'a SwitchPaths,
    switch_journal: &'a SwitchJournal,
    journal: &'a AuthJournal,
    state: ExpectedAuthState,
}

#[derive(Clone, Copy)]
enum FileCommitExpectation<'a> {
    Journal(ConfigCommitExpectation<'a>),
    Auth(AuthCommitExpectation<'a>),
    LegacySnapshot {
        expected: &'a ConfigSnapshot,
        legacy_path: &'a Path,
    },
}

#[derive(Debug, Clone, Copy)]
enum ManagedCommitRole {
    Config,
    Auth,
}

impl ManagedCommitRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Auth => "auth",
        }
    }
}

impl<'a> FileCommitExpectation<'a> {
    fn managed_capture(self) -> Option<(&'a str, ManagedCommitRole)> {
        match self {
            Self::Journal(expectation) => Some((
                expectation.journal.operation_id.as_str(),
                ManagedCommitRole::Config,
            )),
            Self::Auth(expectation) => Some((
                expectation.switch_journal.operation_id.as_str(),
                ManagedCommitRole::Auth,
            )),
            Self::LegacySnapshot { .. } => None,
        }
    }

    fn expected_present(self) -> bool {
        match self {
            Self::Journal(expectation) => expectation.expected.present,
            Self::Auth(expectation) => match expectation.state {
                ExpectedAuthState::Original => expectation.journal.original_present,
                ExpectedAuthState::Applied => true,
            },
            Self::LegacySnapshot { expected, .. } => expected.present,
        }
    }
}

impl ConfigEdit {
    fn snapshot(&self) -> ConfigSnapshot {
        match self {
            Self::Write(text) => ConfigSnapshot::from_text(true, text.clone()),
            Self::Remove => ConfigSnapshot::from_text(false, String::new()),
        }
    }

    fn into_snapshot(self) -> ConfigSnapshot {
        match self {
            Self::Write(text) => ConfigSnapshot::from_text(true, text),
            Self::Remove => ConfigSnapshot::from_text(false, String::new()),
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
    AfterAuthWrite,
    AfterAuthRestore,
    AfterHelperStanzaBackupRemoval,
    AfterLegacyConfigRestore,
    AfterLegacyAuthRestore,
}

impl ApplyFailpoint {
    #[cfg(test)]
    fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::AfterPrepared => "after_prepared",
            Self::AfterConfigWrite => "after_config_write",
            Self::AfterAuthWrite => "after_auth_write",
            Self::AfterAuthRestore => "after_auth_restore",
            Self::AfterHelperStanzaBackupRemoval => "after_helper_stanza_backup_removal",
            Self::AfterLegacyConfigRestore => "after_legacy_config_restore",
            Self::AfterLegacyAuthRestore => "after_legacy_auth_restore",
        }
    }
}

struct OnPatch {
    text: String,
    original_model_provider_repr: Option<String>,
    original_helper_stanza_repr: Option<String>,
    original_features_present: bool,
    original_remote_compaction_v2: Option<bool>,
    original_image_generation: Option<bool>,
}

struct PlannedOnWrite {
    original_text: String,
    applied_text: String,
}

pub fn apply(intent: CodexSwitchIntent) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    apply_with_client_patch(intent, CodexClientPatchConfig::default())
}

pub fn apply_with_client_patch(
    intent: CodexSwitchIntent,
    client_patch: CodexClientPatchConfig,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    apply_with_client_patch_and_failpoint(intent, client_patch, ApplyFailpoint::None)
}

pub fn acquire_ephemeral_local_codex(
    runtime_owner: &crate::runtime_manager::RuntimeOwnerLease,
    client_patch: CodexClientPatchConfig,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    if runtime_owner.service_name() != "codex" || !runtime_owner.lifecycle_mode().owns_runtime() {
        return Err(CodexSwitchError::InvalidBaseUrl {
            reason: "ephemeral Codex switching requires an owned local Codex runtime".to_string(),
        });
    }
    acquire_ephemeral_with_client_patch_and_failpoint(
        ValidatedCodexBaseUrl::local(runtime_owner.proxy_port()),
        client_patch,
        ApplyFailpoint::None,
        runtime_owner.instance_id().to_string(),
    )
}

pub fn inspect() -> Result<CodexSwitchStatus, CodexSwitchError> {
    let paths = SwitchPaths::resolve()?;
    if legacy_state_present(paths.legacy_state.as_path())? {
        return legacy_switch_status(&paths);
    }
    for _ in 0..3 {
        let before = read_inspection_journal_snapshot(&paths)?;
        let current = read_config_snapshot(paths.config.as_path())?;
        let after = read_inspection_journal_snapshot(&paths)?;
        match (before, after) {
            (None, None) => return status_after_legacy_recheck(&paths, &current, None, None),
            (Some(before), Some(after))
                if before.path == after.path && before.snapshot.raw == after.snapshot.raw =>
            {
                return status_after_legacy_recheck(
                    &paths,
                    &current,
                    Some(&after.snapshot.journal),
                    Some(after.path.as_path()),
                );
            }
            (Some(before), Some(after))
                if before.path == after.path
                    && before.snapshot.journal == after.snapshot.journal =>
            {
                return status_after_legacy_recheck(
                    &paths,
                    &current,
                    Some(&after.snapshot.journal),
                    Some(after.path.as_path()),
                );
            }
            _ => {}
        }
    }
    Err(CodexSwitchError::UnstableInspection)
}

/// Restores one exact helper-managed target without consulting the process-wide `CODEX_HOME`.
pub fn restore_managed_applied_for_target(
    client_home: &Path,
    expected_target: &ValidatedCodexBaseUrl,
) -> Result<CodexSwitchTargetRestoreOutcome, CodexSwitchError> {
    let paths = SwitchPaths::resolve_for_codex_home(client_home)?;
    let _lock = OperationLock::acquire(paths.lock.as_path())?;
    if legacy_state_present(paths.legacy_state.as_path())? {
        if let Some(current_path) = current_journal_path_entry(&paths)? {
            return Err(CodexSwitchError::LegacySwitchStateConflict {
                legacy_path: paths.legacy_state.clone(),
                current_path,
            });
        }
        let current = read_config_snapshot(paths.config.as_path())?;
        validate_config_topology(paths.config.as_path(), current.present)?;
        let inspection = inspect_config(paths.config.as_path(), current.text.as_str())?;
        let exact_applied_target = inspection.model_provider.as_deref() == Some(PROVIDER_ID)
            && inspection.helper_base_url.as_deref() == Some(expected_target.as_str());
        if !exact_applied_target {
            return legacy_switch_status(&paths).map(CodexSwitchTargetRestoreOutcome::Unchanged);
        }
        recover_legacy_switch_state(&paths, ApplyFailpoint::None)?;
        return outcome(&paths, CodexSwitchChange::Recovered)
            .map(CodexSwitchTargetRestoreOutcome::Restored);
    }

    let current = read_config_snapshot(paths.config.as_path())?;
    let journal = read_journal(paths.state.as_path())?;
    if journal.is_some() && read_matching_unscoped_journal_snapshot(&paths)?.is_some() {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: format!(
                "matching unscoped switch state at {:?} conflicts with scoped switch state at {:?}; neither state was modified",
                paths.unscoped_state, paths.state
            ),
        });
    }
    let status = status_from_snapshot(&paths, &current, journal.as_ref(), None)?;
    let exact_applied_target = journal.as_ref().is_some_and(|journal| {
        journal.phase == JournalPhase::Applied
            && journal.operation == JournalOperation::On
            && journal.config_path_fingerprint == paths.config_fingerprint
            && journal.target_base_url == expected_target.as_str()
    }) && status.phase == CodexSwitchPhase::Applied
        && status.enabled
        && status.base_url.as_deref() == Some(expected_target.as_str());
    if !exact_applied_target {
        return Ok(CodexSwitchTargetRestoreOutcome::Unchanged(status));
    }

    let mut journal =
        read_journal(paths.state.as_path())?.ok_or(CodexSwitchError::UnstableInspection)?;
    if journal.phase != JournalPhase::Applied
        || journal.operation != JournalOperation::On
        || journal.config_path_fingerprint != paths.config_fingerprint
        || journal.target_base_url != expected_target.as_str()
    {
        return Err(CodexSwitchError::UnstableInspection);
    }
    let current = read_config_snapshot(paths.config.as_path())?;
    if let Err(error) = ensure_helper_stanza_backup(&paths, &mut journal, &current) {
        return match error {
            CodexSwitchError::RecoveryRequired { reason } => mark_recovery(&paths, journal, reason),
            error => Err(error),
        };
    }
    apply_off(&paths, current, Some(journal), ApplyFailpoint::None)
        .map(CodexSwitchTargetRestoreOutcome::Restored)
}

#[cfg(test)]
fn apply_with_failpoint(
    intent: CodexSwitchIntent,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    apply_with_client_patch_and_failpoint(intent, CodexClientPatchConfig::default(), failpoint)
}

fn apply_with_client_patch_and_failpoint(
    intent: CodexSwitchIntent,
    client_patch: CodexClientPatchConfig,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    client_patch.compile().map_err(invalid_client_patch_error)?;
    let paths = SwitchPaths::resolve()?;
    let _lock = OperationLock::acquire(paths.lock.as_path())?;
    apply_with_client_patch_locked(&paths, intent, client_patch, failpoint, None)
}

fn acquire_ephemeral_with_client_patch_and_failpoint(
    validated_base_url: ValidatedCodexBaseUrl,
    client_patch: CodexClientPatchConfig,
    failpoint: ApplyFailpoint,
    auto_restore_generation: String,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    client_patch.compile().map_err(invalid_client_patch_error)?;
    let paths = SwitchPaths::resolve()?;
    let _lock = OperationLock::acquire(paths.lock.as_path())?;
    let result = apply_with_client_patch_locked(
        &paths,
        CodexSwitchIntent::On { validated_base_url },
        client_patch,
        failpoint,
        Some(auto_restore_generation.clone()),
    );
    match result {
        Ok(outcome) => Ok(outcome),
        Err(apply_error) => {
            match compensate_ephemeral_apply_locked(&paths, auto_restore_generation.as_str()) {
                Ok(()) => Err(apply_error),
                Err(recovery_error) => Err(CodexSwitchError::EphemeralRecoveryFailed {
                    apply_error: apply_error.to_string(),
                    recovery_error: recovery_error.to_string(),
                }),
            }
        }
    }
}

fn compensate_ephemeral_apply_locked(
    paths: &SwitchPaths,
    auto_restore_generation: &str,
) -> Result<(), CodexSwitchError> {
    let Some(journal) = read_journal(paths.state.as_path())? else {
        return Ok(());
    };
    if journal.auto_restore_generation.as_deref() != Some(auto_restore_generation) {
        return Ok(());
    }
    apply_with_client_patch_locked(
        paths,
        CodexSwitchIntent::Off,
        journal.recovery_client_patch(),
        ApplyFailpoint::None,
        None,
    )?;
    Ok(())
}

fn apply_with_client_patch_locked(
    paths: &SwitchPaths,
    intent: CodexSwitchIntent,
    client_patch: CodexClientPatchConfig,
    failpoint: ApplyFailpoint,
    requested_auto_restore_generation: Option<String>,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    cleanup_managed_switch_artifacts(paths)?;
    let current_state_path = current_journal_path_entry(paths)?;
    if legacy_state_present(paths.legacy_state.as_path())? {
        if let Some(current_path) = current_state_path {
            return Err(CodexSwitchError::LegacySwitchStateConflict {
                legacy_path: paths.legacy_state.clone(),
                current_path,
            });
        }
        recover_legacy_switch_state(paths, failpoint)?;
        let current = read_config_snapshot(paths.config.as_path())?;
        return match intent {
            CodexSwitchIntent::On { validated_base_url } => begin_on(
                paths,
                current,
                validated_base_url,
                client_patch,
                failpoint,
                requested_auto_restore_generation,
            ),
            CodexSwitchIntent::Off => outcome(paths, CodexSwitchChange::Recovered),
        };
    }
    migrate_matching_unscoped_journal(paths)?;
    let mut journal = read_journal(paths.state.as_path())?;
    let mut current = read_config_snapshot(paths.config.as_path())?;
    if let Some(mut loaded) = journal.take() {
        ensure_journal_config_matches(paths, &loaded)?;
        if let Err(error) = ensure_helper_stanza_backup(paths, &mut loaded, &current) {
            return match error {
                CodexSwitchError::RecoveryRequired { reason } => {
                    mark_recovery(paths, loaded, reason)
                }
                error => Err(error),
            };
        }
        recover_interrupted_file_captures(paths, &mut loaded)?;
        journal = Some(loaded);
        current = read_config_snapshot(paths.config.as_path())?;
    }

    match intent {
        CodexSwitchIntent::On { validated_base_url } => apply_on(
            paths,
            current,
            journal,
            validated_base_url,
            client_patch,
            failpoint,
            requested_auto_restore_generation,
        ),
        CodexSwitchIntent::Off => apply_off(paths, current, journal, failpoint),
    }
}

pub fn restore_if_owned(
    lease: &CodexSwitchRestoreLease,
) -> Result<Option<CodexSwitchOutcome>, CodexSwitchError> {
    let paths = SwitchPaths::resolve()?;
    if paths.state != lease.state_path || paths.config_fingerprint != lease.config_path_fingerprint
    {
        return Ok(None);
    }
    let _lock = OperationLock::acquire(paths.lock.as_path())?;
    if legacy_state_present(paths.legacy_state.as_path())? {
        return Ok(None);
    }
    let Some(journal) = read_journal(paths.state.as_path())? else {
        return Ok(None);
    };
    if journal.phase != JournalPhase::Applied
        || journal.operation != JournalOperation::On
        || journal.operation_id != lease.operation_id
        || journal.applied_fingerprint != lease.applied_fingerprint
        || journal.config_path_fingerprint != lease.config_path_fingerprint
        || journal.auto_restore_generation.as_deref()
            != Some(lease.auto_restore_generation.as_str())
    {
        return Ok(None);
    }

    apply_with_client_patch_locked(
        &paths,
        CodexSwitchIntent::Off,
        journal.recovery_client_patch(),
        ApplyFailpoint::None,
        None,
    )
    .map(Some)
}

pub fn restore_if_owned_with_retry(
    lease: &CodexSwitchRestoreLease,
) -> Result<Option<CodexSwitchOutcome>, CodexSwitchError> {
    const RETRY_DELAYS_MS: [u64; 6] = [0, 20, 50, 100, 200, 400];
    let mut last_error = None;
    for delay_ms in RETRY_DELAYS_MS {
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        match restore_if_owned(lease) {
            Ok(outcome) => return Ok(outcome),
            Err(error) if retryable_restore_error(&error) => last_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_error.expect("restore retry schedule is non-empty"))
}

fn retryable_restore_error(error: &CodexSwitchError) -> bool {
    if matches!(error, CodexSwitchError::LockBusy { .. }) {
        return true;
    }
    #[cfg(windows)]
    if let CodexSwitchError::Io { source, .. } = error {
        return matches!(source.raw_os_error(), Some(32 | 33));
    }
    false
}

fn legacy_state_present(path: &Path) -> Result<bool, CodexSwitchError> {
    switch_path_entry_present(path, "inspect legacy switch state at")
}

fn current_journal_path_entry(paths: &SwitchPaths) -> Result<Option<PathBuf>, CodexSwitchError> {
    if switch_path_entry_present(paths.state.as_path(), "inspect scoped switch state at")? {
        return Ok(Some(paths.state.clone()));
    }
    Ok(read_matching_unscoped_journal_snapshot(paths)?
        .is_some()
        .then(|| paths.unscoped_state.clone()))
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

fn read_legacy_switch_state(
    path: &Path,
) -> Result<(ConfigSnapshot, LegacySwitchState), CodexSwitchError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CodexSwitchError::LegacySwitchState {
                path: path.to_path_buf(),
            });
        }
        Err(source) => return Err(io_error("read", path, source)),
    };
    let text = String::from_utf8(bytes).map_err(|error| CodexSwitchError::InvalidState {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let snapshot = ConfigSnapshot::from_text(true, text);
    validate_config_topology(path, true)?;
    let state = serde_json::from_str::<LegacySwitchState>(&snapshot.text).map_err(|error| {
        CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: error.to_string(),
        }
    })?;
    if !matches!(state.version, 1 | 2) {
        return Err(CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: format!(
                "unsupported legacy state version {}; expected 1 or 2",
                state.version
            ),
        });
    }
    if state
        .original_codex_proxy
        .as_ref()
        .is_some_and(|value| !value.is_table())
    {
        return Err(CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: "legacy original_codex_proxy must be a TOML table".to_string(),
        });
    }
    validate_legacy_auth_state(path, &state)?;
    validate_legacy_state_contract(path, &state)?;
    Ok((snapshot, state))
}

fn validate_legacy_auth_state(
    path: &Path,
    state: &LegacySwitchState,
) -> Result<(), CodexSwitchError> {
    match state.patched_auth_json.as_deref() {
        None if state.original_auth_json_absent || state.original_auth_json.is_some() => {
            Err(CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: "legacy auth recovery fields require patched_auth_json".to_string(),
            })
        }
        None => Ok(()),
        Some(_) if state.original_auth_json_absent && state.original_auth_json.is_some() => {
            Err(CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: "legacy auth state cannot be both absent and present".to_string(),
            })
        }
        Some(_) if !state.original_auth_json_absent && state.original_auth_json.is_none() => {
            Err(CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: "legacy auth state is missing its original JSON".to_string(),
            })
        }
        Some(_) => Ok(()),
    }
}

fn validate_legacy_state_contract(
    path: &Path,
    state: &LegacySwitchState,
) -> Result<(), CodexSwitchError> {
    let invalid = |reason: &str| CodexSwitchError::InvalidState {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    if state.original_config_absent
        && (state.original_model_provider.is_some()
            || state.original_codex_proxy.is_some()
            || state.had_model_providers)
    {
        return Err(invalid(
            "an absent original config cannot contain saved model provider state",
        ));
    }
    if !state.had_model_providers && state.original_codex_proxy.is_some() {
        return Err(invalid(
            "legacy original_codex_proxy requires had_model_providers",
        ));
    }

    let mode = state.patch_mode();
    if matches!(
        state.compaction,
        LegacyCodexCompactionStrategy::RemoteV1 | LegacyCodexCompactionStrategy::RemoteV2
    ) && !mode.uses_official_identity()
    {
        return Err(invalid(
            "remote compaction requires an official-relay or official-imagegen patch mode",
        ));
    }
    if state.responses_websocket && !state.uses_official_identity() {
        return Err(invalid(
            "responses_websocket requires an official OpenAI provider identity",
        ));
    }
    if mode.patches_auth() != state.patched_auth_json.is_some() {
        return Err(invalid(
            "legacy patch_mode and auth recovery fields do not describe the same completed patch",
        ));
    }
    Ok(())
}

fn legacy_recovery_required(path: &Path, reason: impl Into<String>) -> CodexSwitchError {
    CodexSwitchError::LegacyRecoveryRequired {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn legacy_base_url_matches_patch_shape(base_url: &str) -> bool {
    let normalized = base_url.trim().trim_end_matches('/');
    normalized == base_url
        && !normalized.is_empty()
        && Url::parse(normalized).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

fn expected_legacy_helper_stanza(
    legacy_path: &Path,
    state: &LegacySwitchState,
    current: &TomlValue,
) -> Result<TomlValue, CodexSwitchError> {
    let current_table = current.as_table().ok_or_else(|| {
        legacy_recovery_required(
            legacy_path,
            "model_providers.codex_proxy is no longer a TOML table",
        )
    })?;
    let base_url = current_table
        .get("base_url")
        .and_then(TomlValue::as_str)
        .filter(|base_url| legacy_base_url_matches_patch_shape(base_url))
        .ok_or_else(|| {
            legacy_recovery_required(
                legacy_path,
                "model_providers.codex_proxy.base_url no longer matches a v0.20.3 switch target",
            )
        })?;
    let mut expected = state
        .original_codex_proxy
        .as_ref()
        .and_then(TomlValue::as_table)
        .cloned()
        .unwrap_or_default();
    expected.insert(
        "name".to_string(),
        TomlValue::String(if state.uses_official_identity() {
            "OpenAI".to_string()
        } else {
            COMPATIBLE_PROVIDER_NAME.to_string()
        }),
    );
    expected.insert(
        "base_url".to_string(),
        TomlValue::String(base_url.to_string()),
    );
    expected.insert(
        "wire_api".to_string(),
        TomlValue::String("responses".to_string()),
    );
    expected
        .entry("request_max_retries".to_string())
        .or_insert(TomlValue::Integer(0));

    if state.patch_mode() == LegacyCodexPatchMode::ChatGptBridge {
        expected.insert("requires_openai_auth".to_string(), TomlValue::Boolean(true));
    } else {
        expected.remove("requires_openai_auth");
    }
    match state.patch_mode() {
        LegacyCodexPatchMode::Default | LegacyCodexPatchMode::ImagegenBridge => {
            expected.remove("supports_websockets");
        }
        LegacyCodexPatchMode::ChatGptBridge => {
            expected.insert("supports_websockets".to_string(), TomlValue::Boolean(false));
        }
        LegacyCodexPatchMode::OfficialRelay | LegacyCodexPatchMode::OfficialImagegen => {
            expected.insert(
                "supports_websockets".to_string(),
                TomlValue::Boolean(state.responses_websocket),
            );
        }
    }
    Ok(TomlValue::Table(expected))
}

fn legacy_config_restore_edit(
    path: &Path,
    legacy_path: &Path,
    current: &ConfigSnapshot,
    state: &LegacySwitchState,
) -> Result<Option<ConfigEdit>, CodexSwitchError> {
    if !current.present {
        // v0.20.3 preserved an external config deletion while still restoring auth and
        // clearing its switch state. Treat absence as an intentional current projection;
        // the snapshot CAS prevents a concurrently recreated file from being ignored.
        return Ok(None);
    }
    let inspection = inspect_config(path, current.text.as_str())?;
    let selector_is_original = inspection.model_provider == state.original_model_provider;
    let selector_matches_applied = inspection.model_provider.as_deref() == Some(PROVIDER_ID);
    let selector_is_owned = selector_matches_applied && !selector_is_original;
    let stanza_is_original = inspection.helper_stanza == state.original_codex_proxy;
    let stanza_matches_applied = if let Some(stanza) = inspection.helper_stanza.as_ref()
        && (!stanza_is_original || selector_is_owned)
    {
        expected_legacy_helper_stanza(legacy_path, state, stanza)? == *stanza
    } else {
        false
    };
    let stanza_is_owned = stanza_matches_applied && !stanza_is_original;
    if !stanza_is_original && !stanza_is_owned {
        return Err(legacy_recovery_required(
            legacy_path,
            "model_providers.codex_proxy contains edits that cannot be attributed to the v0.20.3 switch patch",
        ));
    }
    let selector_owned_stanza_original =
        selector_is_owned && stanza_is_original && !stanza_matches_applied;
    let selector_original_stanza_owned =
        selector_is_original && !selector_matches_applied && stanza_is_owned;
    if selector_owned_stanza_original || selector_original_stanza_owned {
        return Err(legacy_recovery_required(
            legacy_path,
            "model_provider and model_providers.codex_proxy form a hybrid original/helper projection that cannot be attributed to an atomic v0.20.3 switch write",
        ));
    }

    let mut document = editable_document(path, current.text.as_str())?;
    let root = document.as_table_mut();
    if selector_is_owned {
        if let Some(original) = state.original_model_provider.as_deref() {
            set_string_preserving_decor(root, "model_provider", original);
        } else {
            root.remove("model_provider");
        }
    }

    let remove_model_providers =
        if let Some(providers) = root.get_mut("model_providers").and_then(Item::as_table_mut) {
            if stanza_is_owned {
                if let Some(original) = state.original_codex_proxy.as_ref() {
                    providers.insert(PROVIDER_ID, editable_item_from_toml_value(original, path)?);
                } else {
                    providers.remove(PROVIDER_ID);
                }
            }
            !state.had_model_providers && providers.is_empty()
        } else {
            false
        };
    if remove_model_providers {
        root.remove("model_providers");
    }

    let changed = selector_is_owned || stanza_is_owned || remove_model_providers;
    let edit = if state.original_config_absent && root.is_empty() {
        ConfigEdit::Remove
    } else {
        ConfigEdit::Write(document.to_string())
    };
    let unchanged = match &edit {
        ConfigEdit::Write(text) => text == &current.text,
        ConfigEdit::Remove => !current.present,
    };
    Ok((changed || !unchanged).then_some(edit))
}

fn json_text_semantically_matches(current: &str, expected: &str) -> bool {
    if current == expected {
        return true;
    }
    match (
        serde_json::from_str::<serde_json::Value>(current),
        serde_json::from_str::<serde_json::Value>(expected),
    ) {
        (Ok(current), Ok(expected)) => current == expected,
        _ => false,
    }
}

fn legacy_auth_restore_edit(
    legacy_path: &Path,
    current: &ConfigSnapshot,
    state: &LegacySwitchState,
) -> Result<Option<ConfigEdit>, CodexSwitchError> {
    let Some(patched) = state.patched_auth_json.as_deref() else {
        return Ok(None);
    };
    if current.present && json_text_semantically_matches(&current.text, patched) {
        return Ok(if state.original_auth_json_absent {
            Some(ConfigEdit::Remove)
        } else {
            state
                .original_auth_json
                .as_ref()
                .map(|original| ConfigEdit::Write(original.clone()))
        });
    }
    let already_restored = if state.original_auth_json_absent {
        !current.present
    } else {
        current.present
            && state
                .original_auth_json
                .as_deref()
                .is_some_and(|original| json_text_semantically_matches(&current.text, original))
    };
    if already_restored {
        Ok(None)
    } else {
        Err(legacy_recovery_required(
            legacy_path,
            "auth.json no longer matches either the v0.20.3 helper patch or its saved original",
        ))
    }
}

fn apply_snapshot_edit_if_needed(
    path: &Path,
    legacy_path: &Path,
    current: &ConfigSnapshot,
    edit: Option<ConfigEdit>,
) -> Result<(), CodexSwitchError> {
    match edit {
        Some(edit) => write_snapshot_edit(path, legacy_path, edit, current),
        None => verify_legacy_snapshot_before_commit(path, legacy_path, current),
    }
}

fn recover_legacy_switch_state(
    paths: &SwitchPaths,
    failpoint: ApplyFailpoint,
) -> Result<(), CodexSwitchError> {
    recover_legacy_switch_state_with_before_remove(paths, failpoint, || Ok(()))
}

fn recover_legacy_switch_state_with_before_remove(
    paths: &SwitchPaths,
    failpoint: ApplyFailpoint,
    before_remove: impl FnOnce() -> Result<(), CodexSwitchError>,
) -> Result<(), CodexSwitchError> {
    let (legacy_snapshot, state) = read_legacy_switch_state(paths.legacy_state.as_path())?;

    let config = read_config_snapshot(paths.config.as_path())?;
    validate_config_topology(paths.config.as_path(), config.present)?;
    let config_edit = legacy_config_restore_edit(
        paths.config.as_path(),
        paths.legacy_state.as_path(),
        &config,
        &state,
    )?;

    let auth_plan = if state.patched_auth_json.is_some() {
        let auth = read_config_snapshot(paths.auth.as_path())?;
        validate_config_topology(paths.auth.as_path(), auth.present)?;
        let edit = legacy_auth_restore_edit(paths.legacy_state.as_path(), &auth, &state)?;
        Some((auth, edit))
    } else {
        None
    };

    apply_snapshot_edit_if_needed(
        paths.config.as_path(),
        paths.legacy_state.as_path(),
        &config,
        config_edit,
    )?;
    fail_if_requested(failpoint, ApplyFailpoint::AfterLegacyConfigRestore)?;

    if let Some((auth, auth_edit)) = auth_plan {
        apply_snapshot_edit_if_needed(
            paths.auth.as_path(),
            paths.legacy_state.as_path(),
            &auth,
            auth_edit,
        )?;
        fail_if_requested(failpoint, ApplyFailpoint::AfterLegacyAuthRestore)?;
    }

    let final_config = read_config_snapshot(paths.config.as_path())?;
    validate_config_topology(paths.config.as_path(), final_config.present)?;
    if legacy_config_restore_edit(
        paths.config.as_path(),
        paths.legacy_state.as_path(),
        &final_config,
        &state,
    )?
    .is_some()
    {
        return Err(legacy_recovery_required(
            paths.legacy_state.as_path(),
            "Codex config did not reach its recoverable original projection",
        ));
    }
    let final_auth = if state.patched_auth_json.is_some() {
        let final_auth = read_config_snapshot(paths.auth.as_path())?;
        validate_config_topology(paths.auth.as_path(), final_auth.present)?;
        if legacy_auth_restore_edit(paths.legacy_state.as_path(), &final_auth, &state)?.is_some() {
            return Err(legacy_recovery_required(
                paths.legacy_state.as_path(),
                "auth.json did not reach its saved original state",
            ));
        }
        Some(final_auth)
    } else {
        None
    };

    before_remove()?;
    verify_legacy_snapshot_before_commit(
        paths.config.as_path(),
        paths.legacy_state.as_path(),
        &final_config,
    )?;
    if let Some(final_auth) = final_auth.as_ref() {
        verify_legacy_snapshot_before_commit(
            paths.auth.as_path(),
            paths.legacy_state.as_path(),
            final_auth,
        )?;
    }
    verify_legacy_snapshot_before_commit(
        paths.legacy_state.as_path(),
        paths.legacy_state.as_path(),
        &legacy_snapshot,
    )?;
    remove_file_durable(paths.legacy_state.as_path())
}

fn legacy_switch_status(paths: &SwitchPaths) -> Result<CodexSwitchStatus, CodexSwitchError> {
    let current_state_path = current_journal_path_entry(paths)?;
    let config = read_config_snapshot(paths.config.as_path())
        .and_then(|current| inspect_config(paths.config.as_path(), current.text.as_str()))
        .ok();
    let enabled = config.as_ref().is_some_and(|config| {
        config.model_provider.as_deref() == Some(PROVIDER_ID) && config.helper_stanza.is_some()
    });
    let base_url = config.and_then(|config| config.helper_base_url);
    let mut recovery_reason = legacy_switch_error(paths).to_string();
    if let Some(current_state_path) = current_state_path.as_ref() {
        recovery_reason.push_str(
            format!(
                ". A current switch journal also exists at {:?}; neither journal was modified",
                current_state_path
            )
            .as_str(),
        );
    }

    Ok(CodexSwitchStatus {
        phase: CodexSwitchPhase::RecoveryRequired,
        enabled,
        managed: current_state_path.is_some(),
        base_url,
        client_patch: None,
        recovery_reason: Some(recovery_reason),
        config_path: paths.config.clone(),
        state_path: paths.legacy_state.clone(),
    })
}

fn status_after_legacy_recheck(
    paths: &SwitchPaths,
    current: &ConfigSnapshot,
    journal: Option<&SwitchJournal>,
    journal_state_path: Option<&Path>,
) -> Result<CodexSwitchStatus, CodexSwitchError> {
    if legacy_state_present(paths.legacy_state.as_path())? {
        return legacy_switch_status(paths);
    }
    status_from_snapshot(paths, current, journal, journal_state_path)
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
    expected: &ConfigSnapshot,
) -> Result<(), CodexSwitchError> {
    reject_legacy_switch_state(paths)?;
    write_config_edit(paths.config.as_path(), edit, journal, expected)
}

fn write_current_auth_edit(
    paths: &SwitchPaths,
    edit: ConfigEdit,
    switch_journal: &SwitchJournal,
    auth_journal: &AuthJournal,
    expected: ExpectedAuthState,
) -> Result<(), CodexSwitchError> {
    reject_legacy_switch_state(paths)?;
    write_auth_edit(
        paths,
        paths.auth.as_path(),
        edit,
        switch_journal,
        auth_journal,
        expected,
    )
}

fn managed_helper_stanza_backup_name(name: &str) -> bool {
    name.strip_prefix(HELPER_STANZA_BACKUP_FILE_PREFIX)
        .and_then(|suffix| suffix.strip_suffix(HELPER_STANZA_BACKUP_FILE_SUFFIX))
        .is_some_and(managed_switch_artifact_uuid)
}

fn helper_stanza_backup_path(
    paths: &SwitchPaths,
    journal: &HelperStanzaJournal,
) -> Result<Option<PathBuf>, CodexSwitchError> {
    match (
        journal.original_present,
        journal.backup_file_name.as_deref(),
    ) {
        (false, None) => Ok(None),
        (true, Some(name)) if managed_helper_stanza_backup_name(name) => {
            let parent = paths
                .state
                .parent()
                .ok_or_else(|| CodexSwitchError::InvalidState {
                    path: paths.state.clone(),
                    reason: "switch state path has no parent directory".to_string(),
                })?;
            Ok(Some(parent.join(name)))
        }
        (true, None) => Err(CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: "helper stanza recovery metadata is missing its backup file name".to_string(),
        }),
        (false, Some(_)) => Err(CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason:
                "helper stanza recovery metadata records a backup for an absent original stanza"
                    .to_string(),
        }),
        (true, Some(_)) => Err(CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: "helper stanza recovery metadata contains an invalid backup file name"
                .to_string(),
        }),
    }
}

fn parse_helper_stanza_repr(path: &Path, repr: &str) -> Result<TomlValue, CodexSwitchError> {
    repr.parse::<DocumentMut>()
        .map_err(|error| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: sanitized_toml_parse_reason(error.message(), error.span(), repr),
        })?;
    toml::from_str::<TomlValue>(repr)
        .ok()
        .and_then(|value| {
            value
                .as_table()?
                .get("model_providers")?
                .as_table()?
                .get(PROVIDER_ID)
                .cloned()
        })
        .ok_or_else(|| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: "original helper stanza representation is not a TOML table".to_string(),
        })
}

fn validate_legacy_helper_stanza_fields(
    path: &Path,
    journal: &SwitchJournal,
) -> Result<(), CodexSwitchError> {
    match (
        journal.original_helper_stanza.as_ref(),
        journal.original_helper_stanza_repr.as_deref(),
    ) {
        (Some(expected), Some(repr)) => {
            if &parse_helper_stanza_repr(path, repr)? == expected {
                Ok(())
            } else {
                Err(CodexSwitchError::InvalidState {
                    path: path.to_path_buf(),
                    reason: "original helper stanza representation does not match its value"
                        .to_string(),
                })
            }
        }
        (None, None) => Ok(()),
        _ => Err(CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: "original helper stanza value and representation must agree".to_string(),
        }),
    }
}

fn prepare_helper_stanza_journal(
    path: &Path,
    original: Option<&TomlValue>,
    repr: Option<&str>,
    backup_id: Option<&str>,
) -> Result<HelperStanzaJournal, CodexSwitchError> {
    match (original, repr) {
        (Some(expected), Some(repr)) => {
            if &parse_helper_stanza_repr(path, repr)? != expected {
                return Err(CodexSwitchError::InvalidState {
                    path: path.to_path_buf(),
                    reason: "original helper stanza representation does not match its value"
                        .to_string(),
                });
            }
            let backup_id = backup_id
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| Uuid::new_v4().to_string());
            if !managed_switch_artifact_uuid(backup_id.as_str()) {
                return Err(CodexSwitchError::InvalidState {
                    path: path.to_path_buf(),
                    reason: "helper stanza backup identifier is invalid".to_string(),
                });
            }
            Ok(HelperStanzaJournal {
                original_present: true,
                original_fingerprint: fingerprint(repr.as_bytes()),
                backup_file_name: Some(format!(
                    "{HELPER_STANZA_BACKUP_FILE_PREFIX}{backup_id}{HELPER_STANZA_BACKUP_FILE_SUFFIX}"
                )),
            })
        }
        (None, None) => {
            if backup_id.is_some() {
                return Err(CodexSwitchError::InvalidState {
                    path: path.to_path_buf(),
                    reason: "absent original helper stanza cannot reserve a backup identifier"
                        .to_string(),
                });
            }
            Ok(HelperStanzaJournal {
                original_present: false,
                original_fingerprint: fingerprint(&[]),
                backup_file_name: None,
            })
        }
        _ => Err(CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: "original helper stanza value and representation must agree".to_string(),
        }),
    }
}

fn load_helper_stanza_backup_read_only(
    paths: &SwitchPaths,
    journal: &mut SwitchJournal,
) -> Result<(), CodexSwitchError> {
    let Some(metadata) = journal.helper_stanza_backup.clone() else {
        return validate_legacy_helper_stanza_fields(paths.state.as_path(), journal);
    };
    let Some(path) = helper_stanza_backup_path(paths, &metadata)? else {
        journal.original_helper_stanza = None;
        journal.original_helper_stanza_repr = None;
        return Ok(());
    };
    let backup = read_config_snapshot(path.as_path())?;
    validate_config_topology(path.as_path(), backup.present)?;
    if !backup.present {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "the secure Codex helper stanza backup is missing".to_string(),
        });
    }
    if backup.fingerprint != metadata.original_fingerprint {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "the secure Codex helper stanza backup does not match its recorded fingerprint"
                .to_string(),
        });
    }
    let semantic = parse_helper_stanza_repr(path.as_path(), backup.text.as_str())?;
    journal.original_helper_stanza = Some(semantic);
    journal.original_helper_stanza_repr = Some(backup.text);
    Ok(())
}

fn current_original_helper_stanza_repr(
    paths: &SwitchPaths,
    current: &ConfigSnapshot,
    metadata: &HelperStanzaJournal,
) -> Result<Option<String>, CodexSwitchError> {
    if !current.present || !metadata.original_present {
        return Ok(None);
    }
    let document = editable_document(paths.config.as_path(), current.text.as_str())?;
    Ok(
        helper_stanza_repr_from_document(paths.config.as_path(), &document)?
            .filter(|repr| fingerprint(repr.as_bytes()) == metadata.original_fingerprint),
    )
}

fn load_helper_stanza_backup_or_restored_config(
    paths: &SwitchPaths,
    journal: &mut SwitchJournal,
    current: &ConfigSnapshot,
) -> Result<(), CodexSwitchError> {
    match load_helper_stanza_backup_read_only(paths, journal) {
        Ok(()) => Ok(()),
        Err(CodexSwitchError::RecoveryRequired { .. })
            if journal.phase == JournalPhase::Restored
                && journal.operation == JournalOperation::Off =>
        {
            let metadata = journal.helper_stanza_backup.clone().ok_or_else(|| {
                CodexSwitchError::InvalidState {
                    path: paths.state.clone(),
                    reason: "helper stanza recovery metadata is missing".to_string(),
                }
            })?;
            let Some(repr) = current_original_helper_stanza_repr(paths, current, &metadata)? else {
                return Err(CodexSwitchError::RecoveryRequired {
                    reason: "the secure Codex helper stanza backup is missing or changed after the original stanza was replaced"
                        .to_string(),
                });
            };
            let semantic = parse_helper_stanza_repr(paths.config.as_path(), repr.as_str())?;
            journal.original_helper_stanza = Some(semantic);
            journal.original_helper_stanza_repr = Some(repr);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn ensure_helper_stanza_backup(
    paths: &SwitchPaths,
    journal: &mut SwitchJournal,
    current: &ConfigSnapshot,
) -> Result<(), CodexSwitchError> {
    let migrates_version = journal.version == LEGACY_STATE_VERSION;
    if journal.helper_stanza_backup.is_none() {
        if !migrates_version {
            return Err(CodexSwitchError::InvalidState {
                path: paths.state.clone(),
                reason: "version 2 switch state is missing helper stanza recovery metadata"
                    .to_string(),
            });
        }
        validate_legacy_helper_stanza_fields(paths.state.as_path(), journal)?;
        let legacy_repr = journal.original_helper_stanza_repr.clone();
        let deterministic_backup_id = legacy_repr.as_ref().map(|_| journal.operation_id.as_str());
        journal.helper_stanza_backup = Some(prepare_helper_stanza_journal(
            paths.state.as_path(),
            journal.original_helper_stanza.as_ref(),
            journal.original_helper_stanza_repr.as_deref(),
            deterministic_backup_id,
        )?);
        if let Some(repr) = legacy_repr {
            let metadata = journal.helper_stanza_backup.as_ref().ok_or_else(|| {
                CodexSwitchError::InvalidState {
                    path: paths.state.clone(),
                    reason: "helper stanza recovery metadata is missing".to_string(),
                }
            })?;
            let path = helper_stanza_backup_path(paths, metadata)?.ok_or_else(|| {
                CodexSwitchError::InvalidState {
                    path: paths.state.clone(),
                    reason: "present original helper stanza is missing its backup path".to_string(),
                }
            })?;
            atomic_write_text(path.as_path(), repr.as_str(), FilePermissions::Secure, None)?;
        }
        load_helper_stanza_backup_read_only(paths, journal)?;
        journal.version = STATE_VERSION;
        write_current_journal(paths, journal)?;
        return Ok(());
    }

    match load_helper_stanza_backup_read_only(paths, journal) {
        Ok(()) => {
            if migrates_version {
                journal.version = STATE_VERSION;
                write_current_journal(paths, journal)?;
            }
            return Ok(());
        }
        Err(CodexSwitchError::RecoveryRequired { .. }) => {}
        Err(error) => return Err(error),
    }

    let metadata =
        journal
            .helper_stanza_backup
            .clone()
            .ok_or_else(|| CodexSwitchError::InvalidState {
                path: paths.state.clone(),
                reason: "helper stanza recovery metadata is missing".to_string(),
            })?;
    let Some(repr) = current_original_helper_stanza_repr(paths, current, &metadata)? else {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "the secure Codex helper stanza backup is missing or changed after the original stanza was replaced"
                .to_string(),
        });
    };
    let path = helper_stanza_backup_path(paths, &metadata)?.ok_or_else(|| {
        CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: "present original helper stanza is missing its backup path".to_string(),
        }
    })?;
    atomic_write_text(path.as_path(), repr.as_str(), FilePermissions::Secure, None)?;
    load_helper_stanza_backup_read_only(paths, journal)?;
    if migrates_version {
        journal.version = STATE_VERSION;
        write_current_journal(paths, journal)?;
    }
    Ok(())
}

fn remove_helper_stanza_backup(
    paths: &SwitchPaths,
    journal: &HelperStanzaJournal,
) -> Result<(), CodexSwitchError> {
    if let Some(path) = helper_stanza_backup_path(paths, journal)? {
        remove_file_durable(path.as_path())?;
    }
    Ok(())
}

fn managed_auth_backup_name(name: &str) -> bool {
    name.strip_prefix(AUTH_BACKUP_FILE_PREFIX)
        .and_then(|suffix| suffix.strip_suffix(AUTH_BACKUP_FILE_SUFFIX))
        .is_some_and(managed_switch_artifact_uuid)
}

fn auth_backup_path(
    paths: &SwitchPaths,
    journal: &AuthJournal,
) -> Result<Option<PathBuf>, CodexSwitchError> {
    match (
        journal.original_present,
        journal.backup_file_name.as_deref(),
    ) {
        (false, None) => Ok(None),
        (true, Some(name)) if managed_auth_backup_name(name) => {
            let parent = paths
                .state
                .parent()
                .ok_or_else(|| CodexSwitchError::InvalidState {
                    path: paths.state.clone(),
                    reason: "switch state path has no parent directory".to_string(),
                })?;
            Ok(Some(parent.join(name)))
        }
        (true, None) => Err(CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: "auth recovery metadata is missing its backup file name".to_string(),
        }),
        (false, Some(_)) => Err(CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: "auth recovery metadata records a backup for an absent original file"
                .to_string(),
        }),
        (true, Some(_)) => Err(CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: "auth recovery metadata contains an invalid backup file name".to_string(),
        }),
    }
}

fn read_auth_backup(
    paths: &SwitchPaths,
    journal: &AuthJournal,
) -> Result<Option<ConfigSnapshot>, CodexSwitchError> {
    let Some(path) = auth_backup_path(paths, journal)? else {
        return Ok(None);
    };
    let snapshot = read_config_snapshot(path.as_path())?;
    validate_config_topology(path.as_path(), snapshot.present)?;
    Ok(Some(snapshot))
}

fn ensure_auth_backup(
    paths: &SwitchPaths,
    journal: &AuthJournal,
    current_auth: &ConfigSnapshot,
) -> Result<(), CodexSwitchError> {
    let Some(path) = auth_backup_path(paths, journal)? else {
        return Ok(());
    };
    let backup = read_config_snapshot(path.as_path())?;
    if backup.present {
        validate_config_topology(path.as_path(), true)?;
        if backup.fingerprint == journal.original_fingerprint {
            return Ok(());
        }
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "the secure Codex auth backup does not match its recorded fingerprint"
                .to_string(),
        });
    }
    if !current_auth.matches_original_auth(journal) {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "the secure Codex auth backup is missing after auth.json changed".to_string(),
        });
    }
    atomic_write_text(
        path.as_path(),
        current_auth.text.as_str(),
        FilePermissions::Secure,
        None,
    )?;
    let backup = read_config_snapshot(path.as_path())?;
    validate_config_topology(path.as_path(), backup.present)?;
    if backup.present && backup.fingerprint == journal.original_fingerprint {
        Ok(())
    } else {
        Err(CodexSwitchError::RecoveryRequired {
            reason: "the secure Codex auth backup was not committed with the recorded fingerprint"
                .to_string(),
        })
    }
}

fn render_recorded_auth_facade(
    paths: &SwitchPaths,
    switch_journal: &SwitchJournal,
    auth_journal: &AuthJournal,
    current_auth: &ConfigSnapshot,
) -> Result<String, CodexSwitchError> {
    let strategy = switch_journal
        .recorded_auth_client_patch()
        .compile()
        .map_err(|error| CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: format!("invalid recorded client patch: {error}"),
        })?
        .auth_facade;
    if strategy == CodexAuthFacadeStrategy::Preserve {
        return Err(CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: "auth recovery metadata exists for a client patch that preserves auth.json"
                .to_string(),
        });
    }

    let original = if auth_journal.original_present {
        if current_auth.matches_original_auth(auth_journal) {
            current_auth.text.as_str()
        } else {
            let backup = read_auth_backup(paths, auth_journal)?.ok_or_else(|| {
                CodexSwitchError::RecoveryRequired {
                    reason: "the secure Codex auth backup is missing".to_string(),
                }
            })?;
            if backup.fingerprint != auth_journal.original_fingerprint {
                return Err(CodexSwitchError::RecoveryRequired {
                    reason: "the secure Codex auth backup does not match its recorded fingerprint"
                        .to_string(),
                });
            }
            return render_recorded_auth_facade_from_text(
                paths,
                strategy,
                Some(backup.text.as_str()),
                auth_journal,
            );
        }
    } else {
        ""
    };
    render_recorded_auth_facade_from_text(
        paths,
        strategy,
        auth_journal.original_present.then_some(original),
        auth_journal,
    )
}

fn render_recorded_auth_facade_from_text(
    paths: &SwitchPaths,
    strategy: CodexAuthFacadeStrategy,
    original: Option<&str>,
    journal: &AuthJournal,
) -> Result<String, CodexSwitchError> {
    let rendered = crate::codex_auth_facade::render_auth_facade(strategy, original)
        .map_err(|error| CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: format!("recorded auth facade can no longer be reproduced: {error}"),
        })?
        .ok_or_else(|| CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: "recorded auth facade strategy produced no projection".to_string(),
        })?;
    if fingerprint(rendered.as_bytes()) != journal.applied_fingerprint {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "recorded auth facade no longer produces the planned fingerprint".to_string(),
        });
    }
    Ok(rendered)
}

fn auth_snapshot_matches_recorded_states(
    paths: &SwitchPaths,
    switch_journal: &SwitchJournal,
    auth_journal: &AuthJournal,
    current: &ConfigSnapshot,
) -> Result<(bool, bool), CodexSwitchError> {
    let matches_original = current.matches_original_auth(auth_journal);
    let matches_applied = if current.matches_applied_auth(auth_journal) {
        true
    } else if current.present {
        let applied = render_recorded_auth_facade(paths, switch_journal, auth_journal, current)?;
        json_text_semantically_matches(current.text.as_str(), applied.as_str())
    } else {
        false
    };
    Ok((matches_original, matches_applied))
}

fn journal_patches_auth(
    paths: &SwitchPaths,
    journal: &SwitchJournal,
) -> Result<bool, CodexSwitchError> {
    let strategy = journal
        .recovery_client_patch()
        .compile()
        .map_err(|error| CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: format!("invalid recorded client patch: {error}"),
        })?
        .auth_facade;
    Ok(strategy != CodexAuthFacadeStrategy::Preserve)
}

fn restore_recorded_auth_original(
    paths: &SwitchPaths,
    switch_journal: &SwitchJournal,
    auth_journal: &AuthJournal,
    current_auth: &ConfigSnapshot,
) -> Result<bool, CodexSwitchError> {
    let (matches_original, matches_applied) =
        auth_snapshot_matches_recorded_states(paths, switch_journal, auth_journal, current_auth)?;
    if !matches_original && !matches_applied {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "Codex auth.json matches neither the saved original nor retained facade"
                .to_string(),
        });
    }
    ensure_auth_backup(paths, auth_journal, current_auth)?;
    if matches_original {
        return Ok(false);
    }

    let edit = if auth_journal.original_present {
        let backup = read_auth_backup(paths, auth_journal)?.ok_or_else(|| {
            CodexSwitchError::RecoveryRequired {
                reason: "the secure Codex auth backup is missing".to_string(),
            }
        })?;
        if backup.fingerprint != auth_journal.original_fingerprint {
            return Err(CodexSwitchError::RecoveryRequired {
                reason: "the secure Codex auth backup does not match its recorded fingerprint"
                    .to_string(),
            });
        }
        ConfigEdit::Write(backup.text)
    } else {
        ConfigEdit::Remove
    };
    write_current_auth_edit(
        paths,
        edit,
        switch_journal,
        auth_journal,
        ExpectedAuthState::Applied,
    )?;
    let restored = read_config_snapshot(paths.auth.as_path())?;
    if restored.matches_original_auth(auth_journal) {
        Ok(true)
    } else {
        Err(CodexSwitchError::RecoveryRequired {
            reason: "Codex auth.json changed before the retained original was restored".to_string(),
        })
    }
}

fn apply_on(
    paths: &SwitchPaths,
    current: ConfigSnapshot,
    journal: Option<SwitchJournal>,
    target: ValidatedCodexBaseUrl,
    client_patch: CodexClientPatchConfig,
    failpoint: ApplyFailpoint,
    requested_auto_restore_generation: Option<String>,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    match journal {
        None => begin_on(
            paths,
            current,
            target,
            client_patch,
            failpoint,
            requested_auto_restore_generation,
        ),
        Some(journal) => match journal.phase {
            JournalPhase::RecoveryRequired => Err(CodexSwitchError::RecoveryRequired {
                reason: journal
                    .recovery_reason
                    .unwrap_or_else(|| "stored switch state requires reconciliation".to_string()),
            }),
            JournalPhase::Applied => {
                let mut repaired_retained_auth = false;
                let config_matches =
                    managed_config_matches(paths.config.as_path(), &current, &journal)?;
                if !config_matches.applied {
                    return mark_recovery(
                        paths,
                        journal,
                        "helper-owned Codex fields changed after the switch was applied",
                    );
                }
                if let Some(auth_journal) = journal.auth_patch.clone() {
                    let auth = read_config_snapshot(paths.auth.as_path())?;
                    let (matches_original, matches_applied) =
                        match auth_snapshot_matches_recorded_states(
                            paths,
                            &journal,
                            &auth_journal,
                            &auth,
                        ) {
                            Ok(matches) => matches,
                            Err(CodexSwitchError::RecoveryRequired { reason }) => {
                                return mark_recovery(paths, journal, reason);
                            }
                            Err(error) => return Err(error),
                        };
                    if journal_patches_auth(paths, &journal)? {
                        if !matches_applied {
                            return mark_recovery(
                                paths,
                                journal,
                                "Codex auth.json changed after helper applied its client facade",
                            );
                        }
                        if let Err(error) = ensure_auth_backup(paths, &auth_journal, &auth) {
                            return match error {
                                CodexSwitchError::RecoveryRequired { reason } => {
                                    mark_recovery(paths, journal, reason)
                                }
                                error => Err(error),
                            };
                        }
                    } else {
                        repaired_retained_auth = match restore_recorded_auth_original(
                            paths,
                            &journal,
                            &auth_journal,
                            &auth,
                        ) {
                            Ok(repaired) => repaired,
                            Err(CodexSwitchError::RecoveryRequired { reason }) => {
                                return mark_recovery(paths, journal, reason);
                            }
                            Err(error) => return Err(error),
                        };
                        debug_assert!(matches_original || matches_applied);
                    }
                }
                ensure_target_matches(&journal, &target)?;
                if journal.records_complete_client_patch(client_patch) {
                    let mut journal = journal;
                    reconcile_auto_restore_generation(
                        paths,
                        &mut journal,
                        requested_auto_restore_generation.as_deref(),
                    )?;
                    outcome(
                        paths,
                        if repaired_retained_auth {
                            CodexSwitchChange::Recovered
                        } else {
                            CodexSwitchChange::Unchanged
                        },
                    )
                } else {
                    reapply_on_after_off(
                        paths,
                        current,
                        journal,
                        target,
                        client_patch,
                        failpoint,
                        requested_auto_restore_generation,
                    )
                }
            }
            JournalPhase::Prepared => {
                let config_matches =
                    managed_config_matches(paths.config.as_path(), &current, &journal)?;
                match journal.operation {
                    JournalOperation::On
                        if current.matches_original(&journal) || config_matches.applied =>
                    {
                        ensure_target_matches(&journal, &target)?;
                        if journal.records_complete_client_patch(client_patch) {
                            let mut journal = journal;
                            reconcile_auto_restore_generation(
                                paths,
                                &mut journal,
                                requested_auto_restore_generation.as_deref(),
                            )?;
                            resume_on(paths, journal, failpoint, None)
                        } else {
                            reapply_on_after_off(
                                paths,
                                current,
                                journal,
                                target,
                                client_patch,
                                failpoint,
                                requested_auto_restore_generation,
                            )
                        }
                    }
                    JournalOperation::Off if config_matches.original || config_matches.applied => {
                        ensure_target_matches(&journal, &target)?;
                        reapply_on_after_off(
                            paths,
                            current,
                            journal,
                            target,
                            client_patch,
                            failpoint,
                            requested_auto_restore_generation,
                        )
                    }
                    _ => mark_recovery(
                        paths,
                        journal,
                        "helper-owned Codex fields match neither the prepared original nor applied projection",
                    ),
                }
            }
            JournalPhase::Restored => {
                let config_matches =
                    managed_config_matches(paths.config.as_path(), &current, &journal)?;
                if !config_matches.original && !config_matches.applied {
                    return mark_recovery(
                        paths,
                        journal,
                        "helper-owned Codex fields no longer match the retained switch recovery point",
                    );
                }
                reapply_on_after_off(
                    paths,
                    current,
                    journal,
                    target,
                    client_patch,
                    failpoint,
                    requested_auto_restore_generation,
                )
            }
        },
    }
}

fn reconcile_auto_restore_generation(
    paths: &SwitchPaths,
    journal: &mut SwitchJournal,
    requested: Option<&str>,
) -> Result<(), CodexSwitchError> {
    let next = match requested {
        None => None,
        Some(generation) if journal.auto_restore_generation.is_some() => {
            Some(generation.to_string())
        }
        Some(_) => None,
    };
    if journal.auto_restore_generation != next {
        journal.auto_restore_generation = next;
        write_current_journal(paths, journal)?;
    }
    Ok(())
}

fn reapply_on_after_off(
    paths: &SwitchPaths,
    current: ConfigSnapshot,
    journal: SwitchJournal,
    target: ValidatedCodexBaseUrl,
    client_patch: CodexClientPatchConfig,
    failpoint: ApplyFailpoint,
    requested_auto_restore_generation: Option<String>,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    let retained_auth = journal.auth_patch.clone().map(|auth| RetainedAuthJournal {
        client_patch: journal.recorded_auth_client_patch(),
        auth,
    });
    let retained_helper_stanza_backup = journal.helper_stanza_backup.clone();
    preflight_reapply_after_off(paths, &current, &journal, &target, client_patch)?;
    apply_off(paths, current, Some(journal), failpoint)?;
    if let Some(helper_stanza_backup) = retained_helper_stanza_backup.as_ref() {
        remove_helper_stanza_backup(paths, helper_stanza_backup)?;
    }
    let restored = read_config_snapshot(paths.config.as_path())?;
    if let Some(retained_auth) = retained_auth.as_ref() {
        ensure_retained_auth_is_still_original(paths, &retained_auth.auth)?;
    }
    begin_on_with_retained_auth(
        paths,
        restored,
        target,
        client_patch,
        failpoint,
        requested_auto_restore_generation,
        retained_auth.as_ref(),
    )
}

fn ensure_retained_auth_is_still_original(
    paths: &SwitchPaths,
    auth_journal: &AuthJournal,
) -> Result<(), CodexSwitchError> {
    let current = read_config_snapshot(paths.auth.as_path())?;
    validate_config_topology(paths.auth.as_path(), current.present)?;
    if current.matches_original_auth(auth_journal) {
        Ok(())
    } else {
        Err(CodexSwitchError::RecoveryRequired {
            reason: "Codex auth.json changed between the retained switch-off recovery point and the new switch"
                .to_string(),
        })
    }
}

fn preflight_reapply_after_off(
    paths: &SwitchPaths,
    current: &ConfigSnapshot,
    journal: &SwitchJournal,
    target: &ValidatedCodexBaseUrl,
    client_patch: CodexClientPatchConfig,
) -> Result<(), CodexSwitchError> {
    let config_matches = managed_config_matches(paths.config.as_path(), current, journal)?;
    let restored = if config_matches.original {
        current.clone()
    } else if config_matches.applied {
        let edit = patch_off(paths.config.as_path(), current.text.as_str(), journal)?;
        if !managed_config_matches(paths.config.as_path(), &edit.snapshot(), journal)?.original {
            return Err(CodexSwitchError::RestoreFingerprintMismatch);
        }
        edit.into_snapshot()
    } else {
        return Err(CodexSwitchError::RecoveryRequired {
            reason:
                "helper-owned Codex fields cannot be preflighted from their recorded switch state"
                    .to_string(),
        });
    };
    let original = inspect_config(paths.config.as_path(), restored.text.as_str())?;
    reject_unowned_helper_config(&original)?;
    patch_on(
        paths.config.as_path(),
        restored.text.as_str(),
        target.as_str(),
        client_patch,
    )?;

    let compiled = client_patch.compile().map_err(invalid_client_patch_error)?;
    if compiled.auth_facade == CodexAuthFacadeStrategy::Preserve {
        return Ok(());
    }
    let original_auth = original_auth_snapshot_for_reapply(paths, journal)?;
    crate::codex_auth_facade::render_auth_facade(
        compiled.auth_facade,
        original_auth.present.then_some(original_auth.text.as_str()),
    )
    .map_err(|error| CodexSwitchError::InvalidAuth {
        path: paths.auth.clone(),
        reason: error.to_string(),
    })?
    .ok_or_else(|| CodexSwitchError::InvalidAuth {
        path: paths.auth.clone(),
        reason: "compiled auth facade strategy did not produce an auth projection".to_string(),
    })?;
    Ok(())
}

fn original_auth_snapshot_for_reapply(
    paths: &SwitchPaths,
    journal: &SwitchJournal,
) -> Result<ConfigSnapshot, CodexSwitchError> {
    let Some(auth_journal) = journal.auth_patch.as_ref() else {
        return read_config_snapshot(paths.auth.as_path());
    };
    if !auth_journal.original_present {
        return Ok(ConfigSnapshot::from_text(false, String::new()));
    }

    let current = read_config_snapshot(paths.auth.as_path())?;
    if current.matches_original_auth(auth_journal) {
        return Ok(current);
    }
    let backup = read_auth_backup(paths, auth_journal)?.ok_or_else(|| {
        CodexSwitchError::RecoveryRequired {
            reason: "the secure Codex auth backup is missing".to_string(),
        }
    })?;
    if backup.fingerprint != auth_journal.original_fingerprint {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "the secure Codex auth backup does not match its recorded fingerprint"
                .to_string(),
        });
    }
    Ok(backup)
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
        if let Some(auth) = journal.auth_patch.as_ref()
            && auth.auth_path_fingerprint != config_path_fingerprint(paths.auth.as_path())
        {
            return Err(CodexSwitchError::RecoveryRequired {
                reason: "switch state belongs to a different Codex auth.json path".to_string(),
            });
        }
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
    client_patch: CodexClientPatchConfig,
    failpoint: ApplyFailpoint,
    auto_restore_generation: Option<String>,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    begin_on_with_retained_auth(
        paths,
        current,
        target,
        client_patch,
        failpoint,
        auto_restore_generation,
        None,
    )
}

fn begin_on_with_retained_auth(
    paths: &SwitchPaths,
    current: ConfigSnapshot,
    target: ValidatedCodexBaseUrl,
    client_patch: CodexClientPatchConfig,
    failpoint: ApplyFailpoint,
    auto_restore_generation: Option<String>,
    retained_auth: Option<&RetainedAuthJournal>,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    validate_config_topology(paths.config.as_path(), current.present)?;
    let original = inspect_config(paths.config.as_path(), &current.text)?;
    reject_unowned_helper_config(&original)?;
    let compiled = client_patch.compile().map_err(invalid_client_patch_error)?;
    let prepared_auth = prepare_auth_journal(paths, compiled.auth_facade, retained_auth)?;

    let patch = patch_on(
        paths.config.as_path(),
        &current.text,
        target.as_str(),
        client_patch,
    )?;
    let helper_stanza_backup = prepare_helper_stanza_journal(
        paths.config.as_path(),
        original.helper_stanza.as_ref(),
        patch.original_helper_stanza_repr.as_deref(),
        None,
    )?;
    let applied_fingerprint = fingerprint(patch.text.as_bytes());
    let planned_destination = ConfigSnapshot::from_text(true, patch.text.clone());
    let pending_config = PendingConfigCommit::new(&current, &planned_destination);
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
        pending_config: Some(pending_config),
        auto_restore_generation,
        original_model_provider: original.model_provider,
        original_model_provider_repr: patch.original_model_provider_repr,
        original_helper_stanza: original.helper_stanza,
        original_helper_stanza_repr: patch.original_helper_stanza_repr,
        helper_stanza_backup: Some(helper_stanza_backup),
        original_model_providers_present: original.model_providers_present,
        target_base_url: target.0,
        client_patch,
        original_features_present: patch.original_features_present,
        original_remote_compaction_v2: patch.original_remote_compaction_v2,
        original_image_generation: patch.original_image_generation,
        auth_patch: prepared_auth.auth,
        auth_recovery_patch: prepared_auth.recovery_patch,
        recovery_reason: None,
    };
    write_current_journal(paths, &journal)?;
    resume_on(paths, journal, failpoint, Some(planned_write))
}

fn prepare_auth_journal(
    paths: &SwitchPaths,
    strategy: CodexAuthFacadeStrategy,
    retained_auth: Option<&RetainedAuthJournal>,
) -> Result<PreparedAuthJournal, CodexSwitchError> {
    if strategy == CodexAuthFacadeStrategy::Preserve && retained_auth.is_none() {
        return Ok(PreparedAuthJournal {
            auth: None,
            recovery_patch: None,
        });
    }
    let original = read_config_snapshot(paths.auth.as_path())?;
    validate_config_topology(paths.auth.as_path(), original.present)?;
    if let Some(retained_auth) = retained_auth
        && !original.matches_original_auth(&retained_auth.auth)
    {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "Codex auth.json no longer matches the retained switch-off recovery point"
                .to_string(),
        });
    }
    if strategy == CodexAuthFacadeStrategy::Preserve {
        let retained_auth = retained_auth.ok_or_else(|| CodexSwitchError::InvalidState {
            path: paths.state.clone(),
            reason: "preserved auth recovery metadata is missing".to_string(),
        })?;
        return Ok(PreparedAuthJournal {
            auth: Some(retained_auth.auth.clone()),
            recovery_patch: Some(retained_auth.client_patch),
        });
    }
    let applied = crate::codex_auth_facade::render_auth_facade(
        strategy,
        original.present.then_some(original.text.as_str()),
    )
    .map_err(|error| CodexSwitchError::InvalidAuth {
        path: paths.auth.clone(),
        reason: error.to_string(),
    })?
    .ok_or_else(|| CodexSwitchError::InvalidAuth {
        path: paths.auth.clone(),
        reason: "compiled auth facade strategy did not produce an auth projection".to_string(),
    })?;
    Ok(PreparedAuthJournal {
        auth: Some(AuthJournal {
            auth_path_fingerprint: config_path_fingerprint(paths.auth.as_path()),
            original_present: original.present,
            original_fingerprint: original.fingerprint,
            applied_fingerprint: fingerprint(applied.as_bytes()),
            backup_file_name: if original.present {
                retained_auth
                    .and_then(|auth| auth.auth.backup_file_name.clone())
                    .or_else(|| {
                        Some(format!(
                            "{AUTH_BACKUP_FILE_PREFIX}{}{AUTH_BACKUP_FILE_SUFFIX}",
                            Uuid::new_v4()
                        ))
                    })
            } else {
                None
            },
        }),
        recovery_patch: None,
    })
}

fn resume_on(
    paths: &SwitchPaths,
    mut journal: SwitchJournal,
    failpoint: ApplyFailpoint,
    planned_write: Option<PlannedOnWrite>,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    let current = read_config_snapshot(paths.config.as_path())?;
    if let Err(error) = ensure_helper_stanza_backup(paths, &mut journal, &current) {
        return match error {
            CodexSwitchError::RecoveryRequired { reason } => mark_recovery(paths, journal, reason),
            error => Err(error),
        };
    }
    fail_if_requested(failpoint, ApplyFailpoint::AfterPrepared)?;
    let config_matches = managed_config_matches(paths.config.as_path(), &current, &journal)?;
    let mut resumed_existing_write = config_matches.applied;
    if !current.matches_original(&journal) && !config_matches.applied {
        return mark_recovery(
            paths,
            journal,
            "Codex config changed before the prepared switch write, or helper-owned fields changed after it",
        );
    }
    if let Err(error) = validate_config_topology(paths.config.as_path(), current.present) {
        return mark_recovery(paths, journal, error.to_string());
    }
    let patches_auth = journal_patches_auth(paths, &journal)?;
    if let Some(auth_journal) = journal.auth_patch.clone() {
        let current_auth = read_config_snapshot(paths.auth.as_path())?;
        if let Err(error) = validate_config_topology(paths.auth.as_path(), current_auth.present) {
            return mark_recovery(paths, journal, error.to_string());
        }
        let (matches_original, matches_applied) = match auth_snapshot_matches_recorded_states(
            paths,
            &journal,
            &auth_journal,
            &current_auth,
        ) {
            Ok(matches) => matches,
            Err(CodexSwitchError::RecoveryRequired { reason }) => {
                return mark_recovery(paths, journal, reason);
            }
            Err(error) => return Err(error),
        };
        if patches_auth {
            resumed_existing_write |= matches_applied;
            if !matches_original && !matches_applied {
                return mark_recovery(
                    paths,
                    journal,
                    "Codex auth.json matches neither the prepared original nor applied fingerprint",
                );
            }
            if let Err(error) = ensure_auth_backup(paths, &auth_journal, &current_auth) {
                return match error {
                    CodexSwitchError::RecoveryRequired { reason } => {
                        mark_recovery(paths, journal, reason)
                    }
                    error => Err(error),
                };
            }
        } else {
            match restore_recorded_auth_original(paths, &journal, &auth_journal, &current_auth) {
                Ok(repaired) => resumed_existing_write |= repaired,
                Err(CodexSwitchError::RecoveryRequired { reason }) => {
                    return mark_recovery(paths, journal, reason);
                }
                Err(error) => return Err(error),
            }
        }
    }
    if current.matches_original(&journal) {
        let applied_text = match planned_write {
            Some(planned) if planned.original_text == current.text => planned.applied_text,
            Some(_) | None => {
                patch_on(
                    paths.config.as_path(),
                    current.text.as_str(),
                    journal.target_base_url.as_str(),
                    journal.recovery_client_patch(),
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

        match write_current_config_edit(paths, ConfigEdit::Write(applied_text), &journal, &current)
        {
            Ok(()) => {}
            Err(CodexSwitchError::RecoveryRequired { reason }) => {
                return mark_recovery(paths, journal, reason);
            }
            Err(error) => return Err(error),
        }
        fail_if_requested(failpoint, ApplyFailpoint::AfterConfigWrite)?;
    }
    let written = read_config_snapshot(paths.config.as_path())?;
    if !managed_config_matches(paths.config.as_path(), &written, &journal)?.applied {
        return mark_recovery(
            paths,
            journal,
            "helper-owned Codex fields changed before the applied switch state was committed",
        );
    }

    if patches_auth && let Some(auth_journal) = journal.auth_patch.clone() {
        let current_auth = read_config_snapshot(paths.auth.as_path())?;
        if let Err(error) = validate_config_topology(paths.auth.as_path(), current_auth.present) {
            return mark_recovery(paths, journal, error.to_string());
        }
        let (matches_original, matches_applied) = match auth_snapshot_matches_recorded_states(
            paths,
            &journal,
            &auth_journal,
            &current_auth,
        ) {
            Ok(matches) => matches,
            Err(CodexSwitchError::RecoveryRequired { reason }) => {
                return mark_recovery(paths, journal, reason);
            }
            Err(error) => return Err(error),
        };
        if matches_original && !matches_applied {
            let applied_auth =
                match render_recorded_auth_facade(paths, &journal, &auth_journal, &current_auth) {
                    Ok(applied) => applied,
                    Err(CodexSwitchError::RecoveryRequired { reason }) => {
                        return mark_recovery(paths, journal, reason);
                    }
                    Err(error) => return Err(error),
                };
            match write_current_auth_edit(
                paths,
                ConfigEdit::Write(applied_auth),
                &journal,
                &auth_journal,
                ExpectedAuthState::Original,
            ) {
                Ok(()) => {}
                Err(CodexSwitchError::RecoveryRequired { reason }) => {
                    return mark_recovery(paths, journal, reason);
                }
                Err(error) => return Err(error),
            }
            fail_if_requested(failpoint, ApplyFailpoint::AfterAuthWrite)?;
        } else if !matches_applied {
            return mark_recovery(
                paths,
                journal,
                "Codex auth.json matches neither the prepared original nor applied fingerprint",
            );
        }
        let written_auth = read_config_snapshot(paths.auth.as_path())?;
        let (_, written_matches_applied) = match auth_snapshot_matches_recorded_states(
            paths,
            &journal,
            &auth_journal,
            &written_auth,
        ) {
            Ok(matches) => matches,
            Err(CodexSwitchError::RecoveryRequired { reason }) => {
                return mark_recovery(paths, journal, reason);
            }
            Err(error) => return Err(error),
        };
        if !written_matches_applied {
            return mark_recovery(
                paths,
                journal,
                "Codex auth.json changed before the applied switch state was committed",
            );
        }
    } else if let Some(auth_journal) = journal.auth_patch.clone() {
        let current_auth = read_config_snapshot(paths.auth.as_path())?;
        match restore_recorded_auth_original(paths, &journal, &auth_journal, &current_auth) {
            Ok(repaired) => resumed_existing_write |= repaired,
            Err(CodexSwitchError::RecoveryRequired { reason }) => {
                return mark_recovery(paths, journal, reason);
            }
            Err(error) => return Err(error),
        }
    }

    recover_interrupted_file_captures(paths, &mut journal)?;
    journal.phase = JournalPhase::Applied;
    journal.pending_config = None;
    write_current_journal(paths, &journal)?;
    outcome(
        paths,
        if resumed_existing_write {
            CodexSwitchChange::Recovered
        } else {
            CodexSwitchChange::Applied
        },
    )
}

fn apply_off(
    paths: &SwitchPaths,
    current: ConfigSnapshot,
    journal: Option<SwitchJournal>,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    let Some(journal) = journal else {
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
            let config_matches =
                managed_config_matches(paths.config.as_path(), &current, &journal)?;
            if !config_matches.applied && !config_matches.original {
                return mark_recovery(
                    paths,
                    journal,
                    "helper-owned Codex fields changed after the switch was applied",
                );
            }
            if let Some(auth_journal) = journal.auth_patch.clone() {
                let auth = read_config_snapshot(paths.auth.as_path())?;
                let (matches_original, matches_applied) =
                    match auth_snapshot_matches_recorded_states(
                        paths,
                        &journal,
                        &auth_journal,
                        &auth,
                    ) {
                        Ok(matches) => matches,
                        Err(CodexSwitchError::RecoveryRequired { reason }) => {
                            return mark_recovery(paths, journal, reason);
                        }
                        Err(error) => return Err(error),
                    };
                if !matches_applied && !matches_original {
                    return mark_recovery(
                        paths,
                        journal,
                        "Codex auth.json changed after helper applied its client facade",
                    );
                }
                if let Err(error) = ensure_auth_backup(paths, &auth_journal, &auth) {
                    return match error {
                        CodexSwitchError::RecoveryRequired { reason } => {
                            mark_recovery(paths, journal, reason)
                        }
                        error => Err(error),
                    };
                }
            }
            begin_off(paths, journal, failpoint, false)
        }
        JournalPhase::Prepared => {
            let config_matches =
                managed_config_matches(paths.config.as_path(), &current, &journal)?;
            if !config_matches.original && !config_matches.applied {
                return mark_recovery(
                    paths,
                    journal,
                    "helper-owned Codex fields match neither the prepared original nor applied projection",
                );
            }
            if let Some(auth_journal) = journal.auth_patch.clone() {
                let auth = read_config_snapshot(paths.auth.as_path())?;
                let (matches_original, matches_applied) =
                    match auth_snapshot_matches_recorded_states(
                        paths,
                        &journal,
                        &auth_journal,
                        &auth,
                    ) {
                        Ok(matches) => matches,
                        Err(CodexSwitchError::RecoveryRequired { reason }) => {
                            return mark_recovery(paths, journal, reason);
                        }
                        Err(error) => return Err(error),
                    };
                if !matches_original && !matches_applied {
                    return mark_recovery(
                        paths,
                        journal,
                        "Codex auth.json matches neither the prepared original nor applied fingerprint",
                    );
                }
                if matches_applied
                    && let Err(error) = ensure_auth_backup(paths, &auth_journal, &auth)
                {
                    return match error {
                        CodexSwitchError::RecoveryRequired { reason } => {
                            mark_recovery(paths, journal, reason)
                        }
                        error => Err(error),
                    };
                }
            }
            match journal.operation {
                JournalOperation::On => begin_off(paths, journal, failpoint, true),
                JournalOperation::Off => {
                    let resumed = config_matches.original;
                    resume_off(paths, journal, failpoint, resumed)
                }
            }
        }
        JournalPhase::Restored => {
            let config_matches =
                managed_config_matches(paths.config.as_path(), &current, &journal)?;
            if !config_matches.original && !config_matches.applied {
                return mark_recovery(
                    paths,
                    journal,
                    "helper-owned Codex fields no longer match the retained switch recovery point",
                );
            }
            if let Some(auth_journal) = journal.auth_patch.clone() {
                let auth = read_config_snapshot(paths.auth.as_path())?;
                let (matches_original, matches_applied) =
                    match auth_snapshot_matches_recorded_states(
                        paths,
                        &journal,
                        &auth_journal,
                        &auth,
                    ) {
                        Ok(matches) => matches,
                        Err(CodexSwitchError::RecoveryRequired { reason }) => {
                            return mark_recovery(paths, journal, reason);
                        }
                        Err(error) => return Err(error),
                    };
                if !matches_original && !matches_applied {
                    return mark_recovery(
                        paths,
                        journal,
                        "Codex auth.json no longer matches the retained switch recovery point",
                    );
                }
                if let Err(error) = ensure_auth_backup(paths, &auth_journal, &auth) {
                    return match error {
                        CodexSwitchError::RecoveryRequired { reason } => {
                            mark_recovery(paths, journal, reason)
                        }
                        error => Err(error),
                    };
                }
            }
            resume_off(paths, journal, failpoint, true)
        }
    }
}

fn begin_off(
    paths: &SwitchPaths,
    mut journal: SwitchJournal,
    failpoint: ApplyFailpoint,
    resumed: bool,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    journal.phase = JournalPhase::Prepared;
    journal.operation = JournalOperation::Off;
    journal.operation_id = Uuid::new_v4().to_string();
    journal.pending_config = None;
    journal.recovery_reason = None;
    write_current_journal(paths, &journal)?;
    resume_off(paths, journal, failpoint, resumed)
}

fn resume_off(
    paths: &SwitchPaths,
    mut journal: SwitchJournal,
    failpoint: ApplyFailpoint,
    resumed: bool,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    fail_if_requested(failpoint, ApplyFailpoint::AfterPrepared)?;

    let current = read_config_snapshot(paths.config.as_path())?;
    let config_matches = managed_config_matches(paths.config.as_path(), &current, &journal)?;
    if !config_matches.applied && !config_matches.original {
        return mark_recovery(
            paths,
            journal,
            "helper-owned Codex fields changed between switch-off preparation and write",
        );
    }
    if let Err(error) = validate_config_topology(paths.config.as_path(), current.present) {
        return mark_recovery(paths, journal, error.to_string());
    }
    if config_matches.applied && !config_matches.original {
        let edit = patch_off(paths.config.as_path(), current.text.as_str(), &journal)?;
        let destination = edit.snapshot();
        if !managed_config_matches(paths.config.as_path(), &destination, &journal)?.original {
            journal.phase = JournalPhase::RecoveryRequired;
            journal.recovery_reason = Some(
                "restoring helper-owned Codex fields would not reproduce the original managed projection"
                    .to_string(),
            );
            write_current_journal(paths, &journal)?;
            return Err(CodexSwitchError::RestoreFingerprintMismatch);
        }
        journal.pending_config = Some(PendingConfigCommit::new(&current, &destination));
        write_current_journal(paths, &journal)?;

        match write_current_config_edit(paths, edit, &journal, &current) {
            Ok(()) => {}
            Err(CodexSwitchError::RecoveryRequired { reason }) => {
                return mark_recovery(paths, journal, reason);
            }
            Err(error) => return Err(error),
        }
        fail_if_requested(failpoint, ApplyFailpoint::AfterConfigWrite)?;
    }
    let written = read_config_snapshot(paths.config.as_path())?;
    if !managed_config_matches(paths.config.as_path(), &written, &journal)?.original {
        return mark_recovery(
            paths,
            journal,
            "helper-owned Codex fields changed before switch-off completion was committed",
        );
    }

    if let Some(auth_journal) = journal.auth_patch.clone() {
        let current_auth = read_config_snapshot(paths.auth.as_path())?;
        if let Err(error) = validate_config_topology(paths.auth.as_path(), current_auth.present) {
            return mark_recovery(paths, journal, error.to_string());
        }
        let (matches_original, matches_applied) = match auth_snapshot_matches_recorded_states(
            paths,
            &journal,
            &auth_journal,
            &current_auth,
        ) {
            Ok(matches) => matches,
            Err(CodexSwitchError::RecoveryRequired { reason }) => {
                return mark_recovery(paths, journal, reason);
            }
            Err(error) => return Err(error),
        };
        if matches_applied && !matches_original {
            let edit = if auth_journal.original_present {
                let backup = match read_auth_backup(paths, &auth_journal)? {
                    Some(backup) if backup.fingerprint == auth_journal.original_fingerprint => {
                        backup
                    }
                    Some(_) => {
                        return mark_recovery(
                            paths,
                            journal,
                            "the secure Codex auth backup does not match its recorded fingerprint",
                        );
                    }
                    None => {
                        return mark_recovery(
                            paths,
                            journal,
                            "the secure Codex auth backup is missing",
                        );
                    }
                };
                ConfigEdit::Write(backup.text)
            } else {
                ConfigEdit::Remove
            };
            match write_current_auth_edit(
                paths,
                edit,
                &journal,
                &auth_journal,
                ExpectedAuthState::Applied,
            ) {
                Ok(()) => {}
                Err(CodexSwitchError::RecoveryRequired { reason }) => {
                    return mark_recovery(paths, journal, reason);
                }
                Err(error) => return Err(error),
            }
            fail_if_requested(failpoint, ApplyFailpoint::AfterAuthRestore)?;
        } else if !matches_original {
            return mark_recovery(
                paths,
                journal,
                "Codex auth.json matches neither the applied facade nor its saved original",
            );
        }
        let restored_auth = read_config_snapshot(paths.auth.as_path())?;
        if !restored_auth.matches_original_auth(&auth_journal) {
            return mark_recovery(
                paths,
                journal,
                "Codex auth.json changed before switch-off completion was committed",
            );
        }
    }
    recover_interrupted_file_captures(paths, &mut journal)?;
    journal.phase = JournalPhase::Restored;
    journal.operation = JournalOperation::Off;
    journal.pending_config = None;
    journal.recovery_reason = None;
    write_current_journal(paths, &journal)?;
    if journal.auth_patch.is_none() {
        if let Some(helper_stanza_backup) = journal.helper_stanza_backup.as_ref() {
            remove_helper_stanza_backup(paths, helper_stanza_backup)?;
        }
        fail_if_requested(failpoint, ApplyFailpoint::AfterHelperStanzaBackupRemoval)?;
        remove_current_journal(paths)?;
    }
    outcome(
        paths,
        if resumed {
            CodexSwitchChange::Recovered
        } else {
            CodexSwitchChange::Removed
        },
    )
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
    let restore_lease = journal.as_ref().and_then(|journal| {
        (journal.phase == JournalPhase::Applied && journal.operation == JournalOperation::On)
            .then(|| {
                journal
                    .auto_restore_generation
                    .as_ref()
                    .map(|generation| CodexSwitchRestoreLease {
                        state_path: paths.state.clone(),
                        config_path_fingerprint: journal.config_path_fingerprint.clone(),
                        operation_id: journal.operation_id.clone(),
                        applied_fingerprint: journal.applied_fingerprint.clone(),
                        auto_restore_generation: generation.clone(),
                    })
            })
            .flatten()
    });
    Ok(CodexSwitchOutcome {
        change,
        status: status_from_snapshot(paths, &current, journal.as_ref(), None)?,
        restore_lease,
    })
}

fn status_from_snapshot(
    paths: &SwitchPaths,
    current: &ConfigSnapshot,
    journal: Option<&SwitchJournal>,
    journal_state_path: Option<&Path>,
) -> Result<CodexSwitchStatus, CodexSwitchError> {
    let state_path = journal_state_path.unwrap_or(paths.state.as_path());
    if let Some(journal) = journal
        && journal.config_path_fingerprint != paths.config_fingerprint
    {
        return Ok(CodexSwitchStatus {
            phase: CodexSwitchPhase::RecoveryRequired,
            enabled: false,
            managed: true,
            base_url: Some(journal.target_base_url.clone()),
            client_patch: Some(journal.client_patch),
            recovery_reason: Some(
                "switch state belongs to a different Codex config path".to_string(),
            ),
            config_path: paths.config.clone(),
            state_path: state_path.to_path_buf(),
        });
    }
    if let Some(journal) = journal
        && journal.auth_patch.as_ref().is_some_and(|auth| {
            auth.auth_path_fingerprint != config_path_fingerprint(paths.auth.as_path())
        })
    {
        return Ok(CodexSwitchStatus {
            phase: CodexSwitchPhase::RecoveryRequired,
            enabled: false,
            managed: true,
            base_url: Some(journal.target_base_url.clone()),
            client_patch: Some(journal.client_patch),
            recovery_reason: Some(
                "switch state belongs to a different Codex auth.json path".to_string(),
            ),
            config_path: paths.config.clone(),
            state_path: state_path.to_path_buf(),
        });
    }

    let config = inspect_config(paths.config.as_path(), current.text.as_str())?;
    let enabled =
        config.model_provider.as_deref() == Some(PROVIDER_ID) && config.helper_stanza.is_some();
    let config_base_url = config.helper_base_url;

    let Some(journal) = journal else {
        let orphaned = config.model_provider.as_deref() == Some(PROVIDER_ID);
        return Ok(CodexSwitchStatus {
            phase: if orphaned {
                CodexSwitchPhase::RecoveryRequired
            } else {
                CodexSwitchPhase::Off
            },
            enabled,
            managed: false,
            base_url: config_base_url,
            client_patch: None,
            recovery_reason: orphaned.then(|| {
                "helper provider config exists without helper-owned switch state".to_string()
            }),
            config_path: paths.config.clone(),
            state_path: state_path.to_path_buf(),
        });
    };
    let mut journal = journal.clone();
    let helper_stanza_recovery_reason =
        load_helper_stanza_backup_or_restored_config(paths, &mut journal, current)
            .err()
            .map(|error| error.to_string());
    let journal = &journal;
    let auth_should_be_applied = journal_patches_auth(paths, journal)?;

    if let Err(error) = validate_config_topology(paths.config.as_path(), current.present) {
        return Ok(CodexSwitchStatus {
            phase: CodexSwitchPhase::RecoveryRequired,
            enabled,
            managed: true,
            base_url: config_base_url.or_else(|| Some(journal.target_base_url.clone())),
            client_patch: Some(journal.client_patch),
            recovery_reason: Some(error.to_string()),
            config_path: paths.config.clone(),
            state_path: state_path.to_path_buf(),
        });
    }

    let (auth_matches_original, auth_matches_applied, auth_recovery_reason) =
        if let Some(auth_journal) = journal.auth_patch.as_ref() {
            let auth = read_config_snapshot(paths.auth.as_path())?;
            let topology_error = validate_config_topology(paths.auth.as_path(), auth.present)
                .err()
                .map(|error| error.to_string());
            let (matches_original, matches_applied, mut reason) = if let Some(reason) =
                topology_error
            {
                (false, false, Some(reason))
            } else {
                match auth_snapshot_matches_recorded_states(paths, journal, auth_journal, &auth) {
                    Ok((matches_original, matches_applied)) => (
                        matches_original,
                        matches_applied,
                        (!matches_original && !matches_applied).then(|| {
                            "current Codex auth.json does not match switch journal fingerprints"
                                .to_string()
                        }),
                    ),
                    Err(error) => (false, false, Some(error.to_string())),
                }
            };
            if reason.is_none() && auth_journal.original_present {
                match read_auth_backup(paths, auth_journal) {
                    Ok(Some(backup)) if backup.fingerprint == auth_journal.original_fingerprint => {
                    }
                    Ok(Some(_)) => {
                        reason = Some(
                            "the secure Codex auth backup does not match its recorded fingerprint"
                                .to_string(),
                        );
                    }
                    Ok(None) => {
                        reason = Some("the secure Codex auth backup is missing".to_string());
                    }
                    Err(error) => reason = Some(error.to_string()),
                }
            }
            (matches_original, matches_applied, reason)
        } else {
            (true, true, None)
        };
    let config_matches = if helper_stanza_recovery_reason.is_none() {
        managed_config_matches(paths.config.as_path(), current, journal)?
    } else {
        ManagedConfigMatches {
            original: false,
            applied: false,
        }
    };

    let recovery_material_reason = helper_stanza_recovery_reason.or(auth_recovery_reason);
    let (phase, recovery_reason) = if let Some(reason) = recovery_material_reason {
        (CodexSwitchPhase::RecoveryRequired, Some(reason))
    } else {
        match journal.phase {
            JournalPhase::RecoveryRequired => (
                CodexSwitchPhase::RecoveryRequired,
                journal.recovery_reason.clone(),
            ),
            JournalPhase::Applied
                if config_matches.applied
                    && if auth_should_be_applied {
                        auth_matches_applied
                    } else {
                        auth_matches_original
                    } =>
            {
                (CodexSwitchPhase::Applied, None)
            }
            JournalPhase::Restored
                if config_matches.original && auth_matches_original =>
            {
                (CodexSwitchPhase::Off, None)
            }
            JournalPhase::Restored => (
                CodexSwitchPhase::RecoveryRequired,
                Some(
                    "Codex files changed after switch-off; retained recovery material can restore the recorded original"
                        .to_string(),
                ),
            ),
            JournalPhase::Prepared
                if (config_matches.original || config_matches.applied)
                    && (auth_matches_original || auth_matches_applied) =>
            {
                (CodexSwitchPhase::Prepared, None)
            }
            _ => (
                CodexSwitchPhase::RecoveryRequired,
                Some(
                    "helper-owned Codex fields do not match the switch journal projection"
                        .to_string(),
                ),
            ),
        }
    };

    Ok(CodexSwitchStatus {
        phase,
        enabled,
        managed: true,
        base_url: config_base_url.or_else(|| Some(journal.target_base_url.clone())),
        client_patch: Some(journal.client_patch),
        recovery_reason,
        config_path: paths.config.clone(),
        state_path: state_path.to_path_buf(),
    })
}

struct ConfigInspection {
    model_provider: Option<String>,
    helper_stanza: Option<TomlValue>,
    helper_base_url: Option<String>,
    model_providers_present: bool,
    features: Option<TomlValue>,
}

fn reject_unowned_helper_config(config: &ConfigInspection) -> Result<(), CodexSwitchError> {
    if config.model_provider.as_deref() == Some(PROVIDER_ID) {
        return Err(CodexSwitchError::OrphanedActiveProvider);
    }
    Ok(())
}

pub(crate) fn project_original_codex_config<T>(
    project: impl FnOnce(Option<&str>, CodexAuthMetadata) -> T,
) -> Result<T, CodexSwitchError> {
    let paths = SwitchPaths::resolve()?;
    let _lock = OperationLock::acquire(paths.lock.as_path())?;
    reject_legacy_switch_state(&paths)?;

    project_original_codex_config_locked(&paths, project)
}

pub(crate) fn project_original_codex_config_for_onboarding<T>(
    project: impl FnOnce(Option<&str>, CodexAuthMetadata) -> T,
) -> Result<T, CodexSwitchError> {
    let paths = SwitchPaths::resolve()?;
    let _lock = OperationLock::acquire(paths.lock.as_path())?;
    cleanup_managed_switch_artifacts(&paths)?;
    if legacy_state_present(paths.legacy_state.as_path())? {
        if let Some(current_path) = current_journal_path_entry(&paths)? {
            return Err(CodexSwitchError::LegacySwitchStateConflict {
                legacy_path: paths.legacy_state.clone(),
                current_path,
            });
        }
        recover_legacy_switch_state(&paths, ApplyFailpoint::None)?;
    }

    project_original_codex_config_locked(&paths, project)
}

fn project_original_codex_config_locked<T>(
    paths: &SwitchPaths,
    project: impl FnOnce(Option<&str>, CodexAuthMetadata) -> T,
) -> Result<T, CodexSwitchError> {
    let current = read_config_snapshot(paths.config.as_path())?;
    validate_config_topology(paths.config.as_path(), current.present)?;
    let located_journal = read_inspection_journal_snapshot(paths)?;
    let (mut original, mut original_auth) = match located_journal {
        None => {
            let inspection = inspect_config(paths.config.as_path(), current.text.as_str())?;
            reject_unowned_helper_config(&inspection)?;
            (current, None)
        }
        Some(located) => {
            let mut journal = located.snapshot.journal;
            load_helper_stanza_backup_or_restored_config(paths, &mut journal, &current)?;
            let journal = &journal;
            ensure_journal_config_matches(paths, journal)?;
            let status =
                status_from_snapshot(paths, &current, Some(journal), Some(located.path.as_path()))?;
            let journal_is_stable = matches!(
                (journal.phase, status.phase),
                (JournalPhase::Applied, CodexSwitchPhase::Applied)
                    | (JournalPhase::Restored, CodexSwitchPhase::Off)
            );
            if !journal_is_stable {
                return Err(CodexSwitchError::RecoveryRequired {
                    reason: status.recovery_reason.unwrap_or_else(|| {
                        format!(
                            "Codex switch is {}; recover it before automatic onboarding",
                            status.phase.as_str()
                        )
                    }),
                });
            }
            let config_matches = managed_config_matches(paths.config.as_path(), &current, journal)?;
            let original = match journal.phase {
                JournalPhase::Applied if config_matches.applied => {
                    let edit = patch_off(paths.config.as_path(), current.text.as_str(), journal)?;
                    if !managed_config_matches(paths.config.as_path(), &edit.snapshot(), journal)?
                        .original
                    {
                        return Err(CodexSwitchError::RestoreFingerprintMismatch);
                    }
                    edit.into_snapshot()
                }
                JournalPhase::Restored if config_matches.original => current,
                _ => {
                    return Err(CodexSwitchError::RecoveryRequired {
                        reason: "current Codex config does not match the switch journal; recover it before automatic onboarding"
                            .to_string(),
                    });
                }
            };
            let original_auth = journal
                .auth_patch
                .as_ref()
                .map(|_| original_auth_snapshot_for_reapply(paths, journal))
                .transpose()?;
            (original, original_auth)
        }
    };

    let original_present = original.present;
    let original_text = Zeroizing::new(std::mem::take(&mut original.text));
    let auth_metadata = if let Some(snapshot) = original_auth.as_mut() {
        let present = snapshot.present;
        let text = Zeroizing::new(std::mem::take(&mut snapshot.text));
        codex_auth_metadata_from_json(present.then_some(text.as_str()))
    } else {
        read_codex_auth_metadata()
    };
    let output = project(
        original_present.then_some(original_text.as_str()),
        auth_metadata,
    );
    Ok(output)
}

fn inspect_config(path: &Path, text: &str) -> Result<ConfigInspection, CodexSwitchError> {
    if text.trim().is_empty() {
        return Ok(ConfigInspection {
            model_provider: None,
            helper_stanza: None,
            helper_base_url: None,
            model_providers_present: false,
            features: None,
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
    let features = root.get("features").cloned();

    Ok(ConfigInspection {
        model_provider,
        helper_stanza,
        helper_base_url,
        model_providers_present: providers.is_some(),
        features,
    })
}

fn managed_config_matches(
    path: &Path,
    current: &ConfigSnapshot,
    journal: &SwitchJournal,
) -> Result<ManagedConfigMatches, CodexSwitchError> {
    let current_config = inspect_config(path, current.text.as_str())?;
    let client_patch = journal.recovery_client_patch();
    let compiled = client_patch
        .compile()
        .map_err(|error| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: format!("invalid recorded client patch: {error}"),
        })?;
    let expected_applied_text =
        patch_on(path, "", journal.target_base_url.as_str(), client_patch)?.text;
    let expected_applied = inspect_config(path, expected_applied_text.as_str())?;

    let original = (!journal.original_config_present || current.present)
        && current_config.model_provider == journal.original_model_provider
        && current_config.helper_stanza == journal.original_helper_stanza
        && managed_feature_matches(
            path,
            &current_config,
            "remote_compaction_v2",
            compiled.remote_compaction_v2,
            journal.original_remote_compaction_v2,
        )?
        && managed_feature_matches(
            path,
            &current_config,
            "image_generation",
            compiled.image_generation,
            journal.original_image_generation,
        )?;
    let applied = current.matches_applied(journal)
        || (current.present
            && current_config.model_provider.as_deref() == Some(PROVIDER_ID)
            && current_config.helper_stanza == expected_applied.helper_stanza
            && managed_feature_matches(
                path,
                &current_config,
                "remote_compaction_v2",
                compiled.remote_compaction_v2,
                applied_feature_value(compiled.remote_compaction_v2),
            )?
            && managed_feature_matches(
                path,
                &current_config,
                "image_generation",
                compiled.image_generation,
                applied_feature_value(compiled.image_generation),
            )?);

    Ok(ManagedConfigMatches { original, applied })
}

fn managed_feature_matches(
    path: &Path,
    config: &ConfigInspection,
    key: &str,
    patch: CodexFeatureBoolPatch,
    expected: Option<bool>,
) -> Result<bool, CodexSwitchError> {
    if patch == CodexFeatureBoolPatch::Preserve {
        return Ok(true);
    }
    Ok(inspected_feature_bool(path, config, key)? == expected)
}

fn inspected_feature_bool(
    path: &Path,
    config: &ConfigInspection,
    key: &str,
) -> Result<Option<bool>, CodexSwitchError> {
    let Some(features) = config.features.as_ref() else {
        return Ok(None);
    };
    let features = features
        .as_table()
        .ok_or_else(|| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: "features must be a table".to_string(),
        })?;
    let Some(value) = features.get(key) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: format!("features.{key} must be a boolean"),
        })
}

const fn applied_feature_value(patch: CodexFeatureBoolPatch) -> Option<bool> {
    match patch {
        CodexFeatureBoolPatch::Preserve => None,
        CodexFeatureBoolPatch::Set(value) => Some(value),
    }
}

fn patch_on(
    path: &Path,
    text: &str,
    base_url: &str,
    client_patch: CodexClientPatchConfig,
) -> Result<OnPatch, CodexSwitchError> {
    let compiled = client_patch.compile().map_err(invalid_client_patch_error)?;
    let mut document = editable_document(path, text)?;
    let original_model_provider_repr = model_provider_repr_from_document(path, &document)?;
    let original_helper_stanza_repr = helper_stanza_repr_from_document(path, &document)?;
    let root = document.as_table_mut();
    let owns_remote_compaction_v2 =
        matches!(compiled.remote_compaction_v2, CodexFeatureBoolPatch::Set(_));
    let owns_image_generation = matches!(compiled.image_generation, CodexFeatureBoolPatch::Set(_));
    let original_features_present =
        (owns_remote_compaction_v2 || owns_image_generation) && root.contains_key("features");
    let original_remote_compaction_v2 = capture_feature_bool(
        path,
        root,
        "remote_compaction_v2",
        owns_remote_compaction_v2,
    )?;
    let original_image_generation =
        capture_feature_bool(path, root, "image_generation", owns_image_generation)?;
    apply_feature_bool_patch(
        path,
        root,
        "remote_compaction_v2",
        compiled.remote_compaction_v2,
    )?;
    apply_feature_bool_patch(path, root, "image_generation", compiled.image_generation)?;

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
    helper.insert(
        "name",
        editable_value(compiled.provider_identity.provider_name()),
    );
    helper.insert("base_url", editable_value(base_url));
    helper.insert("wire_api", editable_value("responses"));
    helper.insert("request_max_retries", editable_value(0));
    if let CodexTomlBoolPatch::Set(value) = compiled.requires_openai_auth {
        helper.insert("requires_openai_auth", editable_value(value));
    }
    if let CodexTomlBoolPatch::Set(value) = compiled.supports_websockets {
        helper.insert("supports_websockets", editable_value(value));
    }
    {
        let mut headers = Table::new();
        headers.insert(
            CODEX_CLIENT_RUNTIME_PATCH_HEADER,
            editable_value(CodexClientRuntimePatch::from(client_patch).encode()),
        );
        if compiled.actor_marker {
            headers.insert(
                CODEX_CLIENT_FACADE_ACTOR_HEADER,
                editable_value(CODEX_CLIENT_FACADE_ACTOR_VALUE),
            );
        }
        helper.insert("http_headers", Item::Table(headers));
    }
    providers.insert(PROVIDER_ID, Item::Table(helper));
    set_string_preserving_decor(root, "model_provider", PROVIDER_ID);
    Ok(OnPatch {
        text: document.to_string(),
        original_model_provider_repr,
        original_helper_stanza_repr,
        original_features_present,
        original_remote_compaction_v2,
        original_image_generation,
    })
}

fn capture_feature_bool(
    path: &Path,
    root: &Table,
    key: &str,
    owned: bool,
) -> Result<Option<bool>, CodexSwitchError> {
    if !owned {
        return Ok(None);
    }
    let Some(features) = root.get("features") else {
        return Ok(None);
    };
    let features = features
        .as_table()
        .ok_or_else(|| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: "features must be a table".to_string(),
        })?;
    let Some(value) = features.get(key) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: format!("features.{key} must be a boolean"),
        })
}

fn apply_feature_bool_patch(
    path: &Path,
    root: &mut Table,
    key: &str,
    patch: CodexFeatureBoolPatch,
) -> Result<(), CodexSwitchError> {
    let CodexFeatureBoolPatch::Set(value) = patch else {
        return Ok(());
    };
    if !root.contains_key("features") {
        root.insert("features", Item::Table(Table::new()));
    }
    let features = root
        .get_mut("features")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: "features must be a table".to_string(),
        })?;
    set_value_preserving_decor(features, key, EditableValue::from(value));
    Ok(())
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
            match (
                journal.original_helper_stanza.as_ref(),
                journal.original_helper_stanza_repr.as_deref(),
            ) {
                (Some(original), Some(repr)) => {
                    providers.insert(
                        PROVIDER_ID,
                        editable_helper_stanza_from_repr(repr, original, path)?,
                    );
                }
                (None, None) => {
                    providers.remove(PROVIDER_ID);
                }
                _ => {
                    return Err(CodexSwitchError::InvalidState {
                        path: path.to_path_buf(),
                        reason: "original helper stanza value and representation must agree"
                            .to_string(),
                    });
                }
            }
            !journal.original_model_providers_present && providers.is_empty()
        } else {
            false
        };
    if remove_model_providers {
        root.remove("model_providers");
    }

    let compiled = journal.recovery_client_patch().compile().map_err(|error| {
        CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: error.to_string(),
        }
    })?;
    restore_feature_bool_patch(
        path,
        root,
        "remote_compaction_v2",
        compiled.remote_compaction_v2,
        journal.original_remote_compaction_v2,
    )?;
    restore_feature_bool_patch(
        path,
        root,
        "image_generation",
        compiled.image_generation,
        journal.original_image_generation,
    )?;
    if !journal.original_features_present
        && root
            .get("features")
            .and_then(Item::as_table)
            .is_some_and(Table::is_empty)
    {
        root.remove("features");
    }

    let root_is_empty = root.is_empty();
    let rendered = document.to_string();
    if !journal.original_config_present && root_is_empty && rendered.trim().is_empty() {
        Ok(ConfigEdit::Remove)
    } else {
        Ok(ConfigEdit::Write(rendered))
    }
}

fn restore_feature_bool_patch(
    path: &Path,
    root: &mut Table,
    key: &str,
    patch: CodexFeatureBoolPatch,
    original: Option<bool>,
) -> Result<(), CodexSwitchError> {
    if patch == CodexFeatureBoolPatch::Preserve {
        return Ok(());
    }
    let features = root
        .get_mut("features")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: "features must be a table while restoring owned Codex feature keys".to_string(),
        })?;
    match original {
        Some(value) => set_value_preserving_decor(features, key, EditableValue::from(value)),
        None => {
            features.remove(key);
        }
    }
    Ok(())
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

fn helper_stanza_repr_from_document(
    path: &Path,
    document: &DocumentMut,
) -> Result<Option<String>, CodexSwitchError> {
    let Some(providers) = document.as_table().get("model_providers") else {
        return Ok(None);
    };
    let providers = providers
        .as_table()
        .ok_or_else(|| CodexSwitchError::InvalidConfig {
            path: path.to_path_buf(),
            reason: "model_providers must be a table".to_string(),
        })?;
    let Some(helper) = providers.get(PROVIDER_ID) else {
        return Ok(None);
    };

    let mut snapshot = DocumentMut::new();
    let mut snapshot_providers = Table::new();
    snapshot_providers.insert(PROVIDER_ID, helper.clone());
    snapshot
        .as_table_mut()
        .insert("model_providers", Item::Table(snapshot_providers));
    Ok(Some(snapshot.to_string()))
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

fn editable_helper_stanza_from_repr(
    repr: &str,
    expected: &TomlValue,
    path: &Path,
) -> Result<Item, CodexSwitchError> {
    let document = repr
        .parse::<DocumentMut>()
        .map_err(|error| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: sanitized_toml_parse_reason(error.message(), error.span(), repr),
        })?;
    let item = document
        .as_table()
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .cloned()
        .ok_or_else(|| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: "original helper stanza representation is missing".to_string(),
        })?;
    let semantic = toml::from_str::<TomlValue>(repr)
        .ok()
        .and_then(|value| {
            value
                .as_table()?
                .get("model_providers")?
                .as_table()?
                .get(PROVIDER_ID)
                .cloned()
        })
        .ok_or_else(|| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: "original helper stanza representation is not a TOML table".to_string(),
        })?;
    if &semantic != expected {
        return Err(CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: "original helper stanza representation does not match its value".to_string(),
        });
    }
    Ok(item)
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
        let information = crate::windows_file_info::path_information_no_follow(path)
            .map_err(|source| io_error("read hard-link count for", path, source))?;
        if crate::windows_file_info::is_reparse_point(&information) {
            return Err(CodexSwitchError::UnsupportedConfigTopology {
                path: path.to_path_buf(),
                reason: "reparse-point configs are not replaced because their target can change"
                    .to_string(),
            });
        }
        if information.number_of_links() > 1 {
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

fn verify_journal_before_commit(
    path: &Path,
    expectation: ConfigCommitExpectation<'_>,
) -> Result<(), CodexSwitchError> {
    let (present, current_fingerprint) = read_config_identity(path)?;
    if present != expectation.expected.present
        || current_fingerprint != expectation.expected.fingerprint
    {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "Codex config changed after the final switch snapshot was captured".to_string(),
        });
    }
    validate_config_topology(path, present).map_err(|error| CodexSwitchError::RecoveryRequired {
        reason: error.to_string(),
    })
}

fn verify_auth_before_commit(
    path: &Path,
    expectation: AuthCommitExpectation<'_>,
) -> Result<(), CodexSwitchError> {
    let current = read_config_snapshot(path)?;
    let matches = match expectation.state {
        ExpectedAuthState::Original => current.matches_original_auth(expectation.journal),
        ExpectedAuthState::Applied => {
            auth_snapshot_matches_recorded_states(
                expectation.paths,
                expectation.switch_journal,
                expectation.journal,
                &current,
            )?
            .1
        }
    };
    if !matches {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: "Codex auth.json changed after the final switch fingerprint check".to_string(),
        });
    }
    validate_config_topology(path, current.present).map_err(|error| {
        CodexSwitchError::RecoveryRequired {
            reason: error.to_string(),
        }
    })
}

fn verify_legacy_snapshot_before_commit(
    path: &Path,
    legacy_path: &Path,
    expectation: &ConfigSnapshot,
) -> Result<(), CodexSwitchError> {
    let (present, current_fingerprint) = read_config_identity(path)
        .map_err(|error| legacy_recovery_required(legacy_path, error.to_string()))?;
    if present != expectation.present || current_fingerprint != expectation.fingerprint {
        return Err(legacy_recovery_required(
            legacy_path,
            format!(
                "{} changed while legacy Codex switch state was being recovered",
                path.display()
            ),
        ));
    }
    validate_config_topology(path, present)
        .map_err(|error| legacy_recovery_required(legacy_path, error.to_string()))
}

fn verify_file_before_commit(
    path: &Path,
    expectation: FileCommitExpectation<'_>,
) -> Result<(), CodexSwitchError> {
    match expectation {
        FileCommitExpectation::Journal(expectation) => {
            verify_journal_before_commit(path, expectation)
        }
        FileCommitExpectation::Auth(expectation) => verify_auth_before_commit(path, expectation),
        FileCommitExpectation::LegacySnapshot {
            expected,
            legacy_path,
        } => verify_legacy_snapshot_before_commit(path, legacy_path, expected),
    }
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

fn read_inspection_journal_snapshot(
    paths: &SwitchPaths,
) -> Result<Option<LocatedJournalSnapshot>, CodexSwitchError> {
    if let Some(snapshot) = read_journal_snapshot(paths.state.as_path())? {
        return Ok(Some(LocatedJournalSnapshot {
            path: paths.state.clone(),
            snapshot,
        }));
    }
    Ok(
        read_matching_unscoped_journal_snapshot(paths)?.map(|snapshot| LocatedJournalSnapshot {
            path: paths.unscoped_state.clone(),
            snapshot,
        }),
    )
}

fn read_matching_unscoped_journal_snapshot(
    paths: &SwitchPaths,
) -> Result<Option<JournalSnapshot>, CodexSwitchError> {
    match std::fs::symlink_metadata(paths.unscoped_state.as_path()) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Ok(None),
    }
    let snapshot = match read_journal_snapshot(paths.unscoped_state.as_path()) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) | Err(_) => return Ok(None),
    };
    Ok((snapshot.journal.config_path_fingerprint == paths.config_fingerprint).then_some(snapshot))
}

fn migrate_matching_unscoped_journal(paths: &SwitchPaths) -> Result<(), CodexSwitchError> {
    let Some(before) = read_matching_unscoped_journal_snapshot(paths)? else {
        return Ok(());
    };
    if switch_path_entry_present(paths.state.as_path(), "inspect scoped switch state at")? {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: format!(
                "matching unscoped switch state at {:?} conflicts with scoped switch state at {:?}; neither state was modified",
                paths.unscoped_state, paths.state
            ),
        });
    }

    prepare_state_directory(paths.state.as_path())?;
    move_file_no_replace(paths.unscoped_state.as_path(), paths.state.as_path())?;
    sync_parent_directory(paths.state.as_path())?;

    let migrated = read_journal_snapshot(paths.state.as_path());
    let unchanged = matches!(
        migrated.as_ref(),
        Ok(Some(after)) if after.raw == before.raw && after.journal == before.journal
    );
    if unchanged {
        return Ok(());
    }

    let reason = match migrated {
        Ok(Some(_)) => {
            "the unscoped switch journal changed while it was being migrated".to_string()
        }
        Ok(None) => "the migrated switch journal disappeared before verification".to_string(),
        Err(error) => format!("the migrated switch journal failed verification: {error}"),
    };
    match move_file_no_replace(paths.state.as_path(), paths.unscoped_state.as_path()) {
        Ok(()) => {
            sync_parent_directory(paths.unscoped_state.as_path())?;
            Err(CodexSwitchError::RecoveryRequired { reason })
        }
        Err(rollback_error) => Err(CodexSwitchError::RecoveryRequired {
            reason: format!(
                "{reason}; rollback without replacement also failed ({rollback_error}); both paths were preserved for reconciliation"
            ),
        }),
    }
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
    if !matches!(journal.version, LEGACY_STATE_VERSION | STATE_VERSION) {
        return Err(CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: format!(
                "unsupported state version {}; expected {LEGACY_STATE_VERSION} or {STATE_VERSION}",
                journal.version
            ),
        });
    }
    Uuid::parse_str(journal.operation_id.as_str()).map_err(|error| {
        CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: format!("invalid switch operation identifier: {error}"),
        }
    })?;
    if let Some(auto_restore_generation) = journal.auto_restore_generation.as_deref() {
        Uuid::parse_str(auto_restore_generation).map_err(|error| {
            CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: format!("invalid auto-restore generation: {error}"),
            }
        })?;
    }
    if let Some(pending) = journal.pending_config.as_ref() {
        if !matches!(
            journal.phase,
            JournalPhase::Prepared | JournalPhase::RecoveryRequired
        ) {
            return Err(CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: "pending config commit requires a prepared or recovery-required journal"
                    .to_string(),
            });
        }
        let absent_fingerprint = fingerprint(&[]);
        for (label, present, recorded_fingerprint) in [
            (
                "source",
                pending.source_present,
                pending.source_fingerprint.as_str(),
            ),
            (
                "destination",
                pending.destination_present,
                pending.destination_fingerprint.as_str(),
            ),
        ] {
            if !present && recorded_fingerprint != absent_fingerprint {
                return Err(CodexSwitchError::InvalidState {
                    path: path.to_path_buf(),
                    reason: format!(
                        "an absent pending config {label} must use the absent-file fingerprint"
                    ),
                });
            }
        }
    }
    journal
        .client_patch
        .compile()
        .map_err(|error| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: format!("invalid recorded client patch: {error}"),
        })?;
    if let Some(auth) = journal.auth_patch.as_ref() {
        validate_auth_journal(path, auth)?;
    }
    match journal.version {
        STATE_VERSION => {
            if journal.original_helper_stanza.is_some()
                || journal.original_helper_stanza_repr.is_some()
            {
                return Err(CodexSwitchError::InvalidState {
                    path: path.to_path_buf(),
                    reason:
                        "version 2 switch state cannot contain inline helper stanza recovery data"
                            .to_string(),
                });
            }
            let helper_stanza = journal.helper_stanza_backup.as_ref().ok_or_else(|| {
                CodexSwitchError::InvalidState {
                    path: path.to_path_buf(),
                    reason: "version 2 switch state is missing helper stanza recovery metadata"
                        .to_string(),
                }
            })?;
            validate_helper_stanza_journal(path, helper_stanza)?;
        }
        LEGACY_STATE_VERSION => {
            validate_legacy_helper_stanza_fields(path, &journal)?;
            if let Some(helper_stanza) = journal.helper_stanza_backup.as_ref() {
                validate_helper_stanza_journal(path, helper_stanza)?;
            }
        }
        _ => unreachable!("switch state version was validated above"),
    }
    if let Some(auth_recovery_patch) = journal.auth_recovery_patch {
        if journal.auth_patch.is_none() {
            return Err(CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: "auth recovery patch exists without auth recovery metadata".to_string(),
            });
        }
        let recovery_strategy = auth_recovery_patch
            .compile()
            .map_err(|error| CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: format!("invalid auth recovery client patch: {error}"),
            })?
            .auth_facade;
        let current_strategy = journal
            .recovery_client_patch()
            .compile()
            .map_err(|error| CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: format!("invalid recorded client patch: {error}"),
            })?
            .auth_facade;
        if recovery_strategy == CodexAuthFacadeStrategy::Preserve
            || current_strategy != CodexAuthFacadeStrategy::Preserve
        {
            return Err(CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: "auth recovery patch must retain a non-preserving facade for a client patch that currently preserves auth.json"
                    .to_string(),
            });
        }
    } else if journal.auth_patch.is_some()
        && journal
            .recovery_client_patch()
            .compile()
            .map_err(|error| CodexSwitchError::InvalidState {
                path: path.to_path_buf(),
                reason: format!("invalid recorded client patch: {error}"),
            })?
            .auth_facade
            == CodexAuthFacadeStrategy::Preserve
    {
        return Err(CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason:
                "auth recovery metadata for a preserving client patch is missing its source patch"
                    .to_string(),
        });
    }
    Ok(JournalSnapshot { raw, journal })
}

fn validate_helper_stanza_journal(
    path: &Path,
    helper_stanza: &HelperStanzaJournal,
) -> Result<(), CodexSwitchError> {
    let invalid = |reason: &str| CodexSwitchError::InvalidState {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    if !helper_stanza.original_present && helper_stanza.original_fingerprint != fingerprint(&[]) {
        return Err(invalid(
            "an absent original helper stanza must use the absent-value fingerprint",
        ));
    }
    match (
        helper_stanza.original_present,
        helper_stanza.backup_file_name.as_deref(),
    ) {
        (true, Some(name)) if managed_helper_stanza_backup_name(name) => {}
        (true, Some(_)) => {
            return Err(invalid(
                "helper stanza backup file name is not helper-owned",
            ));
        }
        (true, None) => {
            return Err(invalid(
                "present original helper stanza requires a backup file",
            ));
        }
        (false, Some(_)) => {
            return Err(invalid(
                "absent original helper stanza cannot record a backup file",
            ));
        }
        (false, None) => {}
    }
    Ok(())
}

fn validate_auth_journal(path: &Path, auth: &AuthJournal) -> Result<(), CodexSwitchError> {
    let invalid = |reason: &str| CodexSwitchError::InvalidState {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };
    if !auth.original_present && auth.original_fingerprint != fingerprint(&[]) {
        return Err(invalid(
            "an absent original auth.json must use the absent-file fingerprint",
        ));
    }
    match (auth.original_present, auth.backup_file_name.as_deref()) {
        (true, Some(name)) if managed_auth_backup_name(name) => {}
        (true, Some(_)) => return Err(invalid("auth backup file name is not helper-owned")),
        (true, None) => return Err(invalid("present original auth.json requires a backup file")),
        (false, Some(_)) => {
            return Err(invalid(
                "absent original auth.json cannot record an auth backup file",
            ));
        }
        (false, None) => {}
    }
    Ok(())
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
    expected: &ConfigSnapshot,
) -> Result<(), CodexSwitchError> {
    let expectation = FileCommitExpectation::Journal(ConfigCommitExpectation { journal, expected });
    write_file_edit(path, edit, expectation)
}

fn write_auth_edit(
    paths: &SwitchPaths,
    path: &Path,
    edit: ConfigEdit,
    switch_journal: &SwitchJournal,
    auth_journal: &AuthJournal,
    expected: ExpectedAuthState,
) -> Result<(), CodexSwitchError> {
    let expectation = FileCommitExpectation::Auth(AuthCommitExpectation {
        paths,
        switch_journal,
        journal: auth_journal,
        state: expected,
    });
    write_file_edit(path, edit, expectation)
}

fn write_snapshot_edit(
    path: &Path,
    legacy_path: &Path,
    edit: ConfigEdit,
    expected: &ConfigSnapshot,
) -> Result<(), CodexSwitchError> {
    let expectation = FileCommitExpectation::LegacySnapshot {
        expected,
        legacy_path,
    };
    write_file_edit(path, edit, expectation)
}

fn write_file_edit(
    path: &Path,
    edit: ConfigEdit,
    expectation: FileCommitExpectation<'_>,
) -> Result<(), CodexSwitchError> {
    match edit {
        ConfigEdit::Write(text) => atomic_write_text(
            path,
            text.as_str(),
            FilePermissions::PreserveOrSecure,
            Some(expectation),
        ),
        ConfigEdit::Remove => commit_remove_file(path, expectation),
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
    expectation: Option<FileCommitExpectation<'_>>,
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
    #[cfg(windows)]
    let preserved_metadata = match permissions {
        FilePermissions::Secure => None,
        FilePermissions::PreserveOrSecure => match std::fs::symlink_metadata(path) {
            Ok(metadata) => Some(
                crate::config::ConfigFileMetadata::capture(path, &metadata)
                    .map_err(|source| io_error("capture permissions for", path, source))?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => return Err(io_error("read metadata for", path, source)),
        },
    };
    let temp_path = parent.join(format!(
        "{SWITCH_TEMP_FILE_PREFIX}{}{SWITCH_TEMP_FILE_SUFFIX}",
        Uuid::new_v4()
    ));

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
        #[cfg(windows)]
        match preserved_metadata.as_ref() {
            Some(metadata) => {
                metadata
                    .apply_to_staged_file(temp_path.as_path())
                    .map_err(|source| {
                        io_error("preserve temporary file permissions for", path, source)
                    })?
            }
            None => crate::local_operator::secure_private_windows_path(temp_path.as_path(), false)
                .map_err(|error| {
                    io_error(
                        "secure temporary file for",
                        path,
                        std::io::Error::other(error.to_string()),
                    )
                })?,
        }
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
        match expectation {
            Some(expectation) => commit_staged_file(temp_path.as_path(), path, expectation),
            None => {
                replace_file(temp_path.as_path(), path)?;
                sync_parent_directory(path)
            }
        }
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(temp_path);
    }
    result
}

fn managed_commit_capture_path(
    path: &Path,
    expectation: FileCommitExpectation<'_>,
) -> Result<Option<PathBuf>, CodexSwitchError> {
    let Some((operation_id, role)) = expectation.managed_capture() else {
        return Ok(None);
    };
    managed_capture_path(path, operation_id, role).map(Some)
}

fn managed_capture_path(
    path: &Path,
    operation_id: &str,
    role: ManagedCommitRole,
) -> Result<PathBuf, CodexSwitchError> {
    let operation_id =
        Uuid::parse_str(operation_id).map_err(|error| CodexSwitchError::InvalidState {
            path: path.to_path_buf(),
            reason: format!("switch operation id is not a UUID: {error}"),
        })?;
    let parent = path.parent().ok_or_else(|| {
        io_error(
            "resolve parent for",
            path,
            std::io::Error::other("path has no parent directory"),
        )
    })?;
    Ok(parent.join(format!(
        "{SWITCH_CAPTURE_FILE_PREFIX}{operation_id}-{}",
        role.as_str()
    )))
}

fn recover_interrupted_file_captures(
    paths: &SwitchPaths,
    journal: &mut SwitchJournal,
) -> Result<(), CodexSwitchError> {
    if journal.phase == JournalPhase::RecoveryRequired {
        return Ok(());
    }
    for (path, role) in [
        (paths.config.as_path(), ManagedCommitRole::Config),
        (paths.auth.as_path(), ManagedCommitRole::Auth),
    ] {
        if matches!(role, ManagedCommitRole::Auth) && journal.auth_patch.is_none() {
            continue;
        }
        let capture_path = managed_capture_path(path, journal.operation_id.as_str(), role)?;
        if let Err(reason) =
            recover_interrupted_file_capture(paths, path, capture_path.as_path(), role, journal)
        {
            journal.phase = JournalPhase::RecoveryRequired;
            journal.recovery_reason = Some(reason.clone());
            write_current_journal(paths, journal)?;
            return Err(CodexSwitchError::RecoveryRequired { reason });
        }
    }
    Ok(())
}

fn recover_interrupted_file_capture(
    paths: &SwitchPaths,
    path: &Path,
    capture_path: &Path,
    role: ManagedCommitRole,
    journal: &SwitchJournal,
) -> Result<(), String> {
    let capture = read_config_snapshot(capture_path).map_err(|error| error.to_string())?;
    if !capture.present {
        return Ok(());
    }
    validate_config_topology(capture_path, true).map_err(|error| error.to_string())?;
    let patches_auth = journal_patches_auth(paths, journal).map_err(|error| error.to_string())?;
    let capture_matches_expected = match (role, journal.operation) {
        (ManagedCommitRole::Config, _) => recorded_config_source_matches(&capture, journal),
        (ManagedCommitRole::Auth, JournalOperation::On) if patches_auth => journal
            .auth_patch
            .as_ref()
            .is_some_and(|auth| capture.matches_original_auth(auth)),
        (ManagedCommitRole::Auth, JournalOperation::On) => match journal.auth_patch.as_ref() {
            Some(auth) => {
                auth_snapshot_matches_recorded_states(paths, journal, auth, &capture)
                    .map_err(|error| error.to_string())?
                    .1
            }
            None => false,
        },
        (ManagedCommitRole::Auth, JournalOperation::Off) => match journal.auth_patch.as_ref() {
            Some(auth) => {
                auth_snapshot_matches_recorded_states(paths, journal, auth, &capture)
                    .map_err(|error| error.to_string())?
                    .1
            }
            None => false,
        },
    };
    if !capture_matches_expected {
        return Err(format!(
            "the preserved {} capture changed after it was detached; it was retained at {:?}",
            role.as_str(),
            capture_path
        ));
    }

    let current = read_config_snapshot(path).map_err(|error| error.to_string())?;
    let current_matches_desired = match (role, journal.operation) {
        (ManagedCommitRole::Config, _) => {
            recorded_config_destination_matches(path, &current, journal)
                .map_err(|error| error.to_string())?
        }
        (ManagedCommitRole::Auth, JournalOperation::On) if patches_auth => {
            match journal.auth_patch.as_ref() {
                Some(auth) => {
                    auth_snapshot_matches_recorded_states(paths, journal, auth, &current)
                        .map_err(|error| error.to_string())?
                        .1
                }
                None => false,
            }
        }
        (ManagedCommitRole::Auth, JournalOperation::On) => journal
            .auth_patch
            .as_ref()
            .is_some_and(|auth| current.matches_original_auth(auth)),
        (ManagedCommitRole::Auth, JournalOperation::Off) => journal
            .auth_patch
            .as_ref()
            .is_some_and(|auth| current.matches_original_auth(auth)),
    };
    if current_matches_desired {
        return remove_file_durable(capture_path).map_err(|error| error.to_string());
    }
    if !current.present {
        move_file_no_replace(capture_path, path).map_err(|error| {
            format!(
                "failed to restore the preserved {} capture without replacing another writer: {error}",
                role.as_str()
            )
        })?;
        return sync_parent_directory(path).map_err(|error| error.to_string());
    }
    Err(format!(
        "a competing writer changed the Codex {} path while the helper held a preserved capture at {:?}",
        role.as_str(),
        capture_path
    ))
}

fn recorded_config_source_matches(snapshot: &ConfigSnapshot, journal: &SwitchJournal) -> bool {
    match journal.pending_config.as_ref() {
        Some(pending) => pending.source_matches(snapshot),
        None => match journal.operation {
            JournalOperation::On => snapshot.matches_original(journal),
            JournalOperation::Off => snapshot.matches_applied(journal),
        },
    }
}

fn recorded_config_destination_matches(
    path: &Path,
    snapshot: &ConfigSnapshot,
    journal: &SwitchJournal,
) -> Result<bool, CodexSwitchError> {
    if journal
        .pending_config
        .as_ref()
        .is_some_and(|pending| pending.destination_matches(snapshot))
    {
        return Ok(true);
    }
    let managed = managed_config_matches(path, snapshot, journal)?;
    Ok(match journal.operation {
        JournalOperation::On => managed.applied,
        JournalOperation::Off => managed.original,
    })
}

fn capture_expected_file(
    path: &Path,
    capture_path: &Path,
    expectation: FileCommitExpectation<'_>,
) -> Result<(), CodexSwitchError> {
    ensure_pending_delete_destination_absent(capture_path)?;
    move_file_no_replace(path, capture_path)?;
    sync_parent_directory(path)?;
    if let Err(error) = verify_file_before_commit(capture_path, expectation) {
        return match move_file_no_replace(capture_path, path) {
            Ok(()) => {
                sync_parent_directory(path)?;
                Err(error)
            }
            Err(restore_error) => Err(CodexSwitchError::RecoveryRequired {
                reason: format!(
                    "captured Codex file did not match the expected fingerprint and could not be restored without replacing another writer: {restore_error}"
                ),
            }),
        };
    }
    Ok(())
}

fn commit_staged_file(
    stage_path: &Path,
    path: &Path,
    expectation: FileCommitExpectation<'_>,
) -> Result<(), CodexSwitchError> {
    let Some(capture_path) = managed_commit_capture_path(path, expectation)? else {
        verify_file_before_commit(path, expectation)?;
        replace_file(stage_path, path)?;
        return sync_parent_directory(path);
    };

    if expectation.expected_present() {
        capture_expected_file(path, capture_path.as_path(), expectation)?;
    } else {
        verify_file_before_commit(path, expectation)?;
    }
    if let Err(error) = move_file_no_replace(stage_path, path) {
        return Err(CodexSwitchError::RecoveryRequired {
            reason: format!(
                "Codex file changed while the helper was publishing its prepared replacement; the competing file and recovery capture were preserved: {error}"
            ),
        });
    }
    sync_parent_directory(path)
}

fn commit_remove_file(
    path: &Path,
    expectation: FileCommitExpectation<'_>,
) -> Result<(), CodexSwitchError> {
    let Some(capture_path) = managed_commit_capture_path(path, expectation)? else {
        verify_file_before_commit(path, expectation)?;
        return remove_file_durable(path);
    };
    if expectation.expected_present() {
        capture_expected_file(path, capture_path.as_path(), expectation)?;
    } else {
        verify_file_before_commit(path, expectation)?;
    }
    sync_parent_directory(path)
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), CodexSwitchError> {
    move_file_write_through(source, destination, true)
}

#[cfg(windows)]
fn move_file_no_replace(source: &Path, destination: &Path) -> Result<(), CodexSwitchError> {
    move_file_write_through(source, destination, false)
}

#[cfg(unix)]
fn unix_path_cstring(path: &Path) -> Result<std::ffi::CString, CodexSwitchError> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|source| io_error("encode Unix path for", path, source.into()))
}

#[cfg(target_vendor = "apple")]
fn move_file_no_replace(source: &Path, destination: &Path) -> Result<(), CodexSwitchError> {
    let source_c = unix_path_cstring(source)?;
    let destination_c = unix_path_cstring(destination)?;
    // SAFETY: Both C strings are null-terminated and remain alive for the call.
    let result =
        unsafe { libc::renamex_np(source_c.as_ptr(), destination_c.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io_error(
            "move without replacing",
            destination,
            std::io::Error::last_os_error(),
        ))
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn move_file_no_replace(source: &Path, destination: &Path) -> Result<(), CodexSwitchError> {
    let source_c = unix_path_cstring(source)?;
    let destination_c = unix_path_cstring(destination)?;
    // SAFETY: Both C strings are null-terminated and remain alive for the call.
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source_c.as_ptr(),
            libc::AT_FDCWD,
            destination_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io_error(
            "move without replacing",
            destination,
            std::io::Error::last_os_error(),
        ))
    }
}

#[cfg(all(
    unix,
    not(target_vendor = "apple"),
    not(any(target_os = "linux", target_os = "android"))
))]
fn move_file_no_replace(_source: &Path, destination: &Path) -> Result<(), CodexSwitchError> {
    Err(io_error(
        "move without replacing",
        destination,
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "this platform has no supported no-replace rename primitive",
        ),
    ))
}

#[cfg(windows)]
fn windows_path_wide(path: &Path) -> std::io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;

    let encoded = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if encoded[..encoded.len().saturating_sub(1)].contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path contains an embedded null",
        ));
    }
    Ok(encoded)
}

#[cfg(windows)]
fn windows_file_operation_is_retryable(error: &std::io::Error) -> bool {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
    };

    matches!(
        error.raw_os_error(),
        Some(code)
            if code == ERROR_ACCESS_DENIED as i32
                || code == ERROR_SHARING_VIOLATION as i32
                || code == ERROR_LOCK_VIOLATION as i32
    )
}

#[cfg(windows)]
fn retry_windows_file_operation(
    mut operation: impl FnMut() -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut backoff = std::time::Duration::from_millis(1);
    for attempt in 0..WINDOWS_FILE_OPERATION_ATTEMPTS {
        match operation() {
            Ok(()) => return Ok(()),
            Err(error)
                if windows_file_operation_is_retryable(&error)
                    && attempt + 1 < WINDOWS_FILE_OPERATION_ATTEMPTS =>
            {
                std::thread::sleep(backoff);
                backoff = std::cmp::min(
                    backoff.saturating_mul(2),
                    WINDOWS_FILE_OPERATION_MAX_BACKOFF,
                );
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the bounded Windows file-operation loop returns on its final attempt")
}

#[cfg(windows)]
fn move_file_write_through(
    source: &Path,
    destination: &Path,
    replace_existing: bool,
) -> Result<(), CodexSwitchError> {
    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source_wide = windows_path_wide(source)
        .map_err(|error| io_error("encode Windows path for", source, error))?;
    let destination_wide = windows_path_wide(destination)
        .map_err(|source| io_error("encode Windows path for", destination, source))?;
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace_existing {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
    retry_windows_file_operation(|| {
        // SAFETY: Both encoded paths are null-terminated and live for the duration of the call.
        let moved = unsafe { MoveFileExW(source_wide.as_ptr(), destination_wide.as_ptr(), flags) };
        if moved != 0 {
            return Ok(());
        }
        let move_error = std::io::Error::last_os_error();
        if replace_existing && move_error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) {
            std::fs::rename(source, destination)
        } else {
            Err(move_error)
        }
    })
    .map_err(|source| io_error("move with write-through", destination, source))
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), CodexSwitchError> {
    std::fs::rename(source, destination)
        .map_err(|source| io_error("atomically replace", destination, source))
}

#[cfg(not(windows))]
fn remove_file_durable(path: &Path) -> Result<(), CodexSwitchError> {
    match std::fs::remove_file(path) {
        Ok(()) => sync_parent_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error("remove", path, source)),
    }
}

#[cfg(any(windows, test))]
fn pending_delete_path(path: &Path) -> Result<PathBuf, CodexSwitchError> {
    let parent = path.parent().ok_or_else(|| {
        io_error(
            "resolve parent for",
            path,
            std::io::Error::other("path has no parent directory"),
        )
    })?;
    path.file_name().ok_or_else(|| {
        io_error(
            "resolve file name for",
            path,
            std::io::Error::other("path has no file name"),
        )
    })?;
    Ok(parent.join(format!(
        "{SWITCH_DELETE_TOMBSTONE_PREFIX}{}",
        Uuid::new_v4()
    )))
}

fn ensure_pending_delete_destination_absent(path: &Path) -> Result<(), CodexSwitchError> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error(
            "inspect pending durable deletion destination",
            path,
            source,
        )),
        Ok(_) => Err(io_error(
            "reserve pending durable deletion destination",
            path,
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "managed deletion tombstone already exists",
            ),
        )),
    }
}

#[cfg(windows)]
fn remove_file_with_windows_retry(
    path: &Path,
    action: &'static str,
) -> Result<(), CodexSwitchError> {
    retry_windows_file_operation(|| match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    })
    .map_err(|source| io_error(action, path, source))
}

#[cfg(windows)]
fn remove_file_durable(path: &Path) -> Result<(), CodexSwitchError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error("inspect before durable removal of", path, source)),
    }
    let tombstone = pending_delete_path(path)?;
    ensure_pending_delete_destination_absent(tombstone.as_path())?;
    move_file_write_through(path, tombstone.as_path(), false)?;
    remove_file_with_windows_retry(tombstone.as_path(), "finish pending durable deletion for")
}

fn managed_switch_artifact_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
        && Uuid::parse_str(value).is_ok()
}

fn managed_switch_artifact_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    if name
        .strip_prefix(SWITCH_DELETE_TOMBSTONE_PREFIX)
        .is_some_and(managed_switch_artifact_uuid)
    {
        return true;
    }
    [SWITCH_TEMP_FILE_PREFIX, LEGACY_SWITCH_TEMP_FILE_PREFIX]
        .into_iter()
        .any(|prefix| {
            name.strip_prefix(prefix)
                .and_then(|suffix| suffix.strip_suffix(SWITCH_TEMP_FILE_SUFFIX))
                .is_some_and(managed_switch_artifact_uuid)
        })
}

fn remove_managed_switch_artifact(path: &Path) -> Result<(), CodexSwitchError> {
    #[cfg(windows)]
    {
        remove_file_with_windows_retry(path, "remove stale managed switch artifact")
    }
    #[cfg(not(windows))]
    {
        match std::fs::remove_file(path) {
            Ok(()) => sync_parent_directory(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(io_error(
                "remove stale managed switch artifact",
                path,
                source,
            )),
        }
    }
}

fn cleanup_managed_switch_artifacts(paths: &SwitchPaths) -> Result<(), CodexSwitchError> {
    let mut parents = Vec::<PathBuf>::new();
    for path in [
        paths.config.as_path(),
        paths.auth.as_path(),
        paths.state.as_path(),
        paths.unscoped_state.as_path(),
        paths.legacy_state.as_path(),
    ] {
        if let Some(parent) = path.parent()
            && !parents.iter().any(|known| known == parent)
        {
            parents.push(parent.to_path_buf());
        }
    }
    for parent in parents {
        let entries = match std::fs::read_dir(parent.as_path()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(io_error(
                    "scan managed switch artifacts in",
                    &parent,
                    source,
                ));
            }
        };
        for entry in entries {
            let entry = entry.map_err(|source| {
                io_error("read managed switch artifact entry in", &parent, source)
            })?;
            if !managed_switch_artifact_name(&entry.file_name()) {
                continue;
            }
            let file_type = entry.file_type().map_err(|source| {
                io_error("inspect managed switch artifact", &entry.path(), source)
            })?;
            if file_type.is_file() && !file_type.is_symlink() {
                remove_managed_switch_artifact(entry.path().as_path())?;
            }
        }
    }
    Ok(())
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
    #[cfg(windows)]
    crate::local_operator::secure_private_windows_path(parent, true).map_err(|error| {
        io_error(
            "secure directory",
            parent,
            std::io::Error::other(error.to_string()),
        )
    })?;
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
    auth: PathBuf,
    config_fingerprint: String,
    state: PathBuf,
    unscoped_state: PathBuf,
    lock: PathBuf,
    legacy_state: PathBuf,
}

impl SwitchPaths {
    fn resolve() -> Result<Self, CodexSwitchError> {
        Self::resolve_for_codex_home(crate::config::codex_home().as_path())
    }

    fn resolve_for_codex_home(codex_home: &Path) -> Result<Self, CodexSwitchError> {
        let state_dir = crate::config::proxy_home_dir().join("state");
        let state_dir = absolute_path(state_dir.as_path())?;
        let state_dir = resolve_existing_ancestor(state_dir.as_path())?;
        let config = resolve_config_path(codex_home.join("config.toml").as_path())?;
        let config_fingerprint = config_path_fingerprint(config.as_path());
        let legacy_state = config.with_file_name(LEGACY_STATE_FILE_NAME);
        let auth = config.with_file_name("auth.json");
        Ok(Self {
            state: state_dir.join(scoped_state_file_name(config_fingerprint.as_str())),
            unscoped_state: state_dir.join(UNSCOPED_STATE_FILE_NAME),
            config_fingerprint,
            config,
            auth,
            lock: state_dir.join(LOCK_FILE_NAME),
            legacy_state,
        })
    }
}

fn scoped_state_file_name(config_fingerprint: &str) -> String {
    format!("{SCOPED_STATE_FILE_PREFIX}{config_fingerprint}{SCOPED_STATE_FILE_SUFFIX}")
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

    use crate::auth_resolution::CodexAuthModeMetadata;

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
            self.state_path_for_home(self.codex_home.as_path())
        }

        fn state_path_for_home(&self, codex_home: &Path) -> PathBuf {
            let config = resolve_config_path(codex_home.join("config.toml").as_path())
                .expect("resolve test Codex config path");
            let config_fingerprint = config_path_fingerprint(config.as_path());
            self.resolved_state_dir()
                .join(scoped_state_file_name(config_fingerprint.as_str()))
        }

        fn unscoped_state_path(&self) -> PathBuf {
            self.resolved_state_dir().join(UNSCOPED_STATE_FILE_NAME)
        }

        fn resolved_state_dir(&self) -> PathBuf {
            let state_dir = absolute_path(self.helper_home.join("state").as_path())
                .expect("resolve absolute test helper state path");
            resolve_existing_ancestor(state_dir.as_path())
                .expect("resolve canonical test helper state path")
        }

        fn legacy_state_path(&self) -> PathBuf {
            self.codex_home.join(LEGACY_STATE_FILE_NAME)
        }

        fn auth_path(&self) -> PathBuf {
            self.codex_home.join("auth.json")
        }

        fn auth_backup_path(&self) -> PathBuf {
            let journal = read_journal(self.state_path().as_path())
                .expect("read switch journal")
                .expect("switch journal");
            let name = journal
                .auth_patch
                .and_then(|auth| auth.backup_file_name)
                .expect("auth backup file name");
            self.helper_home.join("state").join(name)
        }

        fn auth_backup_files(&self) -> Vec<PathBuf> {
            let state_dir = self.helper_home.join("state");
            let Ok(entries) = std::fs::read_dir(state_dir) else {
                return Vec::new();
            };
            entries
                .map(|entry| entry.expect("read state entry"))
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(managed_auth_backup_name)
                })
                .map(|entry| entry.path())
                .collect()
        }

        fn helper_stanza_backup_path(&self) -> PathBuf {
            let journal = read_journal(self.state_path().as_path())
                .expect("read switch journal")
                .expect("switch journal");
            let name = journal
                .helper_stanza_backup
                .and_then(|helper_stanza| helper_stanza.backup_file_name)
                .expect("helper stanza backup file name");
            self.helper_home.join("state").join(name)
        }

        fn helper_stanza_backup_files(&self) -> Vec<PathBuf> {
            let state_dir = self.helper_home.join("state");
            let Ok(entries) = std::fs::read_dir(state_dir) else {
                return Vec::new();
            };
            entries
                .map(|entry| entry.expect("read state entry"))
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(managed_helper_stanza_backup_name)
                })
                .map(|entry| entry.path())
                .collect()
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

        fn write_legacy_state(&self, value: serde_json::Value) {
            std::fs::write(
                self.legacy_state_path(),
                serde_json::to_vec_pretty(&value).expect("serialize legacy state"),
            )
            .expect("write legacy state");
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

    fn acquire_test_ephemeral_switch(port: u16) -> Result<CodexSwitchOutcome, CodexSwitchError> {
        acquire_ephemeral_with_client_patch_and_failpoint(
            ValidatedCodexBaseUrl::local(port),
            CodexClientPatchConfig::default(),
            ApplyFailpoint::None,
            Uuid::new_v4().to_string(),
        )
    }

    fn chatgpt_auth_json_for_switch_tests() -> String {
        use base64::Engine as _;

        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            br#"{"email":"user@example.com","https://api.openai.com/auth":{"chatgpt_account_id":"acct_test"}}"#,
        );
        serde_json::to_string_pretty(&serde_json::json!({
            "auth_mode": "apikey",
            "OPENAI_API_KEY": "private-api-key",
            "tokens": {
                "id_token": format!("header.{payload}.signature"),
                "access_token": "access-secret",
                "refresh_token": "refresh-secret",
                "account_id": "acct_test"
            },
            "last_refresh": "2026-07-19T00:00:00Z"
        }))
        .expect("serialize ChatGPT auth fixture")
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
    fn unchanged_switch_on_does_not_claim_restore_ownership() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");

        let first = apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("first switch on");
        let second = apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("second switch on");

        assert_eq!(first.change, CodexSwitchChange::Applied);
        assert!(first.restore_lease.is_none());
        assert_eq!(second.change, CodexSwitchChange::Unchanged);
        assert!(second.restore_lease.is_none());
    }

    #[test]
    fn explicit_same_target_switch_on_invalidates_ephemeral_restore_ownership() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let ephemeral = acquire_test_ephemeral_switch(3211).expect("acquire ephemeral switch");
        let lease = ephemeral
            .restore_lease
            .expect("ephemeral switch owns restore");

        let persistent = apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("explicit persistent switch on");

        assert_eq!(persistent.change, CodexSwitchChange::Unchanged);
        assert!(persistent.restore_lease.is_none());
        assert!(
            restore_if_owned(&lease)
                .expect("old lease restore check")
                .is_none()
        );
        assert!(inspect().expect("inspect persistent switch").enabled);
    }

    #[test]
    fn ephemeral_switch_does_not_claim_an_existing_persistent_generation() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("explicit persistent switch on");

        let ephemeral =
            acquire_test_ephemeral_switch(3211).expect("observe existing persistent switch");

        assert_eq!(ephemeral.change, CodexSwitchChange::Unchanged);
        assert!(ephemeral.restore_lease.is_none());
        assert!(inspect().expect("inspect persistent switch").enabled);
    }

    #[test]
    fn failed_ephemeral_apply_compensates_owned_config_and_auth_writes() {
        for failpoint in [
            ApplyFailpoint::AfterPrepared,
            ApplyFailpoint::AfterConfigWrite,
            ApplyFailpoint::AfterAuthWrite,
        ] {
            let env = TestEnvironment::new();
            let original_config = "# keep config\nmodel_provider = \"openai\"\n";
            let original_auth = r#"{"OPENAI_API_KEY":"original-secret"}"#;
            env.write_config(original_config);
            std::fs::write(env.auth_path(), original_auth).expect("write original auth");

            let error = acquire_ephemeral_with_client_patch_and_failpoint(
                ValidatedCodexBaseUrl::local(3211),
                CodexClientPatchConfig {
                    preset: CodexClientPreset::OfficialImagegen,
                    ..CodexClientPatchConfig::default()
                },
                failpoint,
                Uuid::new_v4().to_string(),
            )
            .expect_err("injected ephemeral apply failure");

            assert!(matches!(error, CodexSwitchError::InjectedFailure(_)));
            assert_eq!(env.read_config(), original_config);
            assert_eq!(
                std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
                original_auth
            );
            assert!(!inspect().expect("inspect compensated switch").enabled);
        }
    }

    #[test]
    fn newer_ephemeral_generation_takes_over_a_released_runtime_switch() {
        let env = TestEnvironment::new();
        let original = "# original\nmodel_provider = \"openai\"\n";
        env.write_config(original);
        let first = acquire_test_ephemeral_switch(3211).expect("first ephemeral switch");
        let first_lease = first.restore_lease.expect("first restore lease");

        let second = acquire_test_ephemeral_switch(3211).expect("second ephemeral switch");
        let second_lease = second.restore_lease.expect("second restore lease");

        assert!(
            restore_if_owned(&first_lease)
                .expect("stale restore check")
                .is_none()
        );
        assert!(inspect().expect("inspect active takeover").enabled);
        assert!(
            restore_if_owned(&second_lease)
                .expect("current restore")
                .is_some()
        );
        assert_eq!(env.read_config(), original);
    }

    #[test]
    fn owned_restore_retries_transient_operation_lock_contention() {
        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n";
        env.write_config(original);
        let applied = acquire_test_ephemeral_switch(3211).expect("ephemeral switch");
        let lease = applied.restore_lease.expect("restore lease");
        let operation_lock =
            OperationLock::acquire(env.lock_path().as_path()).expect("hold switch operation lock");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(35));
            drop(operation_lock);
        });

        let restored = restore_if_owned_with_retry(&lease)
            .expect("retry owned restore")
            .expect("generation remains owned");
        release.join().expect("release operation lock");

        assert_eq!(restored.change, CodexSwitchChange::Removed);
        assert_eq!(env.read_config(), original);
    }

    #[test]
    fn owned_restore_skips_a_newer_switch_generation() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let first = acquire_test_ephemeral_switch(3211).expect("first switch on");
        let lease = first.restore_lease.expect("first switch owns restore");

        apply(CodexSwitchIntent::Off).expect("user switch off");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("user switch on again");
        let config_after_user_switch = std::fs::read(env.config_path()).expect("read config");
        let state_after_user_switch = std::fs::read(env.state_path()).expect("read state");

        assert!(
            restore_if_owned(&lease)
                .expect("old lease restore check")
                .is_none()
        );
        assert_eq!(
            std::fs::read(env.config_path()).expect("read preserved config"),
            config_after_user_switch
        );
        assert_eq!(
            std::fs::read(env.state_path()).expect("read preserved state"),
            state_after_user_switch
        );
    }

    #[test]
    fn owned_restore_removes_its_unchanged_generation() {
        let env = TestEnvironment::new();
        let original = "# keep\nmodel_provider = \"openai\"\n";
        env.write_config(original);
        let applied = acquire_test_ephemeral_switch(3211).expect("switch on");
        let lease = applied.restore_lease.expect("switch owns restore");

        let restored = restore_if_owned(&lease)
            .expect("restore owned switch")
            .expect("generation still owned");

        assert_eq!(restored.change, CodexSwitchChange::Removed);
        assert_eq!(env.read_config(), original);
        assert!(!env.state_path().exists());
    }

    #[test]
    fn owned_restore_merges_unmanaged_codex_config_writeback() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let applied = acquire_test_ephemeral_switch(3211).expect("switch on");
        let lease = applied.restore_lease.expect("switch owns restore");
        let mut edited = env.read_config();
        edited.push_str(
            "\n# written by Codex\n[projects.\"/external\"]\ntrust_level = \"trusted\"\n",
        );
        env.write_config(edited.as_str());

        let restored = restore_if_owned(&lease)
            .expect("restore owned switch")
            .expect("generation still owned");

        assert_eq!(restored.change, CodexSwitchChange::Removed);
        let restored = env.read_config();
        assert!(restored.contains("model_provider = \"openai\""));
        assert!(restored.contains("# written by Codex"));
        assert!(restored.contains("[projects.\"/external\"]"));
        assert!(restored.contains("trust_level = \"trusted\""));
        assert!(!restored.contains("model_providers.codex_proxy"));
        assert!(!env.state_path().exists());
    }

    #[test]
    fn explicit_client_patches_expose_only_the_requested_codex_capabilities() {
        for (client_patch, provider_name, exposes_tools) in [
            (CodexClientPatchConfig::default(), "codex-helper", false),
            (
                CodexClientPatchConfig {
                    preset: CodexClientPreset::OfficialRelay,
                    ..CodexClientPatchConfig::default()
                },
                "OpenAI",
                false,
            ),
            (
                CodexClientPatchConfig {
                    preset: CodexClientPreset::OfficialImagegen,
                    ..CodexClientPatchConfig::default()
                },
                "OpenAI",
                true,
            ),
        ] {
            let env = TestEnvironment::new();
            let original = "model_provider = \"openai\"\n";
            env.write_config(original);
            std::fs::write(env.auth_path(), b"auth sentinel").expect("write auth sentinel");

            let outcome = apply_with_client_patch(
                CodexSwitchIntent::On {
                    validated_base_url: ValidatedCodexBaseUrl::local(3211),
                },
                client_patch,
            )
            .expect("switch on with explicit client patch");

            assert_eq!(outcome.status.client_patch, Some(client_patch));
            let applied = env.read_config();
            assert!(applied.contains(format!("name = \"{provider_name}\"").as_str()));
            assert!(applied.contains("[model_providers.codex_proxy.http_headers]"));
            assert!(applied.contains(CODEX_CLIENT_RUNTIME_PATCH_HEADER));
            assert_eq!(
                applied.contains(CODEX_CLIENT_FACADE_ACTOR_HEADER),
                exposes_tools
            );
            assert_eq!(
                applied.contains(CODEX_CLIENT_FACADE_ACTOR_VALUE),
                exposes_tools
            );
            assert!(!applied.contains("requires_openai_auth"));
            let applied_auth =
                std::fs::read(env.auth_path()).expect("read applied auth projection");
            if exposes_tools {
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&applied_auth)
                        .expect("parse imagegen auth facade"),
                    serde_json::json!({})
                );
            } else {
                assert_eq!(applied_auth, b"auth sentinel");
            }

            apply(CodexSwitchIntent::Off).expect("switch off client patch");
            assert_eq!(env.read_config(), original);
            assert_eq!(
                std::fs::read(env.auth_path()).expect("read restored auth sentinel"),
                b"auth sentinel"
            );
        }
    }

    #[test]
    fn client_patch_projection_round_trips_owned_provider_and_feature_keys() {
        let env = TestEnvironment::new();
        let original = r#"# preserve formatting
model_provider = "openai"

[features]
remote_compaction_v2 = true
image_generation = true
unrelated_feature = true
"#;
        env.write_config(original);
        std::fs::write(env.auth_path(), b"auth sentinel").expect("write auth sentinel");
        let patch = crate::config::CodexClientPatchConfig {
            preset: crate::config::CodexClientPreset::OfficialImagegen,
            responses_websocket: true,
            compaction: crate::config::CodexCompactionStrategy::RemoteV1,
            hosted_image_generation: crate::config::CodexHostedImageGenerationMode::Disabled,
            ..crate::config::CodexClientPatchConfig::default()
        };

        let outcome = apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("apply complete client patch");

        assert_eq!(outcome.status.client_patch.as_ref(), Some(&patch));
        let applied = env.read_config();
        assert!(applied.contains("name = \"OpenAI\""));
        assert!(applied.contains("supports_websockets = true"));
        assert!(applied.contains("remote_compaction_v2 = false"));
        assert!(applied.contains("image_generation = false"));
        assert!(applied.contains("unrelated_feature = true"));
        assert!(!applied.contains(CODEX_CLIENT_FACADE_ACTOR_VALUE));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(env.auth_path()).expect("read applied auth facade")
            )
            .expect("parse applied auth facade"),
            serde_json::json!({})
        );
        let journal = std::fs::read_to_string(env.state_path()).expect("read switch journal");
        assert!(journal.contains("official-imagegen"));
        assert!(journal.contains("remote-v1"));
        assert!(!journal.contains("auth sentinel"));

        apply(CodexSwitchIntent::Off).expect("restore complete client patch");
        assert_eq!(env.read_config(), original);
        assert_eq!(
            std::fs::read(env.auth_path()).expect("read restored auth sentinel"),
            b"auth sentinel"
        );
    }

    #[test]
    fn imagegen_presets_install_an_empty_auth_facade_and_restore_the_exact_original() {
        for preset in [
            crate::config::CodexClientPreset::ImagegenBridge,
            crate::config::CodexClientPreset::OfficialImagegen,
        ] {
            let env = TestEnvironment::new();
            let original_config = "model_provider = \"openai\"\n";
            let original_auth = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"private-auth-canary"}"#;
            env.write_config(original_config);
            std::fs::write(env.auth_path(), original_auth).expect("write original auth");
            let patch = crate::config::CodexClientPatchConfig {
                preset,
                ..crate::config::CodexClientPatchConfig::default()
            };

            apply_with_client_patch(
                CodexSwitchIntent::On {
                    validated_base_url: ValidatedCodexBaseUrl::local(3211),
                },
                patch,
            )
            .expect("apply imagegen auth facade");

            let applied_auth = std::fs::read_to_string(env.auth_path()).expect("read auth facade");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&applied_auth)
                    .expect("parse auth facade"),
                serde_json::json!({})
            );
            let journal = std::fs::read_to_string(env.state_path()).expect("read switch journal");
            assert!(!journal.contains("private-auth-canary"));

            apply(CodexSwitchIntent::Off).expect("restore imagegen auth facade");
            assert_eq!(env.read_config(), original_config);
            assert_eq!(
                std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
                original_auth
            );
        }
    }

    #[test]
    fn imagegen_facade_preserves_a_formatted_empty_auth_file_as_present() {
        let env = TestEnvironment::new();
        let original_config = "model_provider = \"openai\"\n";
        let original_auth = "{\n}\n";
        env.write_config(original_config);
        std::fs::write(env.auth_path(), original_auth).expect("write formatted empty auth");

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            CodexClientPatchConfig {
                preset: CodexClientPreset::OfficialImagegen,
                ..CodexClientPatchConfig::default()
            },
        )
        .expect("apply imagegen facade over formatted empty auth");

        apply(CodexSwitchIntent::Off).expect("restore formatted empty auth");
        assert_eq!(env.read_config(), original_config);
        assert!(env.auth_path().exists());
        assert_eq!(
            std::fs::read(env.auth_path()).expect("read restored formatted empty auth"),
            original_auth.as_bytes()
        );
    }

    #[test]
    fn original_projection_reads_through_an_applied_auth_facade_without_writing_files() {
        let env = TestEnvironment::new();
        let original_config = r#"model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1"
env_key = "RELAY_API_KEY"
requires_openai_auth = false
"#;
        let original_auth = r#"{"auth_mode":"apikey","RELAY_API_KEY":"private-projection-canary"}"#;
        env.write_config(original_config);
        std::fs::write(env.auth_path(), original_auth).expect("write original auth");
        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            CodexClientPatchConfig {
                preset: CodexClientPreset::OfficialImagegen,
                ..CodexClientPatchConfig::default()
            },
        )
        .expect("apply auth facade");

        let config_before = std::fs::read(env.config_path()).expect("read applied config");
        let auth_before = std::fs::read(env.auth_path()).expect("read applied auth");
        let journal_before = std::fs::read(env.state_path()).expect("read applied journal");
        let (provider_id, base_url, metadata) = project_original_codex_config(|config, auth| {
            let value = toml::from_str::<TomlValue>(config.expect("original config projection"))
                .expect("parse original projection");
            let provider_id = value["model_provider"]
                .as_str()
                .expect("active provider")
                .to_string();
            let base_url = value["model_providers"][provider_id.as_str()]["base_url"]
                .as_str()
                .expect("active provider base URL")
                .to_string();
            (provider_id, base_url, auth)
        })
        .expect("project original Codex files");

        assert_eq!(provider_id, "relay");
        assert_eq!(base_url, "https://relay.example/v1");
        assert_eq!(metadata.mode, Some(CodexAuthModeMetadata::ApiKey));
        assert_eq!(metadata.unique_api_key_field(), Some("RELAY_API_KEY"));
        assert!(!format!("{metadata:?}").contains("private-projection-canary"));
        assert_eq!(
            std::fs::read(env.config_path()).expect("reread applied config"),
            config_before
        );
        assert_eq!(
            std::fs::read(env.auth_path()).expect("reread applied auth"),
            auth_before
        );
        assert_eq!(
            std::fs::read(env.state_path()).expect("reread applied journal"),
            journal_before
        );
    }

    #[test]
    fn onboarding_projection_recovers_v0203_state_before_reading_original_files() {
        let env = TestEnvironment::new();
        let original_config = r#"# preserve the original relay
model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1"
env_key = "RELAY_API_KEY"
requires_openai_auth = false
"#;
        let applied_config = format!(
            "{}\n[model_providers.codex_proxy]\nname = \"codex-helper\"\nbase_url = \"http://127.0.0.1:3211/v1\"\nwire_api = \"responses\"\nrequest_max_retries = 0\n",
            original_config.replace(
                "model_provider = \"relay\"",
                "model_provider = \"codex_proxy\""
            )
        );
        let original_auth = r#"{"auth_mode":"apikey","RELAY_API_KEY":"private-legacy-canary"}"#;
        env.write_config(&applied_config);
        std::fs::write(env.auth_path(), original_auth).expect("write original auth");
        env.write_legacy_state(serde_json::json!({
            "version": 2,
            "original_config_absent": false,
            "original_model_provider": "relay",
            "original_codex_proxy": null,
            "had_model_providers": true
        }));

        let (provider_id, base_url, auth) =
            project_original_codex_config_for_onboarding(|config, auth| {
                let value = toml::from_str::<TomlValue>(
                    config.expect("recovered original config projection"),
                )
                .expect("parse recovered original projection");
                let provider_id = value["model_provider"]
                    .as_str()
                    .expect("active provider")
                    .to_string();
                let base_url = value["model_providers"][provider_id.as_str()]["base_url"]
                    .as_str()
                    .expect("active provider base URL")
                    .to_string();
                (provider_id, base_url, auth)
            })
            .expect("recover and project v0.20.3 switch state");

        assert_eq!(provider_id, "relay");
        assert_eq!(base_url, "https://relay.example/v1");
        assert_eq!(auth.mode, Some(CodexAuthModeMetadata::ApiKey));
        assert_eq!(auth.unique_api_key_field(), Some("RELAY_API_KEY"));
        assert!(!format!("{auth:?}").contains("private-legacy-canary"));
        assert_eq!(env.read_config(), original_config);
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read preserved auth"),
            original_auth
        );
        assert!(!env.legacy_state_path().exists());
        assert!(!env.state_path().exists());
    }

    #[test]
    fn chatgpt_bridge_validates_and_restores_the_complete_login_object() {
        let env = TestEnvironment::new();
        let original_config = "model_provider = \"openai\"\n";
        let original_auth = chatgpt_auth_json_for_switch_tests();
        env.write_config(original_config);
        std::fs::write(env.auth_path(), &original_auth).expect("write ChatGPT auth");
        let patch = crate::config::CodexClientPatchConfig {
            preset: crate::config::CodexClientPreset::ChatGptBridge,
            ..crate::config::CodexClientPatchConfig::default()
        };

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("apply ChatGPT bridge auth patch");

        let applied = serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(env.auth_path()).expect("read patched ChatGPT auth"),
        )
        .expect("parse patched ChatGPT auth");
        assert_eq!(applied["auth_mode"], "chatgpt");
        assert!(applied["OPENAI_API_KEY"].is_null());
        assert_eq!(applied["tokens"]["access_token"], "access-secret");
        let journal = std::fs::read_to_string(env.state_path()).expect("read switch journal");
        assert!(!journal.contains("access-secret"));

        apply(CodexSwitchIntent::Off).expect("restore ChatGPT bridge auth patch");
        assert_eq!(env.read_config(), original_config);
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored ChatGPT auth"),
            original_auth
        );
    }

    #[test]
    fn chatgpt_bridge_rejects_missing_login_without_mutating_config_or_state() {
        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n";
        env.write_config(original);
        let patch = crate::config::CodexClientPatchConfig {
            preset: crate::config::CodexClientPreset::ChatGptBridge,
            ..crate::config::CodexClientPatchConfig::default()
        };

        let error = apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect_err("missing ChatGPT login must fail");

        assert!(error.to_string().contains("auth.json"));
        assert_eq!(env.read_config(), original);
        assert!(!env.auth_path().exists());
        assert!(!env.state_path().exists());
    }

    #[test]
    fn imagegen_facade_removes_auth_created_for_an_absent_original() {
        let env = TestEnvironment::new();
        let original_config = "model_provider = \"openai\"\n";
        env.write_config(original_config);
        let patch = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("apply imagegen facade without original auth");

        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(env.auth_path()).expect("read created auth facade")
            )
            .expect("parse created auth facade"),
            serde_json::json!({})
        );
        assert!(env.auth_backup_files().is_empty());

        apply(CodexSwitchIntent::Off).expect("remove created auth facade");
        assert_eq!(env.read_config(), original_config);
        assert!(!env.auth_path().exists());
        let status = inspect().expect("inspect retained absent-auth recovery point");
        assert_eq!(status.phase, CodexSwitchPhase::Off);
        assert!(!status.enabled);
        assert!(status.managed);
        assert!(env.state_path().exists());
        assert!(env.auth_backup_files().is_empty());

        std::fs::write(env.auth_path(), "{}\n").expect("simulate a stale Codex facade write");
        assert_eq!(
            inspect().expect("inspect stale facade write").phase,
            CodexSwitchPhase::RecoveryRequired
        );
        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("remove stale facade using retained recovery point")
                .change,
            CodexSwitchChange::Recovered
        );
        assert!(!env.auth_path().exists());
        assert_eq!(
            inspect().expect("inspect repaired off state").phase,
            CodexSwitchPhase::Off
        );
    }

    #[test]
    fn reapply_from_imagegen_to_chatgpt_uses_the_saved_original_login() {
        let env = TestEnvironment::new();
        let original_config = "model_provider = \"openai\"\n";
        let original_auth = chatgpt_auth_json_for_switch_tests();
        env.write_config(original_config);
        std::fs::write(env.auth_path(), &original_auth).expect("write original ChatGPT auth");
        let target = ValidatedCodexBaseUrl::local(3211);
        let imagegen = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };
        let chatgpt = CodexClientPatchConfig {
            preset: CodexClientPreset::ChatGptBridge,
            ..CodexClientPatchConfig::default()
        };

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: target.clone(),
            },
            imagegen,
        )
        .expect("apply imagegen facade");
        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: target,
            },
            chatgpt,
        )
        .expect("reapply ChatGPT bridge from saved original");

        let applied = serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(env.auth_path()).expect("read reapplied ChatGPT auth"),
        )
        .expect("parse reapplied ChatGPT auth");
        assert_eq!(applied["tokens"]["access_token"], "access-secret");
        assert_eq!(applied["auth_mode"], "chatgpt");
        assert!(applied["OPENAI_API_KEY"].is_null());

        apply(CodexSwitchIntent::Off).expect("restore original after reapply");
        assert_eq!(env.read_config(), original_config);
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored original auth"),
            original_auth
        );
        assert_eq!(
            inspect().expect("inspect retained recovery point").phase,
            CodexSwitchPhase::Off
        );
        assert_eq!(env.auth_backup_files().len(), 1);
    }

    #[test]
    fn retained_off_recovery_restores_auth_after_a_stale_facade_write() {
        let env = TestEnvironment::new();
        let original_config = "model_provider = \"openai\"\n";
        let original_auth = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"original-secret"}"#;
        env.write_config(original_config);
        std::fs::write(env.auth_path(), original_auth).expect("write original auth");
        let patch = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("apply auth facade");
        apply(CodexSwitchIntent::Off).expect("restore original auth");
        let backup = env.auth_backup_path();
        assert_eq!(
            inspect().expect("inspect off state").phase,
            CodexSwitchPhase::Off
        );

        std::fs::write(env.auth_path(), "{}\n").expect("simulate a stale Codex facade write");
        assert_eq!(
            inspect().expect("inspect stale facade write").phase,
            CodexSwitchPhase::RecoveryRequired
        );
        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("repair stale facade from retained recovery")
                .change,
            CodexSwitchChange::Recovered
        );
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read repaired auth"),
            original_auth
        );
        assert!(backup.exists());
        assert_eq!(
            inspect().expect("inspect repaired off state").phase,
            CodexSwitchPhase::Off
        );
    }

    #[test]
    fn retained_auth_backup_survives_a_new_prepared_switch() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let original_auth = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"original-secret"}"#;
        std::fs::write(env.auth_path(), original_auth).expect("write original auth");
        let patch = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };

        apply_with_client_patch(on.clone(), patch).expect("apply first auth facade");
        apply(CodexSwitchIntent::Off).expect("retain the first recovery point");
        let retained_backup = env.auth_backup_path();
        assert!(retained_backup.exists());

        assert!(matches!(
            apply_with_client_patch_and_failpoint(on.clone(), patch, ApplyFailpoint::AfterPrepared),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));
        assert_eq!(env.auth_backup_path(), retained_backup);
        assert_eq!(env.auth_backup_files(), vec![retained_backup.clone()]);

        assert_eq!(
            apply_with_client_patch(on, patch)
                .expect("resume the new switch from the retained backup")
                .change,
            CodexSwitchChange::Applied
        );
        assert_eq!(env.auth_backup_path(), retained_backup);
        assert_eq!(env.auth_backup_files(), vec![retained_backup]);
        apply(CodexSwitchIntent::Off).expect("restore after resumed switch");
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
            original_auth
        );
    }

    #[test]
    fn preserving_reapply_retains_auth_recovery_across_durable_boundaries() {
        for failpoint in [
            ApplyFailpoint::AfterPrepared,
            ApplyFailpoint::AfterConfigWrite,
        ] {
            let env = TestEnvironment::new();
            env.write_config("model_provider = \"openai\"\n");
            let original_auth = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"original-secret"}"#;
            std::fs::write(env.auth_path(), original_auth).expect("write original auth");
            let facade_patch = CodexClientPatchConfig {
                preset: CodexClientPreset::ImagegenBridge,
                ..CodexClientPatchConfig::default()
            };
            let preserving_patch = CodexClientPatchConfig::default();
            let on = CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            };

            apply_with_client_patch(on.clone(), facade_patch).expect("apply auth facade");
            let retained_backup = env.auth_backup_path();
            assert!(matches!(
                apply_with_client_patch_and_failpoint(on.clone(), preserving_patch, failpoint),
                Err(CodexSwitchError::InjectedFailure(_))
            ));

            let prepared = read_journal(env.state_path().as_path())
                .expect("read prepared journal")
                .expect("prepared journal");
            assert_eq!(prepared.phase, JournalPhase::Prepared);
            assert_eq!(prepared.operation, JournalOperation::Off);
            assert_eq!(prepared.client_patch, facade_patch);
            assert_eq!(prepared.auth_recovery_patch, None);
            assert!(prepared.auth_patch.is_some());
            assert_eq!(env.auth_backup_path(), retained_backup);

            apply_with_client_patch(on, preserving_patch)
                .expect("resume preserving patch from retained recovery");
            let applied = read_journal(env.state_path().as_path())
                .expect("read applied preserving journal")
                .expect("applied preserving journal");
            assert_eq!(applied.phase, JournalPhase::Applied);
            assert_eq!(applied.client_patch, preserving_patch);
            assert_eq!(applied.auth_recovery_patch, Some(facade_patch));
            assert!(applied.auth_patch.is_some());
            assert_eq!(
                std::fs::read_to_string(env.auth_path()).expect("read auth after resume"),
                original_auth
            );
            apply(CodexSwitchIntent::Off).expect("restore preserving patch");
            assert_eq!(
                inspect().expect("inspect retained off state").phase,
                CodexSwitchPhase::Off
            );
            assert_eq!(env.auth_backup_path(), retained_backup);
        }
    }

    #[test]
    fn preserving_patch_repairs_a_stale_retained_facade_on_switch_off() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let original_auth = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"original-secret"}"#;
        std::fs::write(env.auth_path(), original_auth).expect("write original auth");
        let facade_patch = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };
        let preserving_patch = CodexClientPatchConfig::default();
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };

        apply_with_client_patch(on.clone(), facade_patch).expect("apply auth facade");
        apply_with_client_patch(on, preserving_patch).expect("apply preserving patch");
        let retained_backup = env.auth_backup_path();
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read preserved auth"),
            original_auth
        );

        std::fs::write(env.auth_path(), "{}\n").expect("simulate stale facade write");
        assert_eq!(
            inspect().expect("inspect stale retained facade").phase,
            CodexSwitchPhase::RecoveryRequired
        );
        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("repair stale facade and switch off")
                .change,
            CodexSwitchChange::Removed
        );
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
            original_auth
        );
        assert_eq!(env.auth_backup_path(), retained_backup);
        assert_eq!(
            inspect().expect("inspect repaired off state").phase,
            CodexSwitchPhase::Off
        );
    }

    #[test]
    fn restored_switch_rebuilds_a_missing_auth_backup_from_the_original() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let original_auth = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"original-secret"}"#;
        std::fs::write(env.auth_path(), original_auth).expect("write original auth");
        let patch = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("apply auth facade");
        apply(CodexSwitchIntent::Off).expect("restore original auth");
        let backup = env.auth_backup_path();
        std::fs::remove_file(&backup).expect("remove retained auth backup");

        assert_eq!(
            inspect().expect("inspect missing backup").phase,
            CodexSwitchPhase::RecoveryRequired
        );
        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("rebuild missing backup from unchanged original")
                .change,
            CodexSwitchChange::Recovered
        );
        assert_eq!(
            std::fs::read_to_string(&backup).expect("read rebuilt auth backup"),
            original_auth
        );
        assert_eq!(
            inspect().expect("inspect rebuilt recovery point").phase,
            CodexSwitchPhase::Off
        );
    }

    #[test]
    fn restored_switch_rejects_a_tampered_auth_backup() {
        let env = TestEnvironment::new();
        let original_config = "model_provider = \"openai\"\n";
        let original_auth = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"original-secret"}"#;
        env.write_config(original_config);
        std::fs::write(env.auth_path(), original_auth).expect("write original auth");
        let patch = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("apply auth facade");
        apply(CodexSwitchIntent::Off).expect("restore original auth");
        std::fs::write(env.auth_backup_path(), "tampered-backup").expect("tamper auth backup");

        assert_eq!(
            inspect().expect("inspect tampered retained backup").phase,
            CodexSwitchPhase::RecoveryRequired
        );
        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::RecoveryRequired { .. })
        ));
        assert_eq!(env.read_config(), original_config);
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read preserved original auth"),
            original_auth
        );
    }

    #[test]
    fn invalid_reapply_keeps_the_existing_switch_and_recovery_material() {
        let env = TestEnvironment::new();
        let original_config = "model_provider = \"openai\"\n";
        let invalid_chatgpt_auth = r#"{"auth_mode":"apikey"}"#;
        env.write_config(original_config);
        std::fs::write(env.auth_path(), invalid_chatgpt_auth).expect("write original auth");
        let target = ValidatedCodexBaseUrl::local(3211);
        let imagegen = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };
        let chatgpt = CodexClientPatchConfig {
            preset: CodexClientPreset::ChatGptBridge,
            ..CodexClientPatchConfig::default()
        };

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: target.clone(),
            },
            imagegen,
        )
        .expect("apply initial imagegen facade");
        let applied_config = std::fs::read(env.config_path()).expect("capture applied config");
        let applied_auth = std::fs::read(env.auth_path()).expect("capture applied auth");
        let applied_state = std::fs::read(env.state_path()).expect("capture applied journal");
        let backup_files = env.auth_backup_files();
        assert_eq!(backup_files.len(), 1);
        let backup = std::fs::read(&backup_files[0]).expect("capture auth backup");

        let error = apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: target,
            },
            chatgpt,
        )
        .expect_err("invalid ChatGPT reapply must fail before switch-off");

        assert!(matches!(error, CodexSwitchError::InvalidAuth { .. }));
        assert_eq!(std::fs::read(env.config_path()).unwrap(), applied_config);
        assert_eq!(std::fs::read(env.auth_path()).unwrap(), applied_auth);
        assert_eq!(std::fs::read(env.state_path()).unwrap(), applied_state);
        assert_eq!(env.auth_backup_files(), backup_files);
        assert_eq!(std::fs::read(&backup_files[0]).unwrap(), backup);

        apply(CodexSwitchIntent::Off).expect("restore the preserved switch");
        assert_eq!(env.read_config(), original_config);
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
            invalid_chatgpt_auth
        );
    }

    #[test]
    fn auth_facade_recovers_after_each_durable_write_boundary() {
        let patch = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };

        let env = TestEnvironment::new();
        let original_config = "model_provider = \"openai\"\n";
        let original_auth = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"auth-secret"}"#;
        env.write_config(original_config);
        std::fs::write(env.auth_path(), original_auth).expect("write original auth");
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };

        assert!(matches!(
            apply_with_client_patch_and_failpoint(
                on.clone(),
                patch,
                ApplyFailpoint::AfterAuthWrite
            ),
            Err(CodexSwitchError::InjectedFailure("after_auth_write"))
        ));
        assert_eq!(
            inspect().expect("inspect prepared auth write").phase,
            CodexSwitchPhase::Prepared
        );
        assert_eq!(
            apply_with_client_patch(on, patch)
                .expect("finish prepared auth write")
                .change,
            CodexSwitchChange::Recovered
        );

        assert!(matches!(
            apply_with_client_patch_and_failpoint(
                CodexSwitchIntent::Off,
                patch,
                ApplyFailpoint::AfterConfigWrite
            ),
            Err(CodexSwitchError::InjectedFailure("after_config_write"))
        ));
        assert_eq!(env.read_config(), original_config);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(env.auth_path()).expect("read still-applied auth facade")
            )
            .expect("parse still-applied auth facade"),
            serde_json::json!({})
        );
        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("finish auth restoration")
                .change,
            CodexSwitchChange::Recovered
        );
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
            original_auth
        );
        assert_eq!(
            inspect().expect("inspect retained recovery point").phase,
            CodexSwitchPhase::Off
        );
        assert_eq!(env.auth_backup_files().len(), 1);

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("apply facade again");
        assert!(matches!(
            apply_with_client_patch_and_failpoint(
                CodexSwitchIntent::Off,
                patch,
                ApplyFailpoint::AfterAuthRestore
            ),
            Err(CodexSwitchError::InjectedFailure("after_auth_restore"))
        ));
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored auth after failure"),
            original_auth
        );
        assert!(env.state_path().exists());
        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("finish state cleanup after auth restoration")
                .change,
            CodexSwitchChange::Recovered
        );
        assert_eq!(
            inspect().expect("inspect completed off state").phase,
            CodexSwitchPhase::Off
        );
        assert_eq!(env.auth_backup_files().len(), 1);
    }

    #[test]
    fn external_auth_edit_is_never_overwritten_during_switch_off() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        std::fs::write(env.auth_path(), "original-auth-secret").expect("write original auth");
        let patch = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };
        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("apply imagegen facade");
        let backup = env.auth_backup_path();
        let external = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"external-change"}"#;
        std::fs::write(env.auth_path(), external).expect("write external auth change");

        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::RecoveryRequired { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read preserved external auth"),
            external
        );
        assert!(backup.exists());
        assert_eq!(
            inspect().expect("inspect recovery state").phase,
            CodexSwitchPhase::RecoveryRequired
        );
    }

    #[test]
    fn tampered_auth_backup_is_reported_before_config_is_restored() {
        let env = TestEnvironment::new();
        let original_config = "model_provider = \"openai\"\n";
        env.write_config(original_config);
        std::fs::write(env.auth_path(), "original-auth-secret").expect("write original auth");
        let patch = CodexClientPatchConfig {
            preset: CodexClientPreset::ImagegenBridge,
            ..CodexClientPatchConfig::default()
        };
        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("apply imagegen facade");
        let applied_config = env.read_config();
        std::fs::write(env.auth_backup_path(), "tampered-backup").expect("tamper auth backup");

        assert_eq!(
            inspect().expect("inspect tampered backup").phase,
            CodexSwitchPhase::RecoveryRequired
        );
        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::RecoveryRequired { .. })
        ));
        assert_eq!(env.read_config(), applied_config);
        assert_ne!(env.read_config(), original_config);
    }

    #[cfg(unix)]
    #[test]
    fn switch_backups_are_stored_with_private_unix_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let env = TestEnvironment::new();
        env.write_config(
            r#"model_provider = "openai"

[model_providers.codex_proxy]
name = "foreign"
base_url = "https://foreign.example/v1"
api_key = "private-helper-stanza-secret"
"#,
        );
        std::fs::write(env.auth_path(), "private-auth-secret").expect("write original auth");
        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            CodexClientPatchConfig {
                preset: CodexClientPreset::ImagegenBridge,
                ..CodexClientPatchConfig::default()
            },
        )
        .expect("apply imagegen facade");

        let backup = env.auth_backup_path();
        assert_eq!(
            std::fs::metadata(&backup)
                .expect("read backup metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(env.helper_stanza_backup_path())
                .expect("read helper stanza backup metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(backup.parent().expect("backup parent"))
                .expect("read state directory metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let journal = std::fs::read_to_string(env.state_path()).expect("read journal");
        assert!(!journal.contains("private-auth-secret"));
        assert!(!journal.contains("private-helper-stanza-secret"));
    }

    #[cfg(windows)]
    #[test]
    fn switch_writes_preserve_or_apply_private_windows_acls() {
        fn metadata(path: &Path) -> crate::config::ConfigFileMetadata {
            let file_metadata = std::fs::symlink_metadata(path).expect("read file metadata");
            crate::config::ConfigFileMetadata::capture(path, &file_metadata)
                .expect("capture Windows security metadata")
        }

        let env = TestEnvironment::new();
        env.write_config(
            r#"model_provider = "openai"

[model_providers.codex_proxy]
name = "foreign"
base_url = "https://foreign.example/v1"
api_key = "private-helper-stanza-secret"
"#,
        );
        std::fs::write(env.auth_path(), "private-auth-secret").expect("write original auth");
        crate::local_operator::secure_private_windows_path(env.config_path().as_path(), false)
            .expect("secure original config");
        crate::local_operator::secure_private_windows_path(env.auth_path().as_path(), false)
            .expect("secure original auth");
        let config_metadata = metadata(env.config_path().as_path());
        let auth_metadata = metadata(env.auth_path().as_path());

        let private_file = env.root.join("private-reference");
        std::fs::write(&private_file, "reference").expect("write private reference file");
        crate::local_operator::secure_private_windows_path(&private_file, false)
            .expect("secure private reference file");
        let private_file_metadata = metadata(&private_file);
        let private_directory = env.root.join("private-reference-directory");
        std::fs::create_dir(&private_directory).expect("create private reference directory");
        crate::local_operator::secure_private_windows_path(&private_directory, true)
            .expect("secure private reference directory");
        let private_directory_metadata = metadata(&private_directory);

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            CodexClientPatchConfig {
                preset: CodexClientPreset::ImagegenBridge,
                ..CodexClientPatchConfig::default()
            },
        )
        .expect("apply imagegen facade");

        assert!(metadata(env.config_path().as_path()).matches(&config_metadata));
        assert!(metadata(env.auth_path().as_path()).matches(&auth_metadata));
        assert!(metadata(env.state_path().as_path()).matches(&private_file_metadata));
        assert!(metadata(env.auth_backup_path().as_path()).matches(&private_file_metadata));
        assert!(
            metadata(env.helper_stanza_backup_path().as_path()).matches(&private_file_metadata)
        );
        assert!(
            metadata(env.state_path().parent().expect("state directory"))
                .matches(&private_directory_metadata)
        );

        apply(CodexSwitchIntent::Off).expect("restore switch files");
        assert!(metadata(env.config_path().as_path()).matches(&config_metadata));
        assert!(metadata(env.auth_path().as_path()).matches(&auth_metadata));
    }

    #[test]
    fn client_patch_removes_features_table_created_only_for_owned_keys_on_switch_off() {
        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n";
        env.write_config(original);
        let patch = crate::config::CodexClientPatchConfig {
            preset: crate::config::CodexClientPreset::OfficialImagegen,
            compaction: crate::config::CodexCompactionStrategy::RemoteV2,
            hosted_image_generation: crate::config::CodexHostedImageGenerationMode::Enabled,
            ..crate::config::CodexClientPatchConfig::default()
        };

        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("apply feature-owning patch");
        let applied = env.read_config();
        assert!(applied.contains("remote_compaction_v2 = true"));
        assert!(applied.contains("image_generation = true"));

        apply(CodexSwitchIntent::Off).expect("restore absent features table");
        assert_eq!(env.read_config(), original);
    }

    #[test]
    fn prepared_switch_resumes_with_recorded_complete_client_patch() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let intent = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        let patch = crate::config::CodexClientPatchConfig {
            preset: crate::config::CodexClientPreset::OfficialImagegen,
            responses_websocket: true,
            compaction: crate::config::CodexCompactionStrategy::RemoteV2,
            hosted_image_generation: crate::config::CodexHostedImageGenerationMode::Enabled,
            ..crate::config::CodexClientPatchConfig::default()
        };

        assert!(matches!(
            apply_with_client_patch_and_failpoint(
                intent.clone(),
                patch,
                ApplyFailpoint::AfterPrepared,
            ),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));
        let outcome =
            apply_with_client_patch(intent, patch).expect("resume prepared complete client patch");

        assert_eq!(outcome.status.client_patch.as_ref(), Some(&patch));
        let applied = env.read_config();
        assert!(applied.contains(CODEX_CLIENT_FACADE_ACTOR_VALUE));
        assert!(applied.contains("supports_websockets = true"));
        assert!(applied.contains("remote_compaction_v2 = true"));
        assert!(applied.contains("image_generation = true"));
    }

    #[test]
    fn changing_client_patch_reapplies_without_losing_the_original_config() {
        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n\n[features]\nimage_generation = false\n";
        env.write_config(original);
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        let relay_patch = CodexClientPatchConfig {
            preset: CodexClientPreset::OfficialRelay,
            ..CodexClientPatchConfig::default()
        };
        let imagegen_patch = CodexClientPatchConfig {
            preset: CodexClientPreset::OfficialImagegen,
            ..CodexClientPatchConfig::default()
        };
        apply_with_client_patch(on.clone(), relay_patch).expect("switch on relay patch");
        let outcome = apply_with_client_patch(on, imagegen_patch).expect("reapply imagegen patch");

        assert_eq!(outcome.status.client_patch, Some(imagegen_patch));
        assert!(env.read_config().contains(CODEX_CLIENT_FACADE_ACTOR_VALUE));

        apply(CodexSwitchIntent::Off).expect("switch off reapplied patch");
        assert_eq!(env.read_config(), original);
    }

    #[test]
    fn interrupted_client_patch_reapply_resumes_from_the_recorded_off_transition() {
        for failpoint in [
            ApplyFailpoint::AfterPrepared,
            ApplyFailpoint::AfterConfigWrite,
        ] {
            let env = TestEnvironment::new();
            let original = "model_provider = \"openai\"\n";
            env.write_config(original);
            let on = CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            };
            let relay_patch = CodexClientPatchConfig {
                preset: CodexClientPreset::OfficialRelay,
                ..CodexClientPatchConfig::default()
            };
            let imagegen_patch = CodexClientPatchConfig {
                preset: CodexClientPreset::OfficialImagegen,
                ..CodexClientPatchConfig::default()
            };
            apply_with_client_patch(on.clone(), relay_patch).expect("switch on initial patch");

            assert!(matches!(
                apply_with_client_patch_and_failpoint(on.clone(), imagegen_patch, failpoint),
                Err(CodexSwitchError::InjectedFailure(_))
            ));
            let outcome = apply_with_client_patch(on, imagegen_patch)
                .expect("resume interrupted patch reapply");

            assert_eq!(outcome.status.phase, CodexSwitchPhase::Applied);
            assert_eq!(outcome.status.client_patch, Some(imagegen_patch));
            apply(CodexSwitchIntent::Off).expect("restore after resumed reapply");
            assert_eq!(env.read_config(), original);
        }
    }

    #[test]
    fn prepared_switch_recovers_with_its_recorded_client_patch() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        let patch = CodexClientPatchConfig {
            preset: CodexClientPreset::OfficialImagegen,
            ..CodexClientPatchConfig::default()
        };

        assert!(matches!(
            apply_with_client_patch_and_failpoint(on.clone(), patch, ApplyFailpoint::AfterPrepared),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));
        let outcome =
            apply_with_client_patch(on, patch).expect("resume prepared client patch switch");

        assert_eq!(outcome.status.client_patch, Some(patch));
        assert!(env.read_config().contains(CODEX_CLIENT_FACADE_ACTOR_VALUE));
    }

    #[test]
    fn current_journal_without_complete_client_patch_fails_closed() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on with complete patch");
        let config_before = env.read_config();
        let mut state = serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(env.state_path()).expect("read current journal"),
        )
        .expect("parse current journal");
        state
            .as_object_mut()
            .expect("journal object")
            .remove("client_patch");
        let incomplete_state =
            serde_json::to_vec_pretty(&state).expect("serialize incomplete journal");
        std::fs::write(env.state_path(), &incomplete_state).expect("write incomplete journal");

        assert!(matches!(
            inspect(),
            Err(CodexSwitchError::InvalidState { .. })
        ));
        assert_eq!(env.read_config(), config_before);
        assert_eq!(std::fs::read(env.state_path()).unwrap(), incomplete_state);
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

    fn legacy_applied_config(base_url: &str) -> String {
        format!(
            r#"model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "{base_url}"
wire_api = "responses"
request_max_retries = 0
"#
        )
    }

    fn legacy_official_applied_config(base_url: &str, responses_websocket: bool) -> String {
        format!(
            r#"model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "{base_url}"
wire_api = "responses"
request_max_retries = 0
supports_websockets = {responses_websocket}
"#
        )
    }

    fn legacy_chatgpt_applied_config(base_url: &str) -> String {
        format!(
            r#"model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "{base_url}"
wire_api = "responses"
request_max_retries = 0
requires_openai_auth = true
supports_websockets = false
"#
        )
    }

    fn legacy_state(version: u32) -> serde_json::Value {
        serde_json::json!({
            "version": version,
            "original_config_absent": false,
            "original_model_provider": "openai",
            "original_codex_proxy": null,
            "had_model_providers": false
        })
    }

    fn legacy_state_with_auth(
        original_absent: bool,
        original: Option<&str>,
        patched: &str,
    ) -> serde_json::Value {
        legacy_state_with_auth_mode("imagegen-bridge", original_absent, original, patched)
    }

    fn legacy_state_with_auth_mode(
        patch_mode: &str,
        original_absent: bool,
        original: Option<&str>,
        patched: &str,
    ) -> serde_json::Value {
        let mut state = legacy_state(2);
        let object = state.as_object_mut().expect("legacy state object");
        object.insert(
            "patch_mode".to_string(),
            serde_json::Value::String(patch_mode.to_string()),
        );
        object.insert(
            "original_auth_json_absent".to_string(),
            serde_json::Value::Bool(original_absent),
        );
        if let Some(original) = original {
            object.insert(
                "original_auth_json".to_string(),
                serde_json::Value::String(original.to_string()),
            );
        }
        object.insert(
            "patched_auth_json".to_string(),
            serde_json::Value::String(patched.to_string()),
        );
        state
    }

    #[test]
    fn switch_off_automatically_recovers_v1_and_v2_legacy_state() {
        for version in [1, 2] {
            let env = TestEnvironment::new();
            env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
            env.write_legacy_state(legacy_state(version));

            let outcome = apply(CodexSwitchIntent::Off).expect("recover legacy switch state");

            assert_eq!(outcome.change, CodexSwitchChange::Recovered);
            assert_eq!(env.read_config(), "model_provider = \"openai\"\n");
            assert!(!env.legacy_state_path().exists());
            assert!(!env.state_path().exists());
        }
    }

    #[test]
    fn legacy_recovery_preserves_an_external_config_deletion_and_restores_auth() {
        let env = TestEnvironment::new();
        let original_auth = r#"{"OPENAI_API_KEY":"original-secret"}"#;
        let patched_auth = "{}";
        env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
        std::fs::write(env.auth_path(), patched_auth).expect("write patched auth");
        env.write_legacy_state(legacy_state_with_auth(
            false,
            Some(original_auth),
            patched_auth,
        ));
        std::fs::remove_file(env.config_path()).expect("delete config outside helper");

        let outcome = apply(CodexSwitchIntent::Off)
            .expect("preserve config deletion while completing legacy recovery");

        assert_eq!(outcome.change, CodexSwitchChange::Recovered);
        assert!(!env.config_path().exists());
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
            original_auth
        );
        assert!(!env.legacy_state_path().exists());
    }

    #[test]
    fn legacy_recovery_rejects_selector_stanza_hybrid_projections() {
        let original_stanza = r#"name = "original proxy"
base_url = "https://original.example/v1"
wire_api = "chat"
request_max_retries = 7"#;
        let original_state = serde_json::json!({
            "version": 2,
            "original_config_absent": false,
            "original_model_provider": "openai",
            "original_codex_proxy": {
                "name": "original proxy",
                "base_url": "https://original.example/v1",
                "wire_api": "chat",
                "request_max_retries": 7
            },
            "had_model_providers": true
        });
        let cases = [
            format!(
                "model_provider = \"codex_proxy\"\n\n[model_providers.codex_proxy]\n{original_stanza}\n"
            ),
            legacy_applied_config("http://127.0.0.1:3211")
                .replace("model_provider = \"codex_proxy\"", "model_provider = \"openai\"")
                .replace(
                    "name = \"codex-helper\"\nbase_url = \"http://127.0.0.1:3211\"\nwire_api = \"responses\"\nrequest_max_retries = 0",
                    "name = \"codex-helper\"\nbase_url = \"http://127.0.0.1:3211\"\nwire_api = \"responses\"\nrequest_max_retries = 7",
                ),
        ];

        for hybrid in cases {
            let env = TestEnvironment::new();
            env.write_config(&hybrid);
            env.write_legacy_state(original_state.clone());
            let stored_state = std::fs::read(env.legacy_state_path()).expect("read legacy state");

            let error = apply(CodexSwitchIntent::Off)
                .expect_err("hybrid selector/stanza projection must fail closed");

            assert!(matches!(
                error,
                CodexSwitchError::LegacyRecoveryRequired { .. }
            ));
            assert_eq!(env.read_config(), hybrid);
            assert_eq!(
                std::fs::read(env.legacy_state_path()).expect("read preserved legacy state"),
                stored_state
            );
        }
    }

    #[test]
    fn legacy_recovery_accepts_an_original_stanza_identical_to_the_helper_patch() {
        let env = TestEnvironment::new();
        let applied = legacy_applied_config("http://127.0.0.1:3211");
        env.write_config(&applied);
        env.write_legacy_state(serde_json::json!({
            "version": 2,
            "original_config_absent": false,
            "original_model_provider": "openai",
            "original_codex_proxy": {
                "name": "codex-helper",
                "base_url": "http://127.0.0.1:3211",
                "wire_api": "responses",
                "request_max_retries": 0
            },
            "had_model_providers": true
        }));

        let outcome = apply(CodexSwitchIntent::Off)
            .expect("restore only the selector when the original stanza equals the patch");

        assert_eq!(outcome.change, CodexSwitchChange::Recovered);
        assert_eq!(
            env.read_config(),
            applied.replace(
                "model_provider = \"codex_proxy\"",
                "model_provider = \"openai\""
            )
        );
        assert!(!env.legacy_state_path().exists());
    }

    #[test]
    fn legacy_noop_snapshot_plan_still_uses_commit_time_cas() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let snapshot = read_config_snapshot(env.config_path().as_path()).expect("read snapshot");
        env.write_config("model_provider = \"external\"\n");

        let error = apply_snapshot_edit_if_needed(
            env.config_path().as_path(),
            env.legacy_state_path().as_path(),
            &snapshot,
            None,
        )
        .expect_err("a no-op plan must still verify its source snapshot");

        assert!(matches!(
            error,
            CodexSwitchError::LegacyRecoveryRequired { .. }
        ));
        assert_eq!(env.read_config(), "model_provider = \"external\"\n");
    }

    #[test]
    fn legacy_snapshot_cas_reports_topology_replacement_as_legacy_recovery() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let snapshot = read_config_snapshot(env.config_path().as_path()).expect("read snapshot");
        std::fs::remove_file(env.config_path()).expect("remove snapshotted config");
        std::fs::create_dir(env.config_path()).expect("replace config with directory");

        let error = apply_snapshot_edit_if_needed(
            env.config_path().as_path(),
            env.legacy_state_path().as_path(),
            &snapshot,
            None,
        )
        .expect_err("a topology replacement must preserve legacy recovery authority");

        assert!(matches!(
            error,
            CodexSwitchError::LegacyRecoveryRequired { .. }
        ));
        assert!(env.config_path().is_dir());
    }

    #[test]
    fn legacy_final_config_snapshot_is_rechecked_before_state_removal() {
        let env = TestEnvironment::new();
        env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
        env.write_legacy_state(legacy_state(2));
        let paths = SwitchPaths::resolve().expect("resolve switch paths");
        let external = "model_provider = \"external\"\n";

        let error =
            recover_legacy_switch_state_with_before_remove(&paths, ApplyFailpoint::None, || {
                env.write_config(external);
                Ok(())
            })
            .expect_err("a final-read race must preserve legacy recovery state");

        assert!(matches!(
            error,
            CodexSwitchError::LegacyRecoveryRequired { .. }
        ));
        assert_eq!(env.read_config(), external);
        assert!(env.legacy_state_path().exists());
    }

    #[test]
    fn legacy_final_auth_snapshot_is_rechecked_before_state_removal() {
        let env = TestEnvironment::new();
        let original_auth = r#"{"OPENAI_API_KEY":"original-secret"}"#;
        let patched_auth = "{}";
        env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
        std::fs::write(env.auth_path(), patched_auth).expect("write patched auth");
        env.write_legacy_state(legacy_state_with_auth(
            false,
            Some(original_auth),
            patched_auth,
        ));
        let paths = SwitchPaths::resolve().expect("resolve switch paths");
        let external_auth = r#"{"external":"edit"}"#;

        let error =
            recover_legacy_switch_state_with_before_remove(&paths, ApplyFailpoint::None, || {
                std::fs::write(env.auth_path(), external_auth).expect("write external auth");
                Ok(())
            })
            .expect_err("an auth final-read race must preserve legacy recovery state");

        assert!(matches!(
            error,
            CodexSwitchError::LegacyRecoveryRequired { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read external auth"),
            external_auth
        );
        assert!(env.legacy_state_path().exists());
    }

    #[test]
    fn legacy_state_snapshot_is_rechecked_before_removal() {
        let env = TestEnvironment::new();
        env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
        env.write_legacy_state(legacy_state(2));
        let paths = SwitchPaths::resolve().expect("resolve switch paths");
        let replacement = serde_json::to_vec_pretty(&legacy_state(1))
            .expect("serialize replacement legacy state");

        let error =
            recover_legacy_switch_state_with_before_remove(&paths, ApplyFailpoint::None, || {
                std::fs::write(env.legacy_state_path(), &replacement)
                    .expect("replace legacy state concurrently");
                Ok(())
            })
            .expect_err("a legacy-state race must preserve the replacement state");

        assert!(matches!(
            error,
            CodexSwitchError::LegacyRecoveryRequired { .. }
        ));
        assert_eq!(
            std::fs::read(env.legacy_state_path()).expect("read replacement state"),
            replacement
        );
    }

    #[test]
    fn managed_switch_artifact_names_require_an_exact_uuid_shape() {
        let uuid = Uuid::new_v4();
        assert!(managed_auth_backup_name(&format!(
            "{AUTH_BACKUP_FILE_PREFIX}{uuid}{AUTH_BACKUP_FILE_SUFFIX}"
        )));
        assert!(managed_helper_stanza_backup_name(&format!(
            "{HELPER_STANZA_BACKUP_FILE_PREFIX}{uuid}{HELPER_STANZA_BACKUP_FILE_SUFFIX}"
        )));
        for name in [
            "codex-switch-auth-v1-not-a-uuid.bak",
            "codex-switch-helper-stanza-v1-not-a-uuid.bak",
            "codex-switch-helper-stanza-v1-00000000-0000-0000-0000-000000000000.bak.extra",
        ] {
            assert!(!managed_auth_backup_name(name));
            assert!(!managed_helper_stanza_backup_name(name));
        }
        for name in [
            format!("{SWITCH_TEMP_FILE_PREFIX}{uuid}{SWITCH_TEMP_FILE_SUFFIX}"),
            format!("{LEGACY_SWITCH_TEMP_FILE_PREFIX}{uuid}{SWITCH_TEMP_FILE_SUFFIX}"),
            format!("{SWITCH_DELETE_TOMBSTONE_PREFIX}{uuid}"),
        ] {
            assert!(managed_switch_artifact_name(std::ffi::OsStr::new(&name)));
        }
        for name in [
            ".codex-switch-not-a-uuid.tmp",
            ".codex-switch-v1-not-a-uuid.tmp",
            ".codex-switch-delete-v1-not-a-uuid",
            &format!("{SWITCH_CAPTURE_FILE_PREFIX}{uuid}-config"),
            ".config.toml.codex-switch-delete-pending",
            ".codex-switch-delete-v1-00000000-0000-0000-0000-000000000000.extra",
            ".codex-switch-v1-00000000-0000-0000-0000-000000000000",
            &format!(
                "{LEGACY_SWITCH_TEMP_FILE_PREFIX}{}{SWITCH_TEMP_FILE_SUFFIX}",
                uuid.simple()
            ),
            &format!("{LEGACY_SWITCH_TEMP_FILE_PREFIX}{{{uuid}}}{SWITCH_TEMP_FILE_SUFFIX}"),
        ] {
            assert!(!managed_switch_artifact_name(std::ffi::OsStr::new(name)));
        }
    }

    #[test]
    fn pending_delete_collision_guard_preserves_both_files() {
        let env = TestEnvironment::new();
        let target = env.codex_home.join("durable-delete-target");
        let tombstone = pending_delete_path(target.as_path()).expect("build tombstone path");
        std::fs::write(&target, b"canonical").expect("write canonical target");
        std::fs::write(&tombstone, b"existing tombstone").expect("write tombstone collision");

        let error = ensure_pending_delete_destination_absent(tombstone.as_path())
            .expect_err("an existing tombstone must fail closed");

        assert!(matches!(
            error,
            CodexSwitchError::Io { source, .. }
                if source.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(
            std::fs::read(target).expect("read canonical target"),
            b"canonical"
        );
        assert_eq!(
            std::fs::read(tombstone).expect("read existing tombstone"),
            b"existing tombstone"
        );
    }

    #[test]
    fn no_replace_move_collision_preserves_source_and_destination() {
        let env = TestEnvironment::new();
        let source = env.codex_home.join("no-replace-source");
        let destination = env.codex_home.join("no-replace-destination");
        std::fs::write(&source, b"prepared replacement").expect("write move source");
        std::fs::write(&destination, b"competing writer").expect("write move destination");

        move_file_no_replace(source.as_path(), destination.as_path())
            .expect_err("no-replace move must reject an existing destination");

        assert_eq!(
            std::fs::read(source).expect("read preserved source"),
            b"prepared replacement"
        );
        assert_eq!(
            std::fs::read(destination).expect("read preserved destination"),
            b"competing writer"
        );
    }

    #[test]
    fn interrupted_managed_capture_is_restored_before_switch_resume() {
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
        let paths = SwitchPaths::resolve().expect("resolve switch paths");
        let journal = read_journal(env.state_path().as_path())
            .expect("read prepared journal")
            .expect("prepared journal");
        let capture = managed_capture_path(
            env.config_path().as_path(),
            journal.operation_id.as_str(),
            ManagedCommitRole::Config,
        )
        .expect("resolve managed capture");
        let expected = read_config_snapshot(env.config_path().as_path()).expect("capture expected");
        capture_expected_file(
            env.config_path().as_path(),
            capture.as_path(),
            FileCommitExpectation::Journal(ConfigCommitExpectation {
                journal: &journal,
                expected: &expected,
            }),
        )
        .expect("simulate crash after detaching the expected config");
        assert!(!env.config_path().exists());
        assert_eq!(
            std::fs::read_to_string(&capture).expect("read detached original"),
            original
        );

        assert_eq!(
            apply(on)
                .expect("recover the capture and resume switch")
                .change,
            CodexSwitchChange::Applied
        );
        assert_eq!(
            inspect().expect("inspect resumed switch").phase,
            CodexSwitchPhase::Applied
        );
        assert!(!capture.exists());
        apply(CodexSwitchIntent::Off).expect("restore original config");
        assert_eq!(env.read_config(), original);
        assert!(!capture.exists());
        assert!(!paths.state.exists());
    }

    #[test]
    fn competing_writer_after_capture_is_preserved_for_manual_recovery() {
        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n";
        let competing = "model_provider = \"external\"\n";
        env.write_config(original);
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        assert!(matches!(
            apply_with_failpoint(on.clone(), ApplyFailpoint::AfterPrepared),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));
        let journal = read_journal(env.state_path().as_path())
            .expect("read prepared journal")
            .expect("prepared journal");
        let capture = managed_capture_path(
            env.config_path().as_path(),
            journal.operation_id.as_str(),
            ManagedCommitRole::Config,
        )
        .expect("resolve managed capture");
        let expected = read_config_snapshot(env.config_path().as_path()).expect("capture expected");
        capture_expected_file(
            env.config_path().as_path(),
            capture.as_path(),
            FileCommitExpectation::Journal(ConfigCommitExpectation {
                journal: &journal,
                expected: &expected,
            }),
        )
        .expect("detach expected config");
        env.write_config(competing);

        assert!(matches!(
            apply(on),
            Err(CodexSwitchError::RecoveryRequired { .. })
        ));
        assert_eq!(env.read_config(), competing);
        assert_eq!(
            std::fs::read_to_string(&capture).expect("read preserved capture"),
            original
        );
        let stored = read_journal(env.state_path().as_path())
            .expect("read recovery journal")
            .expect("recovery journal");
        assert_eq!(stored.phase, JournalPhase::RecoveryRequired);
        assert_eq!(
            inspect().expect("inspect collision").phase,
            CodexSwitchPhase::RecoveryRequired
        );
    }

    #[test]
    fn commit_time_mismatch_restores_the_captured_external_file() {
        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n";
        let external = "model_provider = \"external\"\n";
        env.write_config(original);
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        assert!(matches!(
            apply_with_failpoint(on, ApplyFailpoint::AfterPrepared),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));
        let journal = read_journal(env.state_path().as_path())
            .expect("read prepared journal")
            .expect("prepared journal");
        let capture = managed_capture_path(
            env.config_path().as_path(),
            journal.operation_id.as_str(),
            ManagedCommitRole::Config,
        )
        .expect("resolve managed capture");
        let stage = env.codex_home.join("prepared-config-stage");
        let expected = ConfigSnapshot::from_text(true, original.to_string());
        env.write_config(external);
        std::fs::write(&stage, b"helper replacement").expect("write staged replacement");

        commit_staged_file(
            stage.as_path(),
            env.config_path().as_path(),
            FileCommitExpectation::Journal(ConfigCommitExpectation {
                journal: &journal,
                expected: &expected,
            }),
        )
        .expect_err("commit-time mismatch must fail closed");

        assert_eq!(env.read_config(), external);
        assert!(stage.exists());
        assert!(!capture.exists());
    }

    #[test]
    fn startup_cleanup_removes_only_strict_managed_switch_artifacts() {
        let env = TestEnvironment::new();
        let paths = SwitchPaths::resolve().expect("resolve switch paths");
        let state_path = env.state_path();
        let state_parent = state_path.parent().expect("state parent");
        std::fs::create_dir_all(state_parent).expect("create state parent");
        let managed = [
            env.codex_home.join(format!(
                "{SWITCH_TEMP_FILE_PREFIX}{}{SWITCH_TEMP_FILE_SUFFIX}",
                Uuid::new_v4()
            )),
            env.codex_home.join(format!(
                "{LEGACY_SWITCH_TEMP_FILE_PREFIX}{}{SWITCH_TEMP_FILE_SUFFIX}",
                Uuid::new_v4()
            )),
            state_parent.join(format!(
                "{SWITCH_DELETE_TOMBSTONE_PREFIX}{}",
                Uuid::new_v4()
            )),
        ];
        let unmanaged = [
            env.codex_home.join(".codex-switch-user-data.tmp"),
            env.codex_home
                .join(".config.toml.codex-switch-delete-pending"),
            state_parent.join(".codex-switch-delete-v1-not-a-uuid"),
        ];
        let managed_directory = env.codex_home.join(format!(
            "{SWITCH_TEMP_FILE_PREFIX}{}{SWITCH_TEMP_FILE_SUFFIX}",
            Uuid::new_v4()
        ));
        for path in managed.iter().chain(unmanaged.iter()) {
            std::fs::write(path, b"sensitive-or-external").expect("write cleanup fixture");
        }
        std::fs::create_dir(&managed_directory).expect("create managed-name directory");

        cleanup_managed_switch_artifacts(&paths).expect("clean managed switch artifacts");

        for path in managed {
            assert!(!path.exists(), "managed artifact survived: {path:?}");
        }
        for path in unmanaged {
            assert!(path.exists(), "unmanaged artifact was removed: {path:?}");
        }
        assert!(
            managed_directory.is_dir(),
            "a directory in the managed namespace must be preserved"
        );
    }

    #[cfg(unix)]
    #[test]
    fn startup_cleanup_preserves_managed_name_symlinks() {
        use std::os::unix::fs::symlink;

        let env = TestEnvironment::new();
        let paths = SwitchPaths::resolve().expect("resolve switch paths");
        let external = env.root.join("external-sensitive-file");
        let link = env.codex_home.join(format!(
            "{LEGACY_SWITCH_TEMP_FILE_PREFIX}{}{SWITCH_TEMP_FILE_SUFFIX}",
            Uuid::new_v4()
        ));
        std::fs::write(&external, b"external").expect("write external file");
        symlink(&external, &link).expect("create managed-name symlink");

        cleanup_managed_switch_artifacts(&paths).expect("clean managed switch artifacts");

        assert!(
            std::fs::symlink_metadata(&link)
                .expect("inspect preserved symlink")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read(external).expect("read external file"),
            b"external"
        );
    }

    #[test]
    fn switch_off_recovers_complete_v0203_state_schema() {
        let env = TestEnvironment::new();
        env.write_config(&legacy_official_applied_config(
            "https://relay.example/v1",
            true,
        ));
        env.write_legacy_state(serde_json::json!({
            "version": 2,
            "patch_mode": "official-relay",
            "responses_websocket": true,
            "compaction": "remote-v2",
            "original_config_absent": false,
            "original_model_provider": "openai",
            "original_codex_proxy": null,
            "had_model_providers": false,
            "original_auth_json_absent": false
        }));

        let outcome = apply(CodexSwitchIntent::Off).expect("recover v0.20.3 switch state");

        assert_eq!(outcome.change, CodexSwitchChange::Recovered);
        assert_eq!(env.read_config(), "model_provider = \"openai\"\n");
        assert!(!env.legacy_state_path().exists());
        assert!(!env.state_path().exists());
    }

    #[test]
    fn legacy_recovery_restores_real_imagegen_auth_facades() {
        let original_auth = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"sk-platform-onboarding","tokens":{"access_token":"access","refresh_token":"refresh","account_id":"acct"}}"#;
        for (patch_mode, config) in [
            (
                "imagegen-bridge",
                legacy_applied_config("http://127.0.0.1:3211"),
            ),
            (
                "official-imagegen",
                legacy_official_applied_config("http://127.0.0.1:3211", false),
            ),
        ] {
            let env = TestEnvironment::new();
            env.write_config(&config);
            std::fs::write(env.auth_path(), "{\n}\n").expect("write empty auth facade");
            env.write_legacy_state(legacy_state_with_auth_mode(
                patch_mode,
                false,
                Some(original_auth),
                "{}",
            ));

            let outcome =
                apply(CodexSwitchIntent::Off).expect("recover a real v0.20.3 imagegen auth facade");

            assert_eq!(outcome.change, CodexSwitchChange::Recovered);
            assert_eq!(env.read_config(), "model_provider = \"openai\"\n");
            assert_eq!(
                std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
                original_auth
            );
            assert!(!env.legacy_state_path().exists());
        }
    }

    #[test]
    fn legacy_recovery_restores_real_chatgpt_bridge_auth_object() {
        let env = TestEnvironment::new();
        let original_auth = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":"sk-platform-onboarding","tokens":{"id_token":"id","access_token":"access","refresh_token":"refresh","account_id":"acct"},"last_refresh":"2026-05-17T00:00:00Z"}"#;
        let patched_auth = r#"{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"id_token":"id","access_token":"access","refresh_token":"refresh","account_id":"acct"},"last_refresh":"2026-05-17T00:00:00Z"}"#;
        env.write_config(&legacy_chatgpt_applied_config("http://127.0.0.1:3211"));
        std::fs::write(env.auth_path(), patched_auth).expect("write ChatGPT bridge auth facade");
        env.write_legacy_state(legacy_state_with_auth_mode(
            "chat-gpt-bridge",
            false,
            Some(original_auth),
            patched_auth,
        ));

        let outcome = apply(CodexSwitchIntent::Off)
            .expect("recover a real v0.20.3 ChatGPT bridge auth object");

        assert_eq!(outcome.change, CodexSwitchChange::Recovered);
        assert_eq!(env.read_config(), "model_provider = \"openai\"\n");
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
            original_auth
        );
        assert!(!env.legacy_state_path().exists());
    }

    #[test]
    fn legacy_recovery_accepts_state_first_crash_boundaries() {
        let original_config = "model_provider = \"openai\"\n";
        let original_auth = r#"{"OPENAI_API_KEY":"original-secret"}"#;
        for config in [
            original_config.to_string(),
            legacy_applied_config("http://127.0.0.1:3211"),
        ] {
            let env = TestEnvironment::new();
            env.write_config(&config);
            std::fs::write(env.auth_path(), original_auth).expect("write original auth");
            env.write_legacy_state(legacy_state_with_auth(false, Some(original_auth), "{}"));

            let outcome = apply(CodexSwitchIntent::Off)
                .expect("complete recovery after a state-first legacy crash");

            assert_eq!(outcome.change, CodexSwitchChange::Recovered);
            assert_eq!(env.read_config(), original_config);
            assert_eq!(
                std::fs::read_to_string(env.auth_path()).expect("read original auth"),
                original_auth
            );
            assert!(!env.legacy_state_path().exists());
        }
    }

    #[test]
    fn switch_off_recovers_v1_state_rewritten_with_later_legacy_extensions() {
        let env = TestEnvironment::new();
        env.write_config(&legacy_official_applied_config(
            "https://relay.example/v1",
            true,
        ));
        env.write_legacy_state(serde_json::json!({
            "version": 1,
            "patch_mode": "official-relay-bridge",
            "responses_websocket": true,
            "compaction": "remote_v2",
            "original_config_absent": false,
            "original_model_provider": "openai",
            "original_codex_proxy": null,
            "had_model_providers": false
        }));

        let outcome = apply(CodexSwitchIntent::Off)
            .expect("recover v1 state carrying later legacy extension fields");

        assert_eq!(outcome.change, CodexSwitchChange::Recovered);
        assert_eq!(env.read_config(), "model_provider = \"openai\"\n");
        assert!(!env.legacy_state_path().exists());
    }

    #[test]
    fn legacy_recovery_reconstructs_patch_from_original_stanza_fields() {
        let env = TestEnvironment::new();
        let applied = r#"model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "https://relay.example/v1"
wire_api = "responses"
request_max_retries = 7
extra_setting = "preserved"
supports_websockets = false
"#;
        env.write_config(applied);
        env.write_legacy_state(serde_json::json!({
            "version": 2,
            "patch_mode": "official-relay",
            "original_config_absent": false,
            "original_model_provider": "openai",
            "original_codex_proxy": {
                "name": "original proxy",
                "base_url": "https://original.example/v1",
                "wire_api": "chat",
                "request_max_retries": 7,
                "extra_setting": "preserved",
                "supports_websockets": true
            },
            "had_model_providers": true
        }));

        apply(CodexSwitchIntent::Off).expect("recover reconstructed legacy patch");

        let restored =
            toml::from_str::<TomlValue>(&env.read_config()).expect("parse restored config");
        assert_eq!(restored["model_provider"].as_str(), Some("openai"));
        let stanza = &restored["model_providers"][PROVIDER_ID];
        assert_eq!(stanza["name"].as_str(), Some("original proxy"));
        assert_eq!(
            stanza["base_url"].as_str(),
            Some("https://original.example/v1")
        );
        assert_eq!(stanza["wire_api"].as_str(), Some("chat"));
        assert_eq!(stanza["request_max_retries"].as_integer(), Some(7));
        assert_eq!(stanza["extra_setting"].as_str(), Some("preserved"));
        assert_eq!(stanza["supports_websockets"].as_bool(), Some(true));
        assert!(!env.legacy_state_path().exists());
    }

    #[test]
    fn malformed_known_v0203_fields_are_preserved_without_mutation() {
        for (field, value) in [
            ("patch_mode", serde_json::json!(123)),
            ("responses_websocket", serde_json::json!("yes")),
            ("compaction", serde_json::json!("remote-v3")),
        ] {
            let env = TestEnvironment::new();
            let original = legacy_applied_config("http://127.0.0.1:3211");
            env.write_config(&original);
            let mut state = legacy_state(2);
            state
                .as_object_mut()
                .expect("legacy state object")
                .insert(field.to_string(), value);
            env.write_legacy_state(state);
            let stored_state = std::fs::read(env.legacy_state_path()).expect("read legacy state");

            let error = apply(CodexSwitchIntent::Off)
                .expect_err("malformed known legacy field must block recovery");

            assert!(matches!(error, CodexSwitchError::InvalidState { .. }));
            assert_eq!(env.read_config(), original);
            assert_eq!(
                std::fs::read(env.legacy_state_path()).expect("read preserved legacy state"),
                stored_state
            );
        }
    }

    #[test]
    fn external_edits_to_legacy_helper_stanza_fail_closed() {
        for (field, replacement) in [
            ("wire_api", "wire_api = \"chat\""),
            ("request_max_retries", "request_max_retries = 9"),
        ] {
            let env = TestEnvironment::new();
            let applied = legacy_applied_config("http://127.0.0.1:3211");
            let edited = match field {
                "wire_api" => applied.replace("wire_api = \"responses\"", replacement),
                "request_max_retries" => applied.replace("request_max_retries = 0", replacement),
                _ => unreachable!("covered legacy stanza field"),
            };
            env.write_config(&edited);
            env.write_legacy_state(legacy_state(2));
            let stored_state = std::fs::read(env.legacy_state_path()).expect("read legacy state");

            let error = apply(CodexSwitchIntent::Off)
                .expect_err("externally edited helper stanza must block automatic recovery");

            assert!(matches!(
                error,
                CodexSwitchError::LegacyRecoveryRequired { .. }
            ));
            assert_eq!(env.read_config(), edited);
            assert_eq!(
                std::fs::read(env.legacy_state_path()).expect("read preserved legacy state"),
                stored_state
            );
        }
    }

    #[test]
    fn switch_on_recovers_legacy_state_before_creating_current_journal() {
        let env = TestEnvironment::new();
        env.write_config(&legacy_applied_config("http://127.0.0.1:3111"));
        env.write_legacy_state(legacy_state(2));

        let outcome = apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("migrate legacy state and switch on");

        assert_eq!(outcome.change, CodexSwitchChange::Applied);
        assert!(!env.legacy_state_path().exists());
        assert!(env.state_path().exists());
        assert!(
            env.read_config()
                .contains("base_url = \"http://127.0.0.1:3211\"")
        );
        apply(CodexSwitchIntent::Off).expect("switch off current journal");
        assert_eq!(env.read_config(), "model_provider = \"openai\"\n");
    }

    #[test]
    fn legacy_auth_is_restored_only_while_helper_patch_still_matches() {
        let env = TestEnvironment::new();
        let original_auth = r#"{"OPENAI_API_KEY":"original-secret"}"#;
        let patched_auth = "{}";
        env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
        std::fs::write(env.auth_path(), "{\n}\n").expect("write semantically matching auth");
        let state = legacy_state_with_auth(false, Some(original_auth), patched_auth);
        env.write_legacy_state(state.clone());

        apply(CodexSwitchIntent::Off).expect("recover matching auth patch");

        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
            original_auth
        );

        env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
        std::fs::write(env.auth_path(), r#"{"external":"edit"}"#)
            .expect("write external auth edit");
        env.write_legacy_state(state);
        let edited_config = env.read_config();

        let stored_state = std::fs::read(env.legacy_state_path()).expect("read legacy state");
        let error = apply(CodexSwitchIntent::Off)
            .expect_err("external auth edit must block ambiguous automatic recovery");

        assert!(matches!(
            error,
            CodexSwitchError::LegacyRecoveryRequired { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read external auth"),
            r#"{"external":"edit"}"#
        );
        assert_eq!(env.read_config(), edited_config);
        assert_eq!(
            std::fs::read(env.legacy_state_path()).expect("read preserved legacy state"),
            stored_state
        );
    }

    #[test]
    fn legacy_recovery_is_retryable_after_each_durable_boundary() {
        let env = TestEnvironment::new();
        let original_config = "model_provider = \"openai\"\n";
        let original_auth = r#"{"OPENAI_API_KEY":"original-secret"}"#;
        let patched_auth = "{}";
        env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
        std::fs::write(env.auth_path(), patched_auth).expect("write patched auth");
        env.write_legacy_state(legacy_state_with_auth(
            false,
            Some(original_auth),
            patched_auth,
        ));

        assert!(matches!(
            apply_with_failpoint(
                CodexSwitchIntent::Off,
                ApplyFailpoint::AfterLegacyConfigRestore,
            ),
            Err(CodexSwitchError::InjectedFailure(
                "after_legacy_config_restore"
            ))
        ));
        assert_eq!(env.read_config(), original_config);
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read patched auth"),
            patched_auth
        );
        assert!(env.legacy_state_path().exists());

        assert!(matches!(
            apply_with_failpoint(
                CodexSwitchIntent::Off,
                ApplyFailpoint::AfterLegacyAuthRestore,
            ),
            Err(CodexSwitchError::InjectedFailure(
                "after_legacy_auth_restore"
            ))
        ));
        assert_eq!(env.read_config(), original_config);
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
            original_auth
        );
        assert!(env.legacy_state_path().exists());

        let outcome = apply(CodexSwitchIntent::Off).expect("finish legacy recovery");
        assert_eq!(outcome.change, CodexSwitchChange::Recovered);
        assert_eq!(env.read_config(), original_config);
        assert_eq!(
            std::fs::read_to_string(env.auth_path()).expect("read restored auth"),
            original_auth
        );
        assert!(!env.legacy_state_path().exists());
    }

    #[test]
    fn legacy_recovery_removes_auth_created_by_old_helper() {
        let env = TestEnvironment::new();
        let patched_auth = "{}";
        env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
        std::fs::write(env.auth_path(), patched_auth).expect("write patched auth");
        env.write_legacy_state(legacy_state_with_auth(true, None, patched_auth));

        apply(CodexSwitchIntent::Off).expect("recover legacy switch state");

        assert!(!env.auth_path().exists());
        assert!(!env.legacy_state_path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn config_only_legacy_recovery_does_not_inspect_unmanaged_auth() {
        use std::os::unix::fs::symlink;

        let env = TestEnvironment::new();
        let external_auth = env.root.join("external-auth.json");
        std::fs::write(&external_auth, r#"{"external":true}"#).expect("write external auth");
        symlink(&external_auth, env.auth_path()).expect("link unmanaged auth");
        env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
        env.write_legacy_state(legacy_state(1));

        apply(CodexSwitchIntent::Off).expect("recover config-only legacy state");

        assert_eq!(env.read_config(), "model_provider = \"openai\"\n");
        assert!(
            std::fs::symlink_metadata(env.auth_path())
                .expect("inspect unmanaged auth link")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(external_auth).expect("read external auth"),
            r#"{"external":true}"#
        );
        assert!(!env.legacy_state_path().exists());
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
    fn inactive_foreign_helper_stanza_round_trips_but_orphaned_active_config_is_rejected() {
        let env = TestEnvironment::new();
        let foreign = r#"model_provider = "openai"

[model_providers.codex_proxy] # preserve header
# secret-comment-canary-must-stay-private
name = 'foreign' # preserve inline comment
base_url = "https://foreign.example/v1"

[model_providers.codex_proxy.http_headers]
Authorization = "Bearer authorization-canary-must-stay-private"
"x-api-key" = "api-key-canary-must-stay-private"
"#;
        env.write_config(foreign);
        let expected_backup = helper_stanza_repr_from_document(
            env.config_path().as_path(),
            &foreign
                .parse::<DocumentMut>()
                .expect("parse foreign config"),
        )
        .expect("capture helper stanza representation")
        .expect("foreign helper stanza");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("temporarily replace inactive helper stanza");
        let state = std::fs::read_to_string(env.state_path()).expect("read switch journal");
        for secret in [
            "secret-comment-canary-must-stay-private",
            "authorization-canary-must-stay-private",
            "api-key-canary-must-stay-private",
            "Authorization",
            "x-api-key",
        ] {
            assert!(!state.contains(secret), "journal leaked {secret}");
        }
        assert!(!state.contains("original_helper_stanza"));
        assert_eq!(
            std::fs::read_to_string(env.helper_stanza_backup_path())
                .expect("read private helper stanza backup"),
            expected_backup
        );
        apply(CodexSwitchIntent::Off).expect("restore inactive helper stanza");
        assert_eq!(env.read_config(), foreign);
        assert!(env.helper_stanza_backup_files().is_empty());
        assert_eq!(
            inspect().expect("inspect inactive foreign stanza").phase,
            CodexSwitchPhase::Off
        );

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
    fn prepared_switch_repairs_missing_or_tampered_helper_stanza_backup_from_original_config() {
        for tampered in [false, true] {
            let env = TestEnvironment::new();
            let original = r#"model_provider = "openai"

[model_providers.codex_proxy]
name = "foreign"
base_url = "https://foreign.example/v1"

[model_providers.codex_proxy.http_headers]
Authorization = "Bearer prepared-recovery-secret"
"#;
            env.write_config(original);
            let on = CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            };
            assert!(matches!(
                apply_with_failpoint(on.clone(), ApplyFailpoint::AfterPrepared),
                Err(CodexSwitchError::InjectedFailure("after_prepared"))
            ));
            let backup = env.helper_stanza_backup_path();
            if tampered {
                std::fs::write(&backup, "tampered helper stanza backup")
                    .expect("tamper helper stanza backup");
            } else {
                std::fs::remove_file(&backup).expect("remove helper stanza backup");
            }

            assert_eq!(
                inspect().expect("inspect damaged prepared backup").phase,
                CodexSwitchPhase::RecoveryRequired
            );
            assert_eq!(
                apply(on).expect("repair backup and resume switch").change,
                CodexSwitchChange::Applied
            );
            assert_eq!(
                apply(CodexSwitchIntent::Off)
                    .expect("restore original helper stanza")
                    .change,
                CodexSwitchChange::Removed
            );
            assert_eq!(env.read_config(), original);
            drop(env);
        }
    }

    #[test]
    fn inline_helper_stanza_journal_migrates_to_private_backup_without_secret_fields() {
        let env = TestEnvironment::new();
        let original = r#"model_provider = "openai"

[model_providers.codex_proxy] # migration-secret-comment
name = "foreign"
base_url = "https://foreign.example/v1"
api_key = "migration-secret-api-key"
"#;
        env.write_config(original);
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        assert!(matches!(
            apply_with_failpoint(on.clone(), ApplyFailpoint::AfterPrepared),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));
        let original_backup_path = env.helper_stanza_backup_path();
        let repr = std::fs::read_to_string(&original_backup_path).expect("read stanza backup");
        let semantic = parse_helper_stanza_repr(original_backup_path.as_path(), repr.as_str())
            .expect("parse stanza backup");
        let mut legacy = serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(env.state_path()).expect("read switch journal"),
        )
        .expect("parse switch journal");
        let object = legacy.as_object_mut().expect("switch journal object");
        let operation_id = object
            .get("operation_id")
            .and_then(serde_json::Value::as_str)
            .expect("operation identifier")
            .to_string();
        object.insert(
            "version".to_string(),
            serde_json::Value::from(LEGACY_STATE_VERSION),
        );
        object.remove("helper_stanza_backup");
        object.insert(
            "original_helper_stanza".to_string(),
            serde_json::to_value(semantic).expect("serialize semantic stanza"),
        );
        object.insert(
            "original_helper_stanza_repr".to_string(),
            serde_json::Value::String(repr),
        );
        std::fs::write(
            env.state_path(),
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy journal"),
        )
        .expect("write legacy journal");
        std::fs::remove_file(original_backup_path).expect("remove new-format backup");

        assert_eq!(
            apply(on).expect("migrate inline stanza journal").change,
            CodexSwitchChange::Applied
        );
        let migrated = std::fs::read_to_string(env.state_path()).expect("read migrated journal");
        let migrated_json =
            serde_json::from_str::<serde_json::Value>(&migrated).expect("parse migrated journal");
        let expected_backup_file_name = format!(
            "{HELPER_STANZA_BACKUP_FILE_PREFIX}{operation_id}{HELPER_STANZA_BACKUP_FILE_SUFFIX}"
        );
        assert_eq!(
            migrated_json
                .get("version")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(STATE_VERSION))
        );
        assert_eq!(
            migrated_json
                .pointer("/helper_stanza_backup/backup_file_name")
                .and_then(serde_json::Value::as_str),
            Some(expected_backup_file_name.as_str())
        );
        assert!(!migrated.contains("original_helper_stanza"));
        assert!(!migrated.contains("migration-secret-comment"));
        assert!(!migrated.contains("migration-secret-api-key"));
        assert_eq!(env.helper_stanza_backup_files().len(), 1);
        apply(CodexSwitchIntent::Off).expect("restore migrated helper stanza");
        assert_eq!(env.read_config(), original);
    }

    #[test]
    fn inline_helper_stanza_migration_resumes_after_backup_persisted_before_v2_publish() {
        let env = TestEnvironment::new();
        let original = r#"model_provider = "openai"

[model_providers.codex_proxy] # interrupted-migration-secret-comment
name = "foreign"
base_url = "https://foreign.example/v1"
api_key = "interrupted-migration-secret-api-key"
"#;
        env.write_config(original);
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        assert!(matches!(
            apply_with_failpoint(on.clone(), ApplyFailpoint::AfterPrepared),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));

        let initial_backup_path = env.helper_stanza_backup_path();
        let repr = std::fs::read_to_string(&initial_backup_path).expect("read stanza backup");
        let semantic = parse_helper_stanza_repr(initial_backup_path.as_path(), repr.as_str())
            .expect("parse stanza backup");
        let mut legacy = serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(env.state_path()).expect("read switch journal"),
        )
        .expect("parse switch journal");
        let object = legacy.as_object_mut().expect("switch journal object");
        let operation_id = object
            .get("operation_id")
            .and_then(serde_json::Value::as_str)
            .expect("operation identifier")
            .to_string();
        object.insert(
            "version".to_string(),
            serde_json::Value::from(LEGACY_STATE_VERSION),
        );
        object.remove("helper_stanza_backup");
        object.insert(
            "original_helper_stanza".to_string(),
            serde_json::to_value(semantic).expect("serialize semantic stanza"),
        );
        object.insert(
            "original_helper_stanza_repr".to_string(),
            serde_json::Value::String(repr.clone()),
        );
        std::fs::write(
            env.state_path(),
            serde_json::to_vec_pretty(&legacy).expect("serialize legacy journal"),
        )
        .expect("write legacy journal");
        std::fs::remove_file(initial_backup_path).expect("remove new-format backup");

        let interrupted_backup_path = env.resolved_state_dir().join(format!(
            "{HELPER_STANZA_BACKUP_FILE_PREFIX}{operation_id}{HELPER_STANZA_BACKUP_FILE_SUFFIX}"
        ));
        atomic_write_text(
            interrupted_backup_path.as_path(),
            repr.as_str(),
            FilePermissions::Secure,
            None,
        )
        .expect("persist backup before interrupted journal publication");

        assert_eq!(
            apply(on)
                .expect("resume interrupted stanza migration")
                .change,
            CodexSwitchChange::Applied
        );
        let migrated = std::fs::read_to_string(env.state_path()).expect("read migrated journal");
        let migrated_json =
            serde_json::from_str::<serde_json::Value>(&migrated).expect("parse migrated journal");
        assert_eq!(
            migrated_json
                .get("version")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(STATE_VERSION))
        );
        let backup_files = env.helper_stanza_backup_files();
        assert_eq!(backup_files.len(), 1);
        assert_eq!(
            std::fs::canonicalize(&backup_files[0]).expect("resolve migrated backup"),
            std::fs::canonicalize(interrupted_backup_path)
                .expect("resolve interrupted migration backup")
        );
        assert!(!migrated.contains("original_helper_stanza"));
        assert!(!migrated.contains("interrupted-migration-secret-comment"));
        assert!(!migrated.contains("interrupted-migration-secret-api-key"));

        apply(CodexSwitchIntent::Off).expect("restore after interrupted migration");
        assert_eq!(env.read_config(), original);
    }

    #[test]
    fn unsupported_future_switch_state_is_preserved_without_mutation() {
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
        let mut future = serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(env.state_path()).expect("read switch journal"),
        )
        .expect("parse switch journal");
        future
            .as_object_mut()
            .expect("switch journal object")
            .insert(
                "version".to_string(),
                serde_json::Value::from(STATE_VERSION + 1),
            );
        let future_bytes = serde_json::to_vec_pretty(&future).expect("serialize future journal");
        std::fs::write(env.state_path(), &future_bytes).expect("write future journal");

        for error in [
            inspect().expect_err("future state must fail closed during inspection"),
            apply(CodexSwitchIntent::Off)
                .expect_err("future state must fail closed during mutation"),
        ] {
            assert!(matches!(error, CodexSwitchError::InvalidState { .. }));
            assert!(error.to_string().contains("unsupported state version"));
        }
        assert_eq!(env.read_config(), original);
        assert_eq!(
            std::fs::read(env.state_path()).expect("read preserved future journal"),
            future_bytes
        );
    }

    #[test]
    fn applied_switch_fails_closed_when_helper_stanza_backup_is_missing_or_tampered() {
        for tampered in [false, true] {
            let env = TestEnvironment::new();
            let original = r#"model_provider = "openai"

[model_providers.codex_proxy]
name = "foreign"
base_url = "https://foreign.example/v1"
api_key = "applied-recovery-secret"
"#;
            env.write_config(original);
            apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            })
            .expect("apply switch");
            let applied = env.read_config();
            let backup = env.helper_stanza_backup_path();
            if tampered {
                std::fs::write(&backup, "tampered helper stanza backup")
                    .expect("tamper helper stanza backup");
            } else {
                std::fs::remove_file(&backup).expect("remove helper stanza backup");
            }

            assert_eq!(
                inspect().expect("inspect damaged applied backup").phase,
                CodexSwitchPhase::RecoveryRequired
            );
            assert!(matches!(
                apply(CodexSwitchIntent::Off),
                Err(CodexSwitchError::RecoveryRequired { .. })
            ));
            assert_eq!(env.read_config(), applied);
            assert_ne!(env.read_config(), original);
            let state = std::fs::read_to_string(env.state_path()).expect("read recovery journal");
            assert!(!state.contains("applied-recovery-secret"));
            drop(env);
        }
    }

    #[test]
    fn malformed_legacy_switch_state_is_preserved_when_automatic_recovery_fails() {
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
        assert!(reason.contains("safe automatic recovery"));
        assert!(reason.contains("do not delete, edit, or share"));
        assert!(!reason.contains("preserved-auth-material"));

        for intent in [
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            CodexSwitchIntent::Off,
        ] {
            let error = apply(intent).expect_err("malformed legacy state must remain untouched");
            assert!(matches!(
                &error,
                CodexSwitchError::InvalidState { path, .. }
                    if path == &resolved_legacy_state
            ));
            let message = error.to_string();
            assert!(message.contains("codex-helper-switch-state.json"));
            assert!(!message.contains("preserved-auth-material"));
        }

        assert_eq!(env.read_config(), original);
        let preserved_metadata =
            std::fs::symlink_metadata(env.legacy_state_path()).expect("inspect preserved state");
        assert_eq!(preserved_metadata.len(), legacy_metadata.len());
        assert_eq!(preserved_metadata.file_type(), legacy_metadata.file_type());
        assert!(!env.state_path().exists());
        assert!(env.lock_path().exists());
    }

    #[test]
    fn unsupported_legacy_version_is_preserved_without_mutation() {
        let env = TestEnvironment::new();
        let original = legacy_applied_config("http://127.0.0.1:3211");
        env.write_config(&original);
        env.write_legacy_state(legacy_state(3));
        let stored_state = std::fs::read(env.legacy_state_path()).expect("read legacy state");

        let error = apply(CodexSwitchIntent::Off)
            .expect_err("unsupported legacy state must not be migrated");

        assert!(matches!(error, CodexSwitchError::InvalidState { .. }));
        assert!(
            error
                .to_string()
                .contains("unsupported legacy state version 3")
        );
        assert_eq!(env.read_config(), original);
        assert_eq!(
            std::fs::read(env.legacy_state_path()).expect("read preserved legacy state"),
            stored_state
        );
        assert!(!env.state_path().exists());
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
                .is_some_and(|reason| reason.contains("safe automatic recovery"))
        );
        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::LegacySwitchState { .. })
        ));
        assert!(std::fs::symlink_metadata(env.legacy_state_path()).is_ok());
        assert!(env.lock_path().exists());
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
            Err(CodexSwitchError::LegacySwitchStateConflict { .. })
        ));
        assert_eq!(env.read_config(), applied_config);
        assert_eq!(
            std::fs::read(env.state_path()).expect("read preserved current journal"),
            current_state
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_current_journal_conflicts_with_legacy_recovery() {
        use std::os::unix::fs::symlink;

        let env = TestEnvironment::new();
        env.write_config(&legacy_applied_config("http://127.0.0.1:3211"));
        env.write_legacy_state(legacy_state(2));
        std::fs::create_dir_all(env.state_path().parent().expect("state parent"))
            .expect("create state directory");
        symlink(env.root.join("missing-current-journal"), env.state_path())
            .expect("create dangling current journal");
        let stored_state = std::fs::read(env.legacy_state_path()).expect("read legacy state");

        let error = apply(CodexSwitchIntent::Off)
            .expect_err("any current journal path entry must conflict with legacy recovery");

        assert!(matches!(
            error,
            CodexSwitchError::LegacySwitchStateConflict { .. }
        ));
        assert!(std::fs::symlink_metadata(env.state_path()).is_ok());
        assert_eq!(
            std::fs::read(env.legacy_state_path()).expect("read preserved legacy state"),
            stored_state
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
    fn switch_off_preserves_unmanaged_codex_config_writeback() {
        let env = TestEnvironment::new();
        env.write_config(
            r#"# original config
model_provider = "openai"

[features]
remote_compaction_v2 = false
image_generation = true
unrelated_feature = true
"#,
        );
        let patch = CodexClientPatchConfig {
            preset: CodexClientPreset::OfficialRelay,
            compaction: CodexCompactionStrategy::RemoteV2,
            ..CodexClientPatchConfig::default()
        };
        apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            },
            patch,
        )
        .expect("switch on with one managed feature");
        let journal = std::fs::read_to_string(env.state_path()).expect("read switch journal");
        let journal_json =
            serde_json::from_str::<serde_json::Value>(&journal).expect("parse switch journal");
        assert_eq!(
            journal_json
                .get("version")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(STATE_VERSION))
        );
        assert!(!journal.contains("pending_config"));

        let mut edited = env.read_config();
        edited = edited.replace("unrelated_feature = true", "unrelated_feature = false");
        edited = edited.replace("image_generation = true", "image_generation = false");
        edited.push_str(
            r#"
# written by Codex
[projects."/external"]
trust_level = "trusted"

[notice]
hide_full_access_warning = true

[mcp_servers.docs]
command = "docs"
"#,
        );
        env.write_config(edited.as_str());

        assert_eq!(
            inspect().expect("inspect unmanaged writeback").phase,
            CodexSwitchPhase::Applied
        );
        let outcome = apply(CodexSwitchIntent::Off).expect("merge switch-off restoration");

        assert_eq!(outcome.change, CodexSwitchChange::Removed);
        let restored = env.read_config();
        assert!(restored.contains("# original config"));
        assert!(restored.contains("# written by Codex"));
        assert!(restored.contains("model_provider = \"openai\""));
        assert!(restored.contains("remote_compaction_v2 = false"));
        assert!(restored.contains("image_generation = false"));
        assert!(restored.contains("unrelated_feature = false"));
        assert!(restored.contains("[projects.\"/external\"]"));
        assert!(restored.contains("[notice]"));
        assert!(restored.contains("[mcp_servers.docs]"));
        assert!(!restored.contains("model_providers.codex_proxy"));
        assert!(!env.state_path().exists());
    }

    #[test]
    fn prepared_on_rejects_writeback_but_prepared_off_merges_it() {
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

            let outcome =
                apply(CodexSwitchIntent::Off).expect("merge edit into prepared switch-off");
            assert_eq!(outcome.change, CodexSwitchChange::Removed);
            let restored = env.read_config();
            assert!(restored.contains("model_provider = \"openai\""));
            assert!(restored.contains("# external edit"));
            assert!(!restored.contains("model_providers.codex_proxy"));
        }
    }

    #[test]
    fn switch_off_rejects_helper_owned_projection_conflicts() {
        {
            let env = TestEnvironment::new();
            env.write_config("model_provider = \"openai\"\n");
            apply(CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(3211),
            })
            .expect("switch on");
            let edited = env.read_config().replace(
                "model_provider = \"codex_proxy\"",
                "model_provider = \"external\"",
            );
            env.write_config(edited.as_str());

            assert_eq!(
                inspect().expect("inspect selector conflict").phase,
                CodexSwitchPhase::RecoveryRequired
            );
            assert!(matches!(
                apply(CodexSwitchIntent::Off),
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
            let edited = env.read_config().replace(
                "base_url = \"http://127.0.0.1:3211\"",
                "base_url = \"https://external.example/v1\"",
            );
            env.write_config(edited.as_str());

            assert_eq!(
                inspect().expect("inspect stanza conflict").phase,
                CodexSwitchPhase::RecoveryRequired
            );
            assert!(matches!(
                apply(CodexSwitchIntent::Off),
                Err(CodexSwitchError::RecoveryRequired { .. })
            ));
            assert_eq!(env.read_config(), edited);
        }

        {
            let env = TestEnvironment::new();
            env.write_config(
                "model_provider = \"openai\"\n\n[features]\nremote_compaction_v2 = false\n",
            );
            let patch = CodexClientPatchConfig {
                preset: CodexClientPreset::OfficialRelay,
                compaction: CodexCompactionStrategy::RemoteV2,
                ..CodexClientPatchConfig::default()
            };
            apply_with_client_patch(
                CodexSwitchIntent::On {
                    validated_base_url: ValidatedCodexBaseUrl::local(3211),
                },
                patch,
            )
            .expect("switch on with managed feature");
            let edited = env.read_config().replace(
                "remote_compaction_v2 = true",
                "remote_compaction_v2 = false",
            );
            env.write_config(edited.as_str());

            assert_eq!(
                inspect().expect("inspect managed feature conflict").phase,
                CodexSwitchPhase::RecoveryRequired
            );
            assert!(matches!(
                apply(CodexSwitchIntent::Off),
                Err(CodexSwitchError::RecoveryRequired { .. })
            ));
            assert_eq!(env.read_config(), edited);
        }
    }

    #[test]
    fn switch_off_keeps_comment_only_writeback_when_original_config_was_absent() {
        let env = TestEnvironment::new();
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on without an original config");
        let mut edited = env.read_config();
        edited.push_str("\n# written by Codex while switched\n");
        env.write_config(edited.as_str());

        apply(CodexSwitchIntent::Off).expect("restore while preserving comment-only writeback");

        let restored = env.read_config();
        assert!(restored.contains("# written by Codex while switched"));
        let parsed =
            toml::from_str::<TomlValue>(restored.as_str()).expect("parse comment-only config");
        assert!(parsed.as_table().is_some_and(|table| table.is_empty()));
        assert!(!env.state_path().exists());
    }

    #[test]
    fn different_codex_homes_have_independent_switch_journals() {
        let env = TestEnvironment::new();
        let first_original = "model_provider = \"openai\"\n";
        env.write_config(first_original);
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on first Codex home");
        let first_applied = env.read_config();
        let first_state_path = env.state_path();
        let first_state = std::fs::read(&first_state_path).expect("read first state");

        let other_codex_home = env.root.join("other-codex");
        std::fs::create_dir_all(&other_codex_home).expect("create other Codex home");
        let other_config = other_codex_home.join("config.toml");
        let second_original = "model_provider = \"custom\"\n";
        std::fs::write(&other_config, second_original).expect("write other config");
        let second_state_path = env.state_path_for_home(other_codex_home.as_path());
        assert_ne!(first_state_path, second_state_path);
        unsafe {
            std::env::set_var("CODEX_HOME", &other_codex_home);
        }

        let second_before = inspect().expect("inspect second Codex home");
        assert_eq!(second_before.phase, CodexSwitchPhase::Off);
        assert_eq!(second_before.state_path, second_state_path);
        assert_eq!(env.read_config(), first_applied);
        assert_eq!(
            std::fs::read(&first_state_path).expect("re-read first state"),
            first_state
        );

        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(4321),
        })
        .expect("switch on second Codex home");
        assert!(first_state_path.exists());
        assert!(second_state_path.exists());
        assert!(
            std::fs::read_to_string(&other_config)
                .expect("read applied second config")
                .contains("http://127.0.0.1:4321")
        );

        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("switch off second Codex home")
                .change,
            CodexSwitchChange::Removed
        );
        assert_eq!(
            std::fs::read_to_string(&other_config).unwrap(),
            second_original
        );
        assert!(!second_state_path.exists());
        assert_eq!(env.read_config(), first_applied);
        assert_eq!(std::fs::read(&first_state_path).unwrap(), first_state);

        unsafe {
            std::env::set_var("CODEX_HOME", &env.codex_home);
        }
        assert_eq!(
            inspect().expect("inspect first Codex home").phase,
            CodexSwitchPhase::Applied
        );
        apply(CodexSwitchIntent::Off).expect("switch off first Codex home");
        assert_eq!(env.read_config(), first_original);
    }

    #[test]
    fn matching_unscoped_journal_is_adopted_and_migrated_without_rewriting() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let target = ValidatedCodexBaseUrl::local(3211);
        apply(CodexSwitchIntent::On {
            validated_base_url: target.clone(),
        })
        .expect("create scoped state");
        let scoped_state = env.state_path();
        let unscoped_state = env.unscoped_state_path();
        let state_bytes = std::fs::read(&scoped_state).expect("read scoped state");
        std::fs::rename(&scoped_state, &unscoped_state).expect("downgrade to unscoped state");

        let adopted = inspect().expect("inspect matching unscoped state");
        assert_eq!(adopted.phase, CodexSwitchPhase::Applied);
        assert_eq!(adopted.state_path, unscoped_state);
        assert!(!scoped_state.exists());

        let outcome = apply(CodexSwitchIntent::On {
            validated_base_url: target,
        })
        .expect("migrate matching unscoped state");
        assert_eq!(outcome.change, CodexSwitchChange::Unchanged);
        assert_eq!(outcome.status.state_path, scoped_state);
        assert!(!unscoped_state.exists());
        assert_eq!(std::fs::read(&scoped_state).unwrap(), state_bytes);
    }

    #[test]
    fn mismatched_unscoped_journal_does_not_block_another_codex_home() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on first Codex home");
        let first_applied = env.read_config();
        let first_scoped_state = env.state_path();
        let unscoped_state = env.unscoped_state_path();
        std::fs::rename(&first_scoped_state, &unscoped_state)
            .expect("downgrade first state to unscoped path");
        let unscoped_bytes = std::fs::read(&unscoped_state).expect("read unscoped state");

        let other_codex_home = env.root.join("other-codex");
        std::fs::create_dir_all(&other_codex_home).expect("create other Codex home");
        let other_config = other_codex_home.join("config.toml");
        let other_original = "model_provider = \"custom\"\n";
        std::fs::write(&other_config, other_original).expect("write other config");
        let other_state = env.state_path_for_home(other_codex_home.as_path());
        unsafe {
            std::env::set_var("CODEX_HOME", &other_codex_home);
        }

        assert_eq!(
            inspect().expect("inspect other home").phase,
            CodexSwitchPhase::Off
        );
        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("no-op switch off in other home")
                .change,
            CodexSwitchChange::Unchanged
        );
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(4321),
        })
        .expect("switch on other home despite mismatched unscoped state");
        assert!(other_state.exists());
        assert_eq!(std::fs::read(&unscoped_state).unwrap(), unscoped_bytes);
        assert_eq!(env.read_config(), first_applied);

        apply(CodexSwitchIntent::Off).expect("switch off other home");
        assert_eq!(
            std::fs::read_to_string(&other_config).unwrap(),
            other_original
        );
        assert_eq!(std::fs::read(&unscoped_state).unwrap(), unscoped_bytes);

        unsafe {
            std::env::set_var("CODEX_HOME", &env.codex_home);
        }
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("migrate first home state after returning");
        assert!(first_scoped_state.exists());
        assert!(!unscoped_state.exists());
    }

    #[cfg(unix)]
    #[test]
    fn codex_home_symlink_aliases_share_the_resolved_switch_scope() {
        use std::os::unix::fs::symlink;

        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let codex_home_alias = env.root.join("codex-home-alias");
        symlink(&env.codex_home, &codex_home_alias).expect("link Codex home alias");
        unsafe {
            std::env::set_var("CODEX_HOME", &codex_home_alias);
        }

        let applied = apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on through alias");
        assert_eq!(applied.status.state_path, env.state_path());

        unsafe {
            std::env::set_var("CODEX_HOME", &env.codex_home);
        }
        let direct = inspect().expect("inspect through real Codex home");
        assert_eq!(direct.phase, CodexSwitchPhase::Applied);
        assert_eq!(direct.state_path, env.state_path());
        apply(CodexSwitchIntent::Off).expect("switch off through real Codex home");
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
        let original_state = std::fs::read(env.state_path()).expect("read original state");

        let other_codex_home = env.root.join("retargeted-codex");
        std::fs::create_dir_all(&other_codex_home).expect("create retargeted Codex home");
        let other_config = other_codex_home.join("config.toml");
        std::fs::write(&other_config, &applied).expect("write matching retargeted config");
        std::fs::remove_file(&codex_home_link).expect("remove first home link");
        symlink(&other_codex_home, &codex_home_link).expect("retarget Codex home link");

        assert!(matches!(
            apply(CodexSwitchIntent::Off),
            Err(CodexSwitchError::OrphanedActiveProvider)
        ));
        assert_eq!(
            std::fs::read_to_string(other_config).expect("read retargeted config"),
            applied
        );
        assert_eq!(env.read_config(), applied);
        assert_eq!(std::fs::read(env.state_path()).unwrap(), original_state);

        unsafe {
            std::env::set_var("CODEX_HOME", &env.codex_home);
        }
    }

    #[test]
    fn service_restore_uses_explicit_client_home_and_exact_applied_target() {
        let env = TestEnvironment::new();
        let original = "model_provider = \"openai\"\n";
        env.write_config(original);
        let target = ValidatedCodexBaseUrl::local(3211);
        apply(CodexSwitchIntent::On {
            validated_base_url: target.clone(),
        })
        .expect("switch on service target");

        let other_codex_home = env.root.join("other-codex");
        std::fs::create_dir_all(&other_codex_home).expect("create other Codex home");
        unsafe {
            std::env::set_var("CODEX_HOME", &other_codex_home);
        }
        let outcome = restore_managed_applied_for_target(env.codex_home.as_path(), &target)
            .expect("restore explicit service client home");

        assert!(matches!(
            outcome,
            CodexSwitchTargetRestoreOutcome::Restored(CodexSwitchOutcome {
                change: CodexSwitchChange::Removed,
                ..
            })
        ));
        assert_eq!(env.read_config(), original);
        assert!(!env.state_path().exists());
    }

    #[test]
    fn service_restore_recovers_a_matching_v0203_switch_target() {
        let env = TestEnvironment::new();
        let target = ValidatedCodexBaseUrl::local(3211);
        env.write_config(&legacy_applied_config(target.as_str()));
        env.write_legacy_state(legacy_state(2));

        let outcome = restore_managed_applied_for_target(env.codex_home.as_path(), &target)
            .expect("restore matching v0.20.3 service target");

        assert!(matches!(
            outcome,
            CodexSwitchTargetRestoreOutcome::Restored(CodexSwitchOutcome {
                change: CodexSwitchChange::Recovered,
                ..
            })
        ));
        assert_eq!(env.read_config(), "model_provider = \"openai\"\n");
        assert!(!env.legacy_state_path().exists());
        assert!(!env.state_path().exists());
    }

    #[test]
    fn service_restore_preserves_a_different_v0203_switch_target() {
        let env = TestEnvironment::new();
        let configured_target = ValidatedCodexBaseUrl::local(4321);
        let expected_target = ValidatedCodexBaseUrl::local(3211);
        env.write_config(&legacy_applied_config(configured_target.as_str()));
        env.write_legacy_state(legacy_state(2));
        let config_before = env.read_config();
        let state_before =
            std::fs::read(env.legacy_state_path()).expect("read legacy switch state");

        let outcome =
            restore_managed_applied_for_target(env.codex_home.as_path(), &expected_target)
                .expect("inspect a different v0.20.3 service target");

        assert!(matches!(
            outcome,
            CodexSwitchTargetRestoreOutcome::Unchanged(CodexSwitchStatus {
                phase: CodexSwitchPhase::RecoveryRequired,
                enabled: true,
                managed: false,
                ..
            })
        ));
        assert_eq!(env.read_config(), config_before);
        assert_eq!(
            std::fs::read(env.legacy_state_path()).expect("reread legacy switch state"),
            state_before
        );
    }

    #[test]
    fn service_restore_does_not_modify_a_different_local_or_remote_target() {
        for configured_target in [
            ValidatedCodexBaseUrl::local(4321),
            ValidatedCodexBaseUrl::parse("https://relay.example/v1").expect("remote target"),
        ] {
            let env = TestEnvironment::new();
            env.write_config("model_provider = \"openai\"\n");
            apply(CodexSwitchIntent::On {
                validated_base_url: configured_target.clone(),
            })
            .expect("switch on different target");
            let config_before = env.read_config();
            let state_before = std::fs::read(env.state_path()).expect("read switch state");

            let outcome = restore_managed_applied_for_target(
                env.codex_home.as_path(),
                &ValidatedCodexBaseUrl::local(3211),
            )
            .expect("inspect different target without restoring it");

            assert!(matches!(
                outcome,
                CodexSwitchTargetRestoreOutcome::Unchanged(CodexSwitchStatus {
                    phase: CodexSwitchPhase::Applied,
                    enabled: true,
                    managed: true,
                    ..
                })
            ));
            assert_eq!(env.read_config(), config_before);
            assert_eq!(
                std::fs::read(env.state_path()).expect("re-read switch state"),
                state_before
            );
        }
    }

    #[test]
    fn service_restore_merges_unmanaged_codex_config_writeback() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let target = ValidatedCodexBaseUrl::local(3211);
        apply(CodexSwitchIntent::On {
            validated_base_url: target.clone(),
        })
        .expect("switch on service target");
        let edited = format!("{}\n# external edit\n", env.read_config());
        env.write_config(&edited);

        let outcome = restore_managed_applied_for_target(env.codex_home.as_path(), &target)
            .expect("restore externally edited target");

        assert!(matches!(
            outcome,
            CodexSwitchTargetRestoreOutcome::Restored(CodexSwitchOutcome {
                change: CodexSwitchChange::Removed,
                ..
            })
        ));
        let restored = env.read_config();
        assert!(restored.contains("model_provider = \"openai\""));
        assert!(restored.contains("# external edit"));
        assert!(!restored.contains("model_providers.codex_proxy"));
        assert!(!env.state_path().exists());
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
    fn restored_journal_recovers_after_helper_stanza_backup_cleanup_interruption() {
        let env = TestEnvironment::new();
        let original = r#"model_provider = "openai"

[model_providers.codex_proxy] # preserved comment
name = "foreign"
base_url = "https://foreign.example/v1"
api_key = "cleanup-interruption-secret"
"#;
        env.write_config(original);
        apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        })
        .expect("switch on");

        assert!(matches!(
            apply_with_failpoint(
                CodexSwitchIntent::Off,
                ApplyFailpoint::AfterHelperStanzaBackupRemoval
            ),
            Err(CodexSwitchError::InjectedFailure(
                "after_helper_stanza_backup_removal"
            ))
        ));
        assert_eq!(env.read_config(), original);
        assert!(env.state_path().exists());
        assert!(env.helper_stanza_backup_files().is_empty());
        assert_eq!(
            inspect().expect("inspect interrupted cleanup").phase,
            CodexSwitchPhase::Off
        );
        assert_eq!(
            project_original_codex_config(|config, _| config.map(str::to_string))
                .expect("project original config"),
            Some(original.to_string())
        );

        assert_eq!(
            apply(CodexSwitchIntent::Off)
                .expect("finish interrupted cleanup")
                .change,
            CodexSwitchChange::Recovered
        );
        assert!(!env.state_path().exists());
        assert!(env.helper_stanza_backup_files().is_empty());
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
