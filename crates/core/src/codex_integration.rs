use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::client_config::{
    CLAUDE_ABSENT_BACKUP_SENTINEL, claude_settings_backup_path_for as claude_settings_backup_path,
    claude_settings_path,
};
#[cfg(test)]
use crate::file_replace::write_text_private_file;
use crate::file_replace::{
    ManagedFileSnapshot, ManagedFileTransaction, ManagedFileTransactionError,
    read_managed_file_snapshot,
};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CLAUDE_SWITCH_STATE_SCHEMA_VERSION: u32 = 1;
const CLAUDE_SETTINGS_MAX_BYTES: usize = 16 * 1024 * 1024;

#[cfg(test)]
fn atomic_write(path: &Path, data: &str) -> Result<()> {
    write_text_private_file(path, data)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClaudeSettingsSnapshot {
    Absent,
    Present(String),
}

impl ClaudeSettingsSnapshot {
    fn from_managed(snapshot: &ManagedFileSnapshot, path: &Path) -> Result<Self> {
        snapshot
            .bytes()
            .map(|bytes| {
                String::from_utf8(bytes.to_vec())
                    .map(Self::Present)
                    .with_context(|| format!("parse {:?} as UTF-8", path))
            })
            .unwrap_or(Ok(Self::Absent))
    }

    fn read(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Self::Present)
                .with_context(|| format!("parse {:?} as UTF-8", path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::Absent),
            Err(error) => Err(error).with_context(|| format!("read {:?}", path)),
        }
    }

    fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        match self {
            Self::Absent => digest.update(b"claude-settings-v1:absent"),
            Self::Present(text) => {
                digest.update(b"claude-settings-v1:present\0");
                digest.update(text.as_bytes());
            }
        }
        format!("sha256:{:x}", digest.finalize())
    }

    fn backup_text(&self) -> &str {
        match self {
            Self::Absent => CLAUDE_ABSENT_BACKUP_SENTINEL,
            Self::Present(text) => text,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeSwitchPreparedFrom {
    applied_fingerprint: String,
    base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeSwitchState {
    schema_version: u32,
    backup_fingerprint: String,
    original_absent: bool,
    original_fingerprint: String,
    applied_fingerprint: String,
    base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prepared_from: Option<ClaudeSwitchPreparedFrom>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auto_restore_generation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSwitchRestoreLease {
    settings_path: PathBuf,
    backup_path: PathBuf,
    state_path: PathBuf,
    backup_fingerprint: String,
    applied_fingerprint: String,
    auto_restore_generation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeSwitchFilePhase {
    OriginalPrepared,
    Applied,
    PreviousAppliedPrepared,
}

fn claude_switch_state_path(backup_path: &Path) -> PathBuf {
    let mut path = backup_path.to_path_buf();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings.json.codex-helper-backup".to_string());
    path.set_file_name(format!("{file_name}.state.json"));
    path
}

fn file_fingerprint(text: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"claude-switch-file-v1\0");
    digest.update(text.as_bytes());
    format!("sha256:{:x}", digest.finalize())
}

#[cfg(test)]
fn read_required_text(path: &Path) -> Result<String> {
    let snapshot = read_managed_file_snapshot(path, CLAUDE_SETTINGS_MAX_BYTES)
        .with_context(|| format!("read managed Claude switch file {:?}", path))?;
    managed_snapshot_required_text(&snapshot, path)
}

fn managed_snapshot_required_text(snapshot: &ManagedFileSnapshot, path: &Path) -> Result<String> {
    let bytes = snapshot.bytes().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Claude switch file {:?} is missing", path),
        )
    })?;
    String::from_utf8(bytes.to_vec()).with_context(|| format!("parse {:?} as UTF-8", path))
}

fn load_claude_switch_state_from_snapshot(
    snapshot: &ManagedFileSnapshot,
    path: &Path,
) -> Result<Option<ClaudeSwitchState>> {
    let Some(_) = snapshot.bytes() else {
        return Ok(None);
    };
    let text = managed_snapshot_required_text(snapshot, path)?;
    parse_claude_switch_state(&text, path).map(Some)
}

fn parse_claude_switch_state(text: &str, path: &Path) -> Result<ClaudeSwitchState> {
    let state: ClaudeSwitchState =
        serde_json::from_str(text).with_context(|| format!("parse {:?}", path))?;
    if state.schema_version != CLAUDE_SWITCH_STATE_SCHEMA_VERSION {
        return Err(anyhow!(
            "unsupported Claude switch state version {}; expected {}",
            state.schema_version,
            CLAUDE_SWITCH_STATE_SCHEMA_VERSION
        ));
    }
    Ok(state)
}

fn original_snapshot_from_backup(
    backup_text: &str,
    original_absent: Option<bool>,
) -> ClaudeSettingsSnapshot {
    if original_absent.unwrap_or_else(|| backup_text.trim() == CLAUDE_ABSENT_BACKUP_SENTINEL) {
        ClaudeSettingsSnapshot::Absent
    } else {
        ClaudeSettingsSnapshot::Present(backup_text.to_string())
    }
}

fn claude_base_url(snapshot: &ClaudeSettingsSnapshot) -> Option<String> {
    let ClaudeSettingsSnapshot::Present(text) = snapshot else {
        return None;
    };
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| {
            value
                .as_object()
                .and_then(|object| object.get("env"))
                .and_then(serde_json::Value::as_object)
                .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
        })
}

fn render_claude_patch(
    original: &ClaudeSettingsSnapshot,
    base_url: &str,
) -> Result<ClaudeSettingsSnapshot> {
    let mut value: serde_json::Value = match original {
        ClaudeSettingsSnapshot::Absent => serde_json::json!({}),
        ClaudeSettingsSnapshot::Present(text) if text.trim().is_empty() => serde_json::json!({}),
        ClaudeSettingsSnapshot::Present(text) => {
            serde_json::from_str(text).context("parse the backed-up Claude settings as JSON")?
        }
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("Claude settings root must be an object"))?;
    let env = object
        .entry("env".to_string())
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("Claude settings env must be an object"))?;
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        serde_json::Value::String(base_url.to_string()),
    );
    Ok(ClaudeSettingsSnapshot::Present(
        serde_json::to_string_pretty(&value)?,
    ))
}

