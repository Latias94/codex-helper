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
const COMPATIBLE_PROVIDER_NAME: &str = "codex-helper";
const OPENAI_PROVIDER_NAME: &str = "OpenAI";
// Current Codex treats a non-empty actor header as a client capability gate. The proxy consumes
// this exact non-secret marker locally before applying upstream authentication policy.
pub const CODEX_CLIENT_FACADE_ACTOR_HEADER: &str = "x-openai-actor-authorization";
pub const CODEX_CLIENT_FACADE_ACTOR_VALUE: &str = "codex-helper-client-facade-v1";
const STATE_FILE_NAME: &str = "codex-switch.json";
const LOCK_FILE_NAME: &str = "codex-switch.lock";
const LEGACY_STATE_FILE_NAME: &str = "codex-helper-switch-state.json";
const SWITCH_TEMP_FILE_PREFIX: &str = ".codex-switch-v1-";
const LEGACY_SWITCH_TEMP_FILE_PREFIX: &str = ".codex-switch-";
const SWITCH_TEMP_FILE_SUFFIX: &str = ".tmp";
const SWITCH_DELETE_TOMBSTONE_PREFIX: &str = ".codex-switch-delete-v1-";
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodexClientFacade {
    #[default]
    Compatible,
    OpenAi,
    OpenAiTools,
}

impl CodexClientFacade {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compatible => "compatible",
            Self::OpenAi => "openai",
            Self::OpenAiTools => "openai-tools",
        }
    }

    const fn provider_name(self) -> &'static str {
        match self {
            Self::Compatible => COMPATIBLE_PROVIDER_NAME,
            Self::OpenAi | Self::OpenAiTools => OPENAI_PROVIDER_NAME,
        }
    }

    const fn exposes_openai_tools(self) -> bool {
        matches!(self, Self::OpenAiTools)
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
    pub client_facade: Option<CodexClientFacade>,
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
    #[error(
        "Codex helper is already applied with client facade {current}; run explicit switch off before switching to {requested}"
    )]
    AlreadyAppliedWithDifferentFacade { current: String, requested: String },
    #[error("Codex switch recovery is required: {reason}")]
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
    #[serde(default)]
    client_facade: CodexClientFacade,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_reason: Option<String>,
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

#[derive(Clone, Copy)]
enum FileCommitExpectation<'a> {
    Journal(ConfigCommitExpectation<'a>),
    LegacySnapshot {
        expected: &'a ConfigSnapshot,
        legacy_path: &'a Path,
    },
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
            Self::AfterLegacyConfigRestore => "after_legacy_config_restore",
            Self::AfterLegacyAuthRestore => "after_legacy_auth_restore",
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
    apply_with_client_facade(intent, CodexClientFacade::Compatible)
}