fn recorded_claude_patch(
    original: &ClaudeSettingsSnapshot,
    applied_fingerprint: &str,
    base_url: &str,
    description: &str,
) -> Result<ClaudeSettingsSnapshot> {
    let normalized =
        crate::control_plane_client::normalize_base_url(base_url).ok_or_else(|| {
            anyhow!("Claude switch {description} base URL is invalid; refusing automatic recovery")
        })?;
    if normalized != base_url {
        return Err(anyhow!(
            "Claude switch {description} base URL is not normalized; refusing automatic recovery"
        ));
    }
    let expected = render_claude_patch(original, &normalized)?;
    if applied_fingerprint != expected.fingerprint() {
        return Err(anyhow!(
            "Claude switch {description} no longer reproduces the recorded helper patch"
        ));
    }
    Ok(expected)
}

fn prepared_from_current_patch(
    original: &ClaudeSettingsSnapshot,
    current: &ClaudeSettingsSnapshot,
) -> Result<ClaudeSwitchPreparedFrom> {
    let base_url = claude_base_url(current).ok_or_else(|| {
        anyhow!(
            "Claude settings are not a recognizable helper patch; refusing to overwrite external edits"
        )
    })?;
    let base_url = crate::control_plane_client::normalize_base_url(&base_url).ok_or_else(|| {
        anyhow!(
            "Claude settings have an invalid helper patch base URL; refusing to overwrite external edits"
        )
    })?;
    let expected = render_claude_patch(original, &base_url)?;
    if current != &expected {
        return Err(anyhow!(
            "Claude settings differ from the helper-generated patch; refusing to overwrite external edits"
        ));
    }
    Ok(ClaudeSwitchPreparedFrom {
        applied_fingerprint: current.fingerprint(),
        base_url,
    })
}

#[cfg(test)]
fn load_claude_switch_state(path: &Path) -> Result<Option<ClaudeSwitchState>> {
    let snapshot = read_managed_file_snapshot(path, CLAUDE_SETTINGS_MAX_BYTES)
        .with_context(|| format!("read managed Claude switch file {:?}", path))?;
    load_claude_switch_state_from_snapshot(&snapshot, path)
}

struct ClaudeSwitchTransactions {
    settings: ManagedFileTransaction,
    backup: ManagedFileTransaction,
    state: ManagedFileTransaction,
}

impl ClaudeSwitchTransactions {
    fn begin(settings_path: &Path, backup_path: &Path, state_path: &Path) -> Result<Self> {
        // Every mutating Claude operation takes these locks in the same order.
        let settings = ManagedFileTransaction::begin(settings_path, CLAUDE_SETTINGS_MAX_BYTES)
            .with_context(|| {
                format!("begin managed Claude settings transaction for {settings_path:?}")
            })?;
        let backup = ManagedFileTransaction::begin(backup_path, CLAUDE_SETTINGS_MAX_BYTES)
            .with_context(|| {
                format!("begin managed Claude backup transaction for {backup_path:?}")
            })?;
        let state = ManagedFileTransaction::begin(state_path, CLAUDE_SETTINGS_MAX_BYTES)
            .with_context(|| {
                format!("begin managed Claude state transaction for {state_path:?}")
            })?;
        Ok(Self {
            settings,
            backup,
            state,
        })
    }

    fn verify_current(&self) -> std::result::Result<(), ManagedFileTransactionError> {
        self.settings.verify_current()?;
        self.backup.verify_current()?;
        self.state.verify_current()
    }

    fn verify_recovery_material(&self) -> std::result::Result<(), ManagedFileTransactionError> {
        self.backup.verify_current()?;
        self.state.verify_current()
    }
}

fn claude_recovery_required(operation: &str, error: impl std::fmt::Display) -> anyhow::Error {
    anyhow!(
        "Claude switch recovery required: settings or recovery material changed while {operation}; recovery material was preserved: {error}"
    )
}

fn ensure_claude_switch_files_current(
    files: &ClaudeSwitchTransactions,
    operation: &str,
) -> Result<()> {
    files
        .verify_current()
        .map_err(|error| claude_recovery_required(operation, error))
}

fn ensure_claude_recovery_material_current(
    files: &ClaudeSwitchTransactions,
    operation: &str,
) -> Result<()> {
    files
        .verify_recovery_material()
        .map_err(|error| claude_recovery_required(operation, error))
}

fn restore_and_cleanup_claude_switch(
    files: &mut ClaudeSwitchTransactions,
    settings_path: &Path,
    backup_path: &Path,
    state_path: &Path,
    current: &ClaudeSettingsSnapshot,
    phase: ClaudeSwitchFilePhase,
    original: &ClaudeSettingsSnapshot,
) -> Result<()> {
    ensure_claude_switch_files_current(files, "restoring Claude settings")?;
    if phase != ClaudeSwitchFilePhase::OriginalPrepared {
        restore_claude_settings_snapshot(&mut files.settings, settings_path, original)
            .map_err(|error| claude_recovery_required("restoring Claude settings", error))?;
    }

    // Revalidate the complete recovery bundle before cleanup. The backup is removed first so a
    // later state conflict leaves an explicit state-without-backup recovery-required marker,
    // rather than falling back to an unverified legacy backup.
    if let Err(error) =
        ensure_claude_switch_files_current(files, "removing Claude recovery material")
    {
        if phase != ClaudeSwitchFilePhase::OriginalPrepared {
            restore_claude_settings_snapshot(&mut files.settings, settings_path, current).context(
                "Claude switch recovery required: recovery material changed after settings were restored; failed to return settings to the preceding verified helper patch",
            )?;
            return Err(error.context(
                "Claude switch recovery required: recovery material changed after settings were restored; retained recovery material and returned settings to the preceding verified helper patch",
            ));
        }
        return Err(error.context(
            "Claude switch recovery required: recovery material changed while settings were already restored; retained recovery material",
        ));
    }
    files.backup.remove().map_err(|error| {
        claude_recovery_required(
            &format!("removing Claude settings backup {backup_path:?}"),
            error,
        )
    })?;
    files.state.remove().map_err(|error| {
        claude_recovery_required(
            &format!("removing Claude switch state {state_path:?}"),
            error,
        )
    })?;
    Ok(())
}

fn validate_claude_switch_files(
    current: &ClaudeSettingsSnapshot,
    backup_text: &str,
    state: Option<&ClaudeSwitchState>,
) -> Result<(ClaudeSwitchFilePhase, ClaudeSettingsSnapshot)> {
    let original =
        original_snapshot_from_backup(backup_text, state.map(|state| state.original_absent));
    if let Some(state) = state {
        if state.backup_fingerprint != file_fingerprint(backup_text) {
            return Err(anyhow!(
                "Claude switch backup no longer matches its recorded fingerprint"
            ));
        }
        if state.original_fingerprint != original.fingerprint() {
            return Err(anyhow!(
                "Claude switch original snapshot no longer matches its recorded fingerprint"
            ));
        }
        let expected = recorded_claude_patch(
            &original,
            &state.applied_fingerprint,
            &state.base_url,
            "state",
        )?;
        if current == &original {
            return Ok((ClaudeSwitchFilePhase::OriginalPrepared, original));
        }
        if current == &expected {
            return Ok((ClaudeSwitchFilePhase::Applied, original));
        }
        if let Some(prepared_from) = &state.prepared_from {
            let previous = recorded_claude_patch(
                &original,
                &prepared_from.applied_fingerprint,
                &prepared_from.base_url,
                "prepared predecessor",
            )?;
            if current == &previous {
                return Ok((ClaudeSwitchFilePhase::PreviousAppliedPrepared, original));
            }
        }
        return Err(anyhow!(
            "Claude settings match neither the prepared original nor a verified helper patch; refusing to overwrite external edits"
        ));
    }

    if current == &original {
        return Ok((ClaudeSwitchFilePhase::OriginalPrepared, original));
    }
    let base_url = claude_base_url(current).ok_or_else(|| {
        anyhow!(
            "legacy Claude settings backup exists, but the current file is not a recognizable helper patch; refusing to overwrite external edits"
        )
    })?;
    let normalized = crate::control_plane_client::normalize_base_url(&base_url).ok_or_else(|| {
        anyhow!(
            "legacy Claude settings backup exists, but the current base URL is invalid; refusing to overwrite external edits"
        )
    })?;
    let expected = render_claude_patch(&original, &normalized)?;
    if current != &expected {
        return Err(anyhow!(
            "legacy Claude settings backup exists, but the current file differs from the helper-generated patch; refusing to overwrite external edits"
        ));
    }
    Ok((ClaudeSwitchFilePhase::Applied, original))
}

fn restore_claude_settings_snapshot(
    settings: &mut ManagedFileTransaction,
    settings_path: &Path,
    original: &ClaudeSettingsSnapshot,
) -> Result<()> {
    match original {
        ClaudeSettingsSnapshot::Absent => settings
            .remove()
            .with_context(|| format!("restore absent Claude settings {:?}", settings_path)),
        ClaudeSettingsSnapshot::Present(text) => settings
            .replace(text.as_bytes())
            .with_context(|| format!("restore Claude settings {:?}", settings_path)),
    }
}

fn restore_retryable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<ManagedFileTransactionError>()
            .is_some_and(|source| matches!(source, ManagedFileTransactionError::Busy { .. }))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexStartupReadinessSeverity {
    Info,
    Warning,
}