pub fn apply_with_client_facade(
    intent: CodexSwitchIntent,
    client_facade: CodexClientFacade,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    apply_with_client_facade_and_failpoint(intent, client_facade, ApplyFailpoint::None)
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

#[cfg(test)]
fn apply_with_failpoint(
    intent: CodexSwitchIntent,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    apply_with_client_facade_and_failpoint(intent, CodexClientFacade::Compatible, failpoint)
}

fn apply_with_client_facade_and_failpoint(
    intent: CodexSwitchIntent,
    client_facade: CodexClientFacade,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    let paths = SwitchPaths::resolve()?;
    let _lock = OperationLock::acquire(paths.lock.as_path())?;
    cleanup_managed_switch_artifacts(&paths)?;
    let current_state_present =
        switch_path_entry_present(paths.state.as_path(), "inspect current switch state at")?;
    if legacy_state_present(paths.legacy_state.as_path())? {
        if current_state_present {
            return Err(CodexSwitchError::LegacySwitchStateConflict {
                legacy_path: paths.legacy_state.clone(),
                current_path: paths.state.clone(),
            });
        }
        recover_legacy_switch_state(&paths, failpoint)?;
        let current = read_config_snapshot(paths.config.as_path())?;
        return match intent {
            CodexSwitchIntent::On { validated_base_url } => begin_on(
                &paths,
                current,
                validated_base_url,
                client_facade,
                failpoint,
            ),
            CodexSwitchIntent::Off => outcome(&paths, CodexSwitchChange::Recovered),
        };
    }
    let journal = read_journal(paths.state.as_path())?;
    if let Some(journal) = journal.as_ref() {
        ensure_journal_config_matches(&paths, journal)?;
    }
    let current = read_config_snapshot(paths.config.as_path())?;

    match intent {
        CodexSwitchIntent::On { validated_base_url } => apply_on(
            &paths,
            current,
            journal,
            validated_base_url,
            client_facade,
            failpoint,
        ),
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
        client_facade: None,
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
    client_facade: CodexClientFacade,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    match journal {
        None => begin_on(paths, current, target, client_facade, failpoint),
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
                ensure_switch_matches(&journal, &target, client_facade)?;
                outcome(paths, CodexSwitchChange::Unchanged)
            }
            JournalPhase::Prepared => match journal.operation {
                JournalOperation::On if current.matches_applied(&journal) => {
                    ensure_switch_matches(&journal, &target, client_facade)?;
                    journal.phase = JournalPhase::Applied;
                    write_current_journal(paths, &journal)?;
                    outcome(paths, CodexSwitchChange::Recovered)
                }
                JournalOperation::On if current.matches_original(&journal) => {
                    ensure_switch_matches(&journal, &target, client_facade)?;
                    resume_on(paths, journal, failpoint, None)
                }
                JournalOperation::Off if current.matches_original(&journal) => {
                    remove_current_journal(paths)?;
                    begin_on(paths, current, target, client_facade, failpoint)
                }
                JournalOperation::Off if current.matches_applied(&journal) => {
                    ensure_switch_matches(&journal, &target, client_facade)?;
                    journal.phase = JournalPhase::Applied;
                    journal.operation = JournalOperation::On;
                    write_current_journal(paths, &journal)?;
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

fn ensure_switch_matches(
    journal: &SwitchJournal,
    target: &ValidatedCodexBaseUrl,
    client_facade: CodexClientFacade,
) -> Result<(), CodexSwitchError> {
    ensure_target_matches(journal, target)?;
    if journal.client_facade == client_facade {
        return Ok(());
    }
    Err(CodexSwitchError::AlreadyAppliedWithDifferentFacade {
        current: journal.client_facade.as_str().to_string(),
        requested: client_facade.as_str().to_string(),
    })
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
    client_facade: CodexClientFacade,
    failpoint: ApplyFailpoint,
) -> Result<CodexSwitchOutcome, CodexSwitchError> {
    validate_config_topology(paths.config.as_path(), current.present)?;
    let original = inspect_config(paths.config.as_path(), &current.text)?;
    reject_unowned_helper_config(&original)?;

    let patch = patch_on(
        paths.config.as_path(),
        &current.text,
        target.as_str(),
        client_facade,
    )?;
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
        client_facade,
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
                journal.client_facade,
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
            client_facade: Some(journal.client_facade),
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
            client_facade: None,
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
            client_facade: Some(journal.client_facade),
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
        client_facade: Some(journal.client_facade),
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

fn patch_on(
    path: &Path,
    text: &str,
    base_url: &str,
    client_facade: CodexClientFacade,
) -> Result<OnPatch, CodexSwitchError> {
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
    helper.insert("name", editable_value(client_facade.provider_name()));
    helper.insert("base_url", editable_value(base_url));
    helper.insert("wire_api", editable_value("responses"));
    helper.insert("request_max_retries", editable_value(0));
    if client_facade.exposes_openai_tools() {
        let mut headers = Table::new();
        headers.insert(
            CODEX_CLIENT_FACADE_ACTOR_HEADER,
            editable_value(CODEX_CLIENT_FACADE_ACTOR_VALUE),
        );
        helper.insert("http_headers", Item::Table(headers));
    }
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
    let expectation = FileCommitExpectation::Journal(ConfigCommitExpectation {
        journal,
        state: expected,
    });
    match edit {
        ConfigEdit::Write(text) => atomic_write_text(
            path,
            text.as_str(),
            FilePermissions::PreserveOrSecure,
            Some(expectation),
        ),
        ConfigEdit::Remove => {
            verify_file_before_commit(path, expectation)?;
            remove_file_durable(path)
        }
    }
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
    match edit {
        ConfigEdit::Write(text) => atomic_write_text(
            path,
            text.as_str(),
            FilePermissions::PreserveOrSecure,
            Some(expectation),
        ),
        ConfigEdit::Remove => {
            verify_file_before_commit(path, expectation)?;
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
            verify_file_before_commit(path, expectation)?;
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
    move_file_write_through(source, destination, true)
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

#[cfg(any(windows, test))]
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
        let auth = config.with_file_name("auth.json");
        Ok(Self {
            config_fingerprint: config_path_fingerprint(config.as_path()),
            config,
            auth,
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
        let facades = [
            (CodexClientFacade::Compatible, "compatible"),
            (CodexClientFacade::OpenAi, "openai"),
            (CodexClientFacade::OpenAiTools, "openai-tools"),
        ];
        for (facade, expected) in facades {
            assert_eq!(facade.as_str(), expected);
        }

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

        fn auth_path(&self) -> PathBuf {
            self.codex_home.join("auth.json")
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
    fn explicit_client_facades_expose_only_the_requested_codex_capabilities() {
        for (facade, provider_name, exposes_tools) in [
            (CodexClientFacade::Compatible, "codex-helper", false),
            (CodexClientFacade::OpenAi, "OpenAI", false),
            (CodexClientFacade::OpenAiTools, "OpenAI", true),
        ] {
            let env = TestEnvironment::new();
            let original = "model_provider = \"openai\"\n";
            env.write_config(original);
            std::fs::write(env.auth_path(), b"auth sentinel").expect("write auth sentinel");

            let outcome = apply_with_client_facade(
                CodexSwitchIntent::On {
                    validated_base_url: ValidatedCodexBaseUrl::local(3211),
                },
                facade,
            )
            .expect("switch on with explicit client facade");

            assert_eq!(outcome.status.client_facade, Some(facade));
            let applied = env.read_config();
            assert!(applied.contains(format!("name = \"{provider_name}\"").as_str()));
            assert_eq!(
                applied.contains("[model_providers.codex_proxy.http_headers]"),
                exposes_tools
            );
            assert_eq!(
                applied.contains(CODEX_CLIENT_FACADE_ACTOR_HEADER),
                exposes_tools
            );
            assert_eq!(
                applied.contains(CODEX_CLIENT_FACADE_ACTOR_VALUE),
                exposes_tools
            );
            assert!(!applied.contains("requires_openai_auth"));
            assert_eq!(
                std::fs::read(env.auth_path()).expect("read auth sentinel"),
                b"auth sentinel"
            );

            apply(CodexSwitchIntent::Off).expect("switch off facade");
            assert_eq!(env.read_config(), original);
            assert_eq!(
                std::fs::read(env.auth_path()).expect("read restored auth sentinel"),
                b"auth sentinel"
            );
        }
    }

    #[test]
    fn changing_client_facade_requires_explicit_switch_off() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        apply_with_client_facade(on.clone(), CodexClientFacade::OpenAi)
            .expect("switch on OpenAI facade");
        let config_before = env.read_config();
        let state_before = std::fs::read(env.state_path()).expect("read state before mismatch");

        assert!(matches!(
            apply_with_client_facade(on.clone(), CodexClientFacade::OpenAiTools),
            Err(CodexSwitchError::AlreadyAppliedWithDifferentFacade { .. })
        ));
        assert_eq!(env.read_config(), config_before);
        assert_eq!(
            std::fs::read(env.state_path()).expect("read state after mismatch"),
            state_before
        );

        apply(CodexSwitchIntent::Off).expect("switch off old facade");
        apply_with_client_facade(on, CodexClientFacade::OpenAiTools)
            .expect("switch on new facade after off");
        assert!(env.read_config().contains(CODEX_CLIENT_FACADE_ACTOR_VALUE));
    }

    #[test]
    fn prepared_switch_recovers_with_its_recorded_client_facade() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };

        assert!(matches!(
            apply_with_client_facade_and_failpoint(
                on.clone(),
                CodexClientFacade::OpenAiTools,
                ApplyFailpoint::AfterPrepared,
            ),
            Err(CodexSwitchError::InjectedFailure("after_prepared"))
        ));
        let outcome = apply_with_client_facade(on, CodexClientFacade::OpenAiTools)
            .expect("resume prepared facade switch");

        assert_eq!(
            outcome.status.client_facade,
            Some(CodexClientFacade::OpenAiTools)
        );
        assert!(env.read_config().contains(CODEX_CLIENT_FACADE_ACTOR_VALUE));
    }

    #[test]
    fn journals_without_client_facade_default_to_compatible() {
        let env = TestEnvironment::new();
        env.write_config("model_provider = \"openai\"\n");
        let on = CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(3211),
        };
        apply(on.clone()).expect("switch on compatible facade");
        let mut state = serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(env.state_path()).expect("read current journal"),
        )
        .expect("parse current journal");
        state
            .as_object_mut()
            .expect("journal object")
            .remove("client_facade");
        std::fs::write(
            env.state_path(),
            serde_json::to_vec_pretty(&state).expect("serialize old journal"),
        )
        .expect("write old journal shape");

        let status = inspect().expect("inspect old journal shape");
        assert_eq!(status.client_facade, Some(CodexClientFacade::Compatible));
        assert_eq!(
            apply(on).expect("reuse old journal shape").change,
            CodexSwitchChange::Unchanged
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