impl CodexStartupReadinessSeverity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexStartupReadinessIssueKind {
    SwitchDisabled,
    SwitchPortMismatch,
    DiagnosticError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexStartupReadinessIssue {
    pub kind: CodexStartupReadinessIssueKind,
    pub severity: CodexStartupReadinessSeverity,
    pub title: String,
    pub detail: String,
    pub action: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexStartupReadiness {
    pub issues: Vec<CodexStartupReadinessIssue>,
}

impl CodexStartupReadiness {
    pub fn has_issues(&self) -> bool {
        !self.issues.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct ClaudeSwitchStatus {
    /// Whether Claude Code currently matches a verified helper proxy patch.
    pub enabled: bool,
    /// Current `env.ANTHROPIC_BASE_URL` value, when present.
    pub base_url: Option<String>,
    /// Whether a backup file exists for safe restore.
    pub has_backup: bool,
    /// Resolved settings file path (`settings.json` or legacy `claude.json`).
    pub settings_path: PathBuf,
    /// Why the helper-owned restore cannot proceed safely, when reconciliation is required.
    pub recovery_reason: Option<String>,
}

pub fn claude_switch_status() -> Result<ClaudeSwitchStatus> {
    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path(&settings_path);
    let state_path = claude_switch_state_path(&backup_path);
    let current = ClaudeSettingsSnapshot::read(&settings_path)?;
    let base_url = claude_base_url(&current);
    let backup = read_managed_file_snapshot(&backup_path, CLAUDE_SETTINGS_MAX_BYTES)
        .with_context(|| format!("read managed Claude switch file {:?}", backup_path))?;
    let state = read_managed_file_snapshot(&state_path, CLAUDE_SETTINGS_MAX_BYTES)
        .with_context(|| format!("read managed Claude switch file {:?}", state_path))?;
    let has_backup = backup.bytes().is_some();
    let (enabled, recovery_reason) = if has_backup {
        let validation =
            managed_snapshot_required_text(&backup, &backup_path).and_then(|backup_text| {
                let state = load_claude_switch_state_from_snapshot(&state, &state_path)?;
                validate_claude_switch_files(&current, &backup_text, state.as_ref())
            });
        match validation {
            Ok((
                ClaudeSwitchFilePhase::Applied | ClaudeSwitchFilePhase::PreviousAppliedPrepared,
                _,
            )) => (true, None),
            Ok((ClaudeSwitchFilePhase::OriginalPrepared, _)) => (false, None),
            Err(error) => (false, Some(error.to_string())),
        }
    } else if state.bytes().is_some() {
        (
            false,
            Some(
                "Claude switch state exists without its backup; refusing automatic recovery"
                    .to_string(),
            ),
        )
    } else {
        let enabled = base_url
            .as_deref()
            .is_some_and(|url| url.contains("127.0.0.1") || url.contains("localhost"));
        (enabled, None)
    };

    Ok(ClaudeSwitchStatus {
        enabled,
        base_url,
        has_backup,
        settings_path,
        recovery_reason,
    })
}

pub fn claude_switch_on(port: u16) -> Result<()> {
    claude_switch_on_base_url(&format!("http://127.0.0.1:{port}"))
}

pub fn claude_switch_on_base_url(base_url: &str) -> Result<()> {
    claude_switch_on_base_url_with_auto_restore(base_url, None).map(|_| ())
}

pub fn acquire_ephemeral_local_claude(port: u16) -> Result<ClaudeSwitchRestoreLease> {
    let auto_restore_generation = uuid::Uuid::new_v4().to_string();
    claude_switch_on_base_url_with_auto_restore(
        &format!("http://127.0.0.1:{port}"),
        Some(auto_restore_generation),
    )?
    .ok_or_else(|| anyhow!("ephemeral Claude switch did not produce a restore lease"))
}

fn claude_switch_on_base_url_with_auto_restore(
    base_url: &str,
    auto_restore_generation: Option<String>,
) -> Result<Option<ClaudeSwitchRestoreLease>> {
    claude_switch_on_base_url_with_auto_restore_after_settings_apply(
        base_url,
        auto_restore_generation,
        || {},
    )
}

fn claude_switch_on_base_url_with_auto_restore_after_settings_apply<F>(
    base_url: &str,
    auto_restore_generation: Option<String>,
    after_settings_apply: F,
) -> Result<Option<ClaudeSwitchRestoreLease>>
where
    F: FnOnce(),
{
    let base_url = crate::control_plane_client::normalize_base_url(base_url)
        .ok_or_else(|| anyhow!("Claude proxy base URL must start with http:// or https://"))?;
    if let Some(auto_restore_generation) = auto_restore_generation.as_deref() {
        uuid::Uuid::parse_str(auto_restore_generation)
            .context("Claude auto-restore generation must be a UUID")?;
    }
    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path(&settings_path);
    let state_path = claude_switch_state_path(&backup_path);
    let mut files = ClaudeSwitchTransactions::begin(&settings_path, &backup_path, &state_path)?;
    let current = ClaudeSettingsSnapshot::from_managed(files.settings.current(), &settings_path)?;

    if files.backup.current().bytes().is_none() {
        if files.state.current().bytes().is_some() {
            return Err(anyhow!(
                "Claude switch recovery required: state {:?} exists without backup {:?}; refusing to replace either file",
                state_path,
                backup_path
            ));
        }
        // Validate the source before committing a backup so malformed user files do not create
        // a misleading prepared operation.
        let _ = render_claude_patch(&current, &base_url)?;
        files
            .backup
            .replace_private(current.backup_text().as_bytes())
            .with_context(|| format!("prepare Claude settings backup {:?}", backup_path))?;
    }

    let backup_text = managed_snapshot_required_text(files.backup.current(), &backup_path)?;
    let previous_state =
        load_claude_switch_state_from_snapshot(files.state.current(), &state_path)?;
    let (phase, original) =
        validate_claude_switch_files(&current, &backup_text, previous_state.as_ref()).map_err(
            |error| claude_recovery_required("validating Claude recovery material", error),
        )?;
    let applied = render_claude_patch(&original, &base_url)?;
    let prepared_from = match phase {
        ClaudeSwitchFilePhase::OriginalPrepared => None,
        ClaudeSwitchFilePhase::Applied | ClaudeSwitchFilePhase::PreviousAppliedPrepared => {
            let prepared_from = prepared_from_current_patch(&original, &current)?;
            (prepared_from.applied_fingerprint != applied.fingerprint()).then_some(prepared_from)
        }
    };
    let state = ClaudeSwitchState {
        schema_version: CLAUDE_SWITCH_STATE_SCHEMA_VERSION,
        backup_fingerprint: file_fingerprint(&backup_text),
        original_absent: matches!(&original, ClaudeSettingsSnapshot::Absent),
        original_fingerprint: original.fingerprint(),
        applied_fingerprint: applied.fingerprint(),
        base_url,
        prepared_from,
        auto_restore_generation: auto_restore_generation.clone(),
    };
    let state_text = serde_json::to_string_pretty(&state)?;
    files
        .state
        .replace_private(state_text.as_bytes())
        .with_context(|| format!("prepare Claude switch state {:?}", state_path))?;

    ensure_claude_switch_files_current(&files, "preparing a Claude settings patch")?;
    let ClaudeSettingsSnapshot::Present(applied_text) = applied else {
        unreachable!("the Claude helper patch always creates a settings file");
    };
    files
        .settings
        .replace(applied_text.as_bytes())
        .with_context(|| format!("apply Claude settings patch to {:?}", settings_path))?;
    after_settings_apply();
    if let Err(error) = ensure_claude_recovery_material_current(
        &files,
        "verifying recovery material after applying a Claude settings patch",
    ) {
        const CONTEXT: &str = "Claude switch recovery required: recovery material changed after the settings patch was applied; restored the original settings snapshot";
        restore_claude_settings_snapshot(&mut files.settings, &settings_path, &original)
            .context(CONTEXT)?;
        return Err(error.context(CONTEXT));
    }
    eprintln!(
        "[EXPERIMENTAL] Updated {:?} to use Claude proxy via codex-helper",
        settings_path
    );
    Ok(
        auto_restore_generation.map(|auto_restore_generation| ClaudeSwitchRestoreLease {
            settings_path,
            backup_path,
            state_path,
            backup_fingerprint: state.backup_fingerprint,
            applied_fingerprint: state.applied_fingerprint,
            auto_restore_generation,
        }),
    )
}

pub fn claude_switch_off() -> Result<()> {
    claude_switch_off_after_validation(|| {})
}

fn claude_switch_off_after_validation<F>(after_validation: F) -> Result<()>
where
    F: FnOnce(),
{
    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path(&settings_path);
    let state_path = claude_switch_state_path(&backup_path);
    let mut files = ClaudeSwitchTransactions::begin(&settings_path, &backup_path, &state_path)?;
    if files.backup.current().bytes().is_none() {
        if files.state.current().bytes().is_some() {
            return Err(anyhow!(
                "Claude switch recovery required: state {:?} exists without backup {:?}; refusing automatic recovery",
                state_path,
                backup_path
            ));
        }
        return Ok(());
    }

    let current = ClaudeSettingsSnapshot::from_managed(files.settings.current(), &settings_path)?;
    let backup_text = managed_snapshot_required_text(files.backup.current(), &backup_path)?;
    let state = load_claude_switch_state_from_snapshot(files.state.current(), &state_path)?;
    let (phase, original) = validate_claude_switch_files(&current, &backup_text, state.as_ref())
        .map_err(|error| claude_recovery_required("validating Claude recovery material", error))?;
    after_validation();
    restore_and_cleanup_claude_switch(
        &mut files,
        &settings_path,
        &backup_path,
        &state_path,
        &current,
        phase,
        &original,
    )?;
    if phase != ClaudeSwitchFilePhase::OriginalPrepared {
        eprintln!(
            "[EXPERIMENTAL] Restored Claude settings from backup {:?}",
            backup_path
        );
    }
    Ok(())
}

pub fn restore_claude_switch_if_owned(lease: &ClaudeSwitchRestoreLease) -> Result<bool> {
    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path(&settings_path);
    let state_path = claude_switch_state_path(&backup_path);
    if settings_path != lease.settings_path
        || backup_path != lease.backup_path
        || state_path != lease.state_path
    {
        return Ok(false);
    }

    let mut files = ClaudeSwitchTransactions::begin(&settings_path, &backup_path, &state_path)?;
    let current = ClaudeSettingsSnapshot::from_managed(files.settings.current(), &settings_path)?;
    let Some(_) = files.backup.current().bytes() else {
        if files.state.current().bytes().is_some() {
            return Err(anyhow!(
                "Claude switch recovery required: state {:?} exists without backup {:?}; retained recovery material",
                state_path,
                backup_path
            ));
        }
        return Ok(false);
    };
    let Some(_) = files.state.current().bytes() else {
        return Err(anyhow!(
            "Claude switch recovery required: backup {:?} exists without state {:?}; retained recovery material",
            backup_path,
            state_path
        ));
    };
    let backup_text = managed_snapshot_required_text(files.backup.current(), &backup_path)?;
    let state_text = managed_snapshot_required_text(files.state.current(), &state_path)?;
    let state = parse_claude_switch_state(&state_text, &state_path)
        .map_err(|error| claude_recovery_required("parsing Claude recovery state", error))?;
    if state.backup_fingerprint != lease.backup_fingerprint
        || state.applied_fingerprint != lease.applied_fingerprint
        || state.auto_restore_generation.as_deref() != Some(lease.auto_restore_generation.as_str())
    {
        return Ok(false);
    }
    let (phase, original) = validate_claude_switch_files(&current, &backup_text, Some(&state))
        .map_err(|error| claude_recovery_required("validating Claude recovery material", error))?;
    if phase != ClaudeSwitchFilePhase::Applied {
        return Ok(false);
    }
    restore_and_cleanup_claude_switch(
        &mut files,
        &settings_path,
        &backup_path,
        &state_path,
        &current,
        phase,
        &original,
    )?;
    Ok(true)
}

pub fn restore_claude_switch_if_owned_with_retry(lease: &ClaudeSwitchRestoreLease) -> Result<bool> {
    const RETRY_DELAYS_MS: [u64; 6] = [0, 20, 50, 100, 200, 400];
    let mut last_error = None;
    for delay_ms in RETRY_DELAYS_MS {
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        match restore_claude_switch_if_owned(lease) {
            Ok(restored) => return Ok(restored),
            Err(error) if restore_retryable(&error) => last_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_error.expect("Claude restore retry schedule is non-empty"))
}

/// Warn before replacing an existing local Claude proxy patch.
pub fn guard_claude_settings_before_switch_on_interactive() -> Result<()> {
    use std::io::{self, Write};

    let status = claude_switch_status()?;
    if let Some(reason) = status.recovery_reason.as_deref() {
        return Err(anyhow!("Claude switch recovery required: {reason}"));
    }
    let Some(base_url) = status.base_url.as_deref().filter(|_| status.enabled) else {
        return Ok(());
    };
    let backup_path = claude_settings_backup_path(&status.settings_path);
    if !status.has_backup {
        eprintln!(
            "Warning: Claude settings {:?} points ANTHROPIC_BASE_URL to a local address ({base_url}), but no backup file {:?} was found; inspect this config manually if this is unexpected.",
            status.settings_path, backup_path
        );
        return Ok(());
    }

    if !atty::is(atty::Stream::Stdin) || !atty::is(atty::Stream::Stdout) {
        eprintln!(
            "Notice: Claude settings {:?} already points to the local proxy ({base_url}), and backup {:?} exists; run `codex-helper switch off --claude` to restore the original config.",
            status.settings_path, backup_path
        );
        return Ok(());
    }

    eprintln!(
        "Claude settings {:?} already points ANTHROPIC_BASE_URL to the local proxy ({base_url}), and backup {:?} exists.\nRestore the original Claude settings now? [Y/n] ",
        status.settings_path, backup_path
    );
    eprint!("> ");
    io::stdout().flush().ok();

    let mut input = String::new();
    if let Err(error) = io::stdin().read_line(&mut input) {
        eprintln!("Failed to read input: {error}");
        return Ok(());
    }
    let answer = input.trim();
    let confirmed =
        answer.is_empty() || answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");
    if confirmed {
        if let Err(error) = claude_switch_off() {
            eprintln!("Failed to restore Claude settings: {error}");
        } else {
            eprintln!("Restored Claude settings from backup.");
        }
    } else {
        eprintln!("Keeping the current Claude settings unchanged.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct ScopedEnv {
        saved: Vec<(String, Option<String>)>,
    }

    impl ScopedEnv {
        fn new() -> Self {
            Self { saved: Vec::new() }
        }

        unsafe fn set_path(&mut self, key: &str, value: &Path) {
            self.saved.push((key.to_string(), std::env::var(key).ok()));
            unsafe { std::env::set_var(key, value) };
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            for (key, value) in self.saved.drain(..).rev() {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    fn prepare_replacement_state_before_settings_write(
        settings_path: &Path,
        backup_path: &Path,
        state_path: &Path,
        next_base_url: &str,
    ) {
        let current = ClaudeSettingsSnapshot::read(settings_path).expect("read current patch");
        let backup_text = read_required_text(backup_path).expect("read backup");
        let previous_state = load_claude_switch_state(state_path).expect("read existing state");
        let (phase, original) =
            validate_claude_switch_files(&current, &backup_text, previous_state.as_ref())
                .expect("validate current helper patch");
        assert!(matches!(
            phase,
            ClaudeSwitchFilePhase::Applied | ClaudeSwitchFilePhase::PreviousAppliedPrepared
        ));

        let next_base_url = crate::control_plane_client::normalize_base_url(next_base_url)
            .expect("normalize next base URL");
        let applied = render_claude_patch(&original, &next_base_url).expect("render next patch");
        let prepared_from =
            prepared_from_current_patch(&original, &current).expect("record current helper patch");
        assert_ne!(prepared_from.applied_fingerprint, applied.fingerprint());
        let state = ClaudeSwitchState {
            schema_version: CLAUDE_SWITCH_STATE_SCHEMA_VERSION,
            backup_fingerprint: file_fingerprint(&backup_text),
            original_absent: matches!(&original, ClaudeSettingsSnapshot::Absent),
            original_fingerprint: original.fingerprint(),
            applied_fingerprint: applied.fingerprint(),
            base_url: next_base_url,
            prepared_from: Some(prepared_from),
            auto_restore_generation: None,
        };
        atomic_write(
            state_path,
            &serde_json::to_string_pretty(&state).expect("serialize replacement state"),
        )
        .expect("simulate state write before settings replacement");
    }

    #[test]
    fn claude_switch_off_refreshes_the_next_backup_snapshot() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-switch-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let original = r#"{
  "env": {
    "ANTHROPIC_BASE_URL": "https://api.anthropic.com/v1"
  }
}"#;
        let updated = r#"{
  "env": {
    "ANTHROPIC_BASE_URL": "https://proxy.example/v1"
  }
}"#;
        fs::write(&settings_path, original).expect("write original settings");

        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        claude_switch_on(3211).expect("first switch on");
        claude_switch_off().expect("first switch off");
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);
        assert!(!backup_path.exists());

        fs::write(&settings_path, updated).expect("write updated settings");
        claude_switch_on(3211).expect("second switch on");
        assert_eq!(fs::read_to_string(&backup_path).unwrap(), updated);
        claude_switch_off().expect("second switch off");
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), updated);
        assert!(!backup_path.exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_switch_off_recovers_a_prepared_replacement_state() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-prepared-off-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let state_path = claude_switch_state_path(&backup_path);
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        let first_base_url = "http://127.0.0.1:3210";
        let next_base_url = "http://127.0.0.1:4210";
        fs::write(&settings_path, original).expect("write original settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        claude_switch_on_base_url(first_base_url).expect("apply initial helper patch");
        prepare_replacement_state_before_settings_write(
            &settings_path,
            &backup_path,
            &state_path,
            next_base_url,
        );

        let status = claude_switch_status().expect("inspect prepared replacement state");
        assert!(status.enabled);
        assert_eq!(status.base_url.as_deref(), Some(first_base_url));
        assert!(status.recovery_reason.is_none());

        claude_switch_off().expect("restore the prepared predecessor patch");
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);
        assert!(!backup_path.exists());
        assert!(!state_path.exists());

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_switch_on_resumes_a_prepared_replacement_state() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-prepared-on-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let state_path = claude_switch_state_path(&backup_path);
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        let first_base_url = "http://127.0.0.1:3210";
        let next_base_url = "http://127.0.0.1:4210";
        fs::write(&settings_path, original).expect("write original settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        claude_switch_on_base_url(first_base_url).expect("apply initial helper patch");
        prepare_replacement_state_before_settings_write(
            &settings_path,
            &backup_path,
            &state_path,
            next_base_url,
        );

        claude_switch_on_base_url(next_base_url).expect("resume the prepared replacement");
        let status = claude_switch_status().expect("inspect resumed replacement state");
        assert!(status.enabled);
        assert_eq!(status.base_url.as_deref(), Some(next_base_url));
        assert!(status.recovery_reason.is_none());

        claude_switch_off().expect("restore original settings after resume");
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);
        assert!(!backup_path.exists());
        assert!(!state_path.exists());

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ephemeral_claude_restore_only_owns_its_generation() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-ephemeral-generation-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        fs::write(&settings_path, original).expect("write original settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        let first = acquire_ephemeral_local_claude(3210).expect("acquire first restore lease");
        let second = acquire_ephemeral_local_claude(4210).expect("acquire second restore lease");

        assert!(
            !restore_claude_switch_if_owned(&first).expect("check stale restore lease"),
            "the old foreground session must not restore the newer helper patch"
        );
        let active = claude_switch_status().expect("inspect newer helper patch");
        assert!(active.enabled);
        assert_eq!(active.base_url.as_deref(), Some("http://127.0.0.1:4210"));

        assert!(
            restore_claude_switch_if_owned(&second).expect("restore current generation"),
            "the active foreground session should restore its own generation"
        );
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ephemeral_claude_restore_retries_a_transient_managed_file_lock() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-ephemeral-retry-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        fs::write(&settings_path, original).expect("write original settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        let lease = acquire_ephemeral_local_claude(3210).expect("acquire foreground lease");
        let settings_lock =
            ManagedFileTransaction::begin(&settings_path, CLAUDE_SETTINGS_MAX_BYTES)
                .expect("hold a transient settings transaction lock");
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let restore = std::thread::spawn(move || {
            let _ = result_tx.send(restore_claude_switch_if_owned_with_retry(&lease));
        });

        std::thread::sleep(Duration::from_millis(80));
        drop(settings_lock);
        let restored = result_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("restore retry must finish after the transient lock is released")
            .expect("restore retry result");
        restore.join().expect("join restore retry thread");
        assert!(restored);
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn persistent_claude_switch_supersedes_an_ephemeral_restore_lease() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-persistent-supersedes-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        fs::write(&settings_path, original).expect("write original settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        let lease = acquire_ephemeral_local_claude(3210).expect("acquire foreground lease");
        claude_switch_on(4210).expect("apply persistent replacement switch");

        assert!(
            !restore_claude_switch_if_owned(&lease).expect("check superseded foreground lease"),
            "an explicit switch must survive foreground process cleanup"
        );
        let active = claude_switch_status().expect("inspect persistent helper patch");
        assert!(active.enabled);
        assert_eq!(active.base_url.as_deref(), Some("http://127.0.0.1:4210"));

        claude_switch_off().expect("restore persistent switch");
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_switch_compensates_when_recovery_material_disappears_after_apply() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-post-apply-recovery-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let state_path = claude_switch_state_path(&backup_path);
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        fs::write(&settings_path, original).expect("write original settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        let error = claude_switch_on_base_url_with_auto_restore_after_settings_apply(
            "http://127.0.0.1:3210",
            None,
            || fs::remove_file(&backup_path).expect("simulate a concurrent legacy backup removal"),
        )
        .expect_err("missing recovery material after apply must compensate the settings write");
        assert!(
            error
                .to_string()
                .contains("recovery material changed after the settings patch was applied"),
            "error must disclose the compensated recovery conflict: {error:#}"
        );
        assert_eq!(
            fs::read_to_string(&settings_path).expect("read compensated settings"),
            original,
            "the helper must restore the captured original settings instead of leaving an unrecoverable patch"
        );
        assert!(!backup_path.exists());
        assert!(
            state_path.exists(),
            "retain conflicting state for explicit recovery"
        );
        let status = claude_switch_status().expect("inspect compensated recovery state");
        assert!(!status.enabled);
        assert!(status.recovery_reason.is_some());

        fs::remove_file(&state_path).expect("remove test-only stale recovery state");
        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn claude_switch_writes_private_recovery_material() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-private-recovery-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let state_path = claude_switch_state_path(&backup_path);
        fs::write(&settings_path, r#"{"env":{"ANTHROPIC_API_KEY":"secret"}}"#)
            .expect("write sensitive Claude settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        claude_switch_on(3210).expect("create private recovery material");
        for path in [&backup_path, &state_path] {
            assert_eq!(
                fs::metadata(path)
                    .expect("read recovery metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "{} must be private before helper recovery material is published",
                path.display()
            );
        }

        claude_switch_off().expect("restore private recovery material");
        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn claude_switch_rejects_linked_recovery_material() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-linked-recovery-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let state_path = claude_switch_state_path(&backup_path);
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        fs::write(&settings_path, original).expect("write original settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        for (name, recovery_path) in [("backup", &backup_path), ("state", &state_path)] {
            claude_switch_on(3210).expect("create Claude recovery material");
            let linked_path = claude_home.join(format!("{name}-hard-link"));
            fs::hard_link(recovery_path, &linked_path)
                .expect("create a second link to recovery material");

            let error = claude_switch_off()
                .expect_err("linked recovery material must fail closed instead of restoring");
            let detail = format!("{error:#}");
            assert!(
                detail.contains("hard links"),
                "{name} error must explain the rejected topology: {detail}"
            );
            assert_ne!(
                fs::read_to_string(&settings_path).expect("read still-patched settings"),
                original,
                "a rejected recovery path must leave the active helper patch untouched"
            );

            fs::remove_file(&linked_path).expect("remove test hard link");
            claude_switch_off().expect("restore after the recovery topology is safe again");
            assert_eq!(
                fs::read_to_string(&settings_path).expect("read restored settings"),
                original
            );
        }

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_switch_off_restores_the_absent_file_sentinel() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-absent-switch-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let state_path = claude_switch_state_path(&backup_path);
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        claude_switch_on(3210).expect("switch on without pre-existing settings");
        assert!(settings_path.exists());
        assert_eq!(
            fs::read_to_string(&backup_path).expect("read absent sentinel"),
            CLAUDE_ABSENT_BACKUP_SENTINEL
        );
        assert!(state_path.exists());

        claude_switch_off().expect("restore absent settings state");
        assert!(!settings_path.exists());
        assert!(!backup_path.exists());
        assert!(!state_path.exists());

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_switch_off_fails_closed_after_external_settings_edit() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-external-edit-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let state_path = claude_switch_state_path(&backup_path);
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        let external = r#"{"permissions":{"allow":["Read","Write"]}}"#;
        fs::write(&settings_path, original).expect("write original settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        claude_switch_on(3210).expect("switch on");
        fs::write(&settings_path, external).expect("simulate external settings edit");

        let error = claude_switch_off().expect_err("external edit must block restore");
        assert!(
            error
                .to_string()
                .contains("refusing to overwrite external edits")
        );
        assert_eq!(
            fs::read_to_string(&settings_path).expect("read external settings"),
            external
        );
        assert!(backup_path.exists(), "recovery backup must be preserved");
        assert!(state_path.exists(), "recovery state must be preserved");
        let status = claude_switch_status().expect("inspect conflicted switch");
        assert!(!status.enabled);
        assert!(status.recovery_reason.is_some());

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_switch_off_preserves_recovery_material_changed_after_validation() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-recovery-bundle-race-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let state_path = claude_switch_state_path(&backup_path);
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        let external_backup = r#"{"permissions":{"allow":["External"]}}"#;
        fs::write(&settings_path, original).expect("write original settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        claude_switch_on(3210).expect("apply helper patch");
        let applied = fs::read_to_string(&settings_path).expect("capture helper patch");
        let recorded_state = fs::read_to_string(&state_path).expect("capture recovery state");

        let error = claude_switch_off_after_validation(|| {
            fs::write(&backup_path, external_backup)
                .expect("simulate an external recovery backup edit after validation");
        })
        .expect_err("changed recovery material must block restore and cleanup");

        assert!(
            error
                .to_string()
                .contains("Claude switch recovery required"),
            "the caller must be directed to explicit recovery: {error:#}"
        );
        assert_eq!(
            fs::read_to_string(&settings_path).expect("read still-patched settings"),
            applied,
            "a changed recovery bundle must block the settings restore"
        );
        assert_eq!(
            fs::read_to_string(&backup_path).expect("read external backup"),
            external_backup,
            "the external backup edit must not be overwritten or deleted"
        );
        assert_eq!(
            fs::read_to_string(&state_path).expect("read retained recovery state"),
            recorded_state,
            "the paired state must remain available for reconciliation"
        );
        let status = claude_switch_status().expect("inspect conflicted recovery material");
        assert!(!status.enabled);
        assert!(status.recovery_reason.is_some());

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_switch_off_preserves_state_changed_after_validation() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-recovery-state-race-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let state_path = claude_switch_state_path(&backup_path);
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        let external_state = r#"{"external":true}"#;
        fs::write(&settings_path, original).expect("write original settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        claude_switch_on(3210).expect("apply helper patch");
        let applied = fs::read_to_string(&settings_path).expect("capture helper patch");

        let error = claude_switch_off_after_validation(|| {
            fs::write(&state_path, external_state)
                .expect("simulate an external recovery state edit after validation");
        })
        .expect_err("changed recovery state must block restore and cleanup");

        assert!(
            error
                .to_string()
                .contains("Claude switch recovery required"),
            "the caller must be directed to explicit recovery: {error:#}"
        );
        assert_eq!(
            fs::read_to_string(&settings_path).expect("read still-patched settings"),
            applied,
            "a changed recovery state must block the settings restore"
        );
        assert_eq!(
            fs::read_to_string(&backup_path).expect("read retained backup"),
            original,
            "the valid backup must remain paired with the changed state"
        );
        assert_eq!(
            fs::read_to_string(&state_path).expect("read external state"),
            external_state,
            "the external state edit must not be overwritten or deleted"
        );
        let status = claude_switch_status().expect("inspect conflicted recovery state");
        assert!(!status.enabled);
        assert!(status.recovery_reason.is_some());

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_claude_backup_is_reused_only_for_the_expected_helper_patch() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-legacy-backup-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let state_path = claude_switch_state_path(&backup_path);
        let original =
            ClaudeSettingsSnapshot::Present(r#"{"permissions":{"allow":["Read"]}}"#.to_string());
        let old_patch = render_claude_patch(&original, "http://127.0.0.1:3210")
            .expect("render legacy helper patch");
        let ClaudeSettingsSnapshot::Present(old_patch) = old_patch else {
            unreachable!();
        };
        fs::write(&backup_path, original.backup_text()).expect("write legacy raw backup");
        fs::write(&settings_path, old_patch).expect("write legacy helper patch");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        claude_switch_on(4210).expect("upgrade the verified legacy helper patch");
        assert!(state_path.exists(), "legacy state should be upgraded");
        assert_eq!(
            claude_switch_status()
                .expect("inspect upgraded switch")
                .base_url
                .as_deref(),
            Some("http://127.0.0.1:4210")
        );
        claude_switch_off().expect("restore legacy original");
        assert_eq!(
            fs::read_to_string(&settings_path).expect("read restored settings"),
            original.backup_text()
        );

        drop(env);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_claude_backup_conflict_is_preserved_for_manual_recovery() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-claude-legacy-conflict-test-{}",
            uuid::Uuid::new_v4()
        ));
        let claude_home = root.join("claude");
        fs::create_dir_all(&claude_home).expect("create Claude home");
        let settings_path = claude_home.join("settings.json");
        let backup_path = claude_home.join("settings.json.codex-helper-backup");
        let original = r#"{"permissions":{"allow":["Read"]}}"#;
        let external = r#"{"permissions":{"allow":["Write"]}}"#;
        fs::write(&backup_path, original).expect("write legacy raw backup");
        fs::write(&settings_path, external).expect("write unrelated current settings");
        let mut env = ScopedEnv::new();
        unsafe { env.set_path("CLAUDE_HOME", &claude_home) };

        let on_error = claude_switch_on(3210).expect_err("conflicted backup must block switch on");
        assert!(
            on_error
                .to_string()
                .contains("refusing to overwrite external edits")
        );
        let off_error = claude_switch_off().expect_err("conflicted backup must block switch off");
        assert!(
            off_error
                .to_string()
                .contains("refusing to overwrite external edits")
        );
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), external);
        assert_eq!(fs::read_to_string(&backup_path).unwrap(), original);

        drop(env);
        let _ = fs::remove_dir_all(root);
    }
}
