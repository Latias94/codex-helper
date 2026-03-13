use super::*;

#[derive(Debug, Default)]
pub struct ViewState {
    pub requested_page: Option<Page>,
    pub setup: SetupViewState,
    pub stations: StationsViewState,
    pub doctor: DoctorViewState,
    pub sessions: SessionsViewState,
    pub requests: RequestsViewState,
    pub config: ConfigViewState,
    pub history: HistoryViewState,
}

#[derive(Debug, Default)]
pub struct StationsViewState {
    pub search: String,
    pub enabled_only: bool,
    pub overrides_only: bool,
    pub selected_name: Option<String>,
    pub(super) retry_editor: StationsRetryEditorState,
}

#[derive(Debug, Default)]
pub(super) struct StationsRetryEditorState {
    pub(super) source_signature: Option<String>,
    pub(super) profile: String,
    pub(super) cloudflare_challenge_cooldown_secs: String,
    pub(super) cloudflare_timeout_cooldown_secs: String,
    pub(super) transport_cooldown_secs: String,
    pub(super) cooldown_backoff_factor: String,
    pub(super) cooldown_backoff_max_secs: String,
}

#[derive(Debug, Default)]
pub struct DoctorViewState {
    pub report: Option<crate::doctor::DoctorReport>,
    pub last_error: Option<String>,
    pub loaded_at_ms: Option<u64>,
}

#[derive(Debug)]
pub struct SetupViewState {
    pub import_codex_on_init: bool,
}

impl Default for SetupViewState {
    fn default() -> Self {
        Self {
            import_codex_on_init: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ConfigMode {
    #[default]
    Form,
    Raw,
}

#[derive(Debug)]
pub struct ConfigViewState {
    pub(super) mode: ConfigMode,
    pub(super) service: crate::config::ServiceKind,
    pub(super) selected_name: Option<String>,
    pub(super) station_editor: ConfigStationEditorState,
    pub(super) selected_provider_name: Option<String>,
    pub(super) provider_editor: ConfigProviderEditorState,
    pub(super) selected_profile_name: Option<String>,
    pub(super) new_profile_name: String,
    pub(super) profile_editor: ConfigProfileEditorState,
    pub(super) working: Option<ConfigWorkingDocument>,
    pub(super) load_error: Option<String>,
    pub(super) import_codex: ImportCodexModalState,
}

impl Default for ConfigViewState {
    fn default() -> Self {
        Self {
            mode: ConfigMode::Form,
            service: crate::config::ServiceKind::Codex,
            selected_name: None,
            station_editor: ConfigStationEditorState::default(),
            selected_provider_name: None,
            provider_editor: ConfigProviderEditorState::default(),
            selected_profile_name: None,
            new_profile_name: String::new(),
            profile_editor: ConfigProfileEditorState::default(),
            working: None,
            load_error: None,
            import_codex: ImportCodexModalState::default(),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct ConfigStationEditorState {
    pub(super) station_name: Option<String>,
    pub(super) alias: String,
    pub(super) enabled: bool,
    pub(super) level: u8,
    pub(super) members: Vec<ConfigStationMemberEditorState>,
    pub(super) new_station_name: String,
}

#[derive(Debug, Default, Clone)]
pub(super) struct ConfigStationMemberEditorState {
    pub(super) provider: String,
    pub(super) endpoint_names: String,
    pub(super) preferred: bool,
}

#[derive(Debug, Default)]
pub(super) struct ConfigProviderEditorState {
    pub(super) provider_name: Option<String>,
    pub(super) alias: String,
    pub(super) enabled: bool,
    pub(super) auth_token_env: String,
    pub(super) api_key_env: String,
    pub(super) endpoints: Vec<ConfigProviderEndpointEditorState>,
    pub(super) new_provider_name: String,
}

#[derive(Debug, Default, Clone)]
pub(super) struct ConfigProviderEndpointEditorState {
    pub(super) name: String,
    pub(super) base_url: String,
    pub(super) enabled: bool,
}

#[derive(Debug, Default)]
pub(super) struct ConfigProfileEditorState {
    pub(super) profile_name: Option<String>,
    pub(super) extends: Option<String>,
    pub(super) station: Option<String>,
    pub(super) model: String,
    pub(super) reasoning_effort: String,
    pub(super) service_tier: String,
}

#[derive(Debug, Clone)]
pub(super) enum ConfigWorkingDocument {
    Legacy(crate::config::ProxyConfig),
    V2(crate::config::ProxyConfigV2),
}

#[derive(Debug)]
pub(super) struct ImportCodexModalState {
    pub(super) open: bool,
    pub(super) add_missing: bool,
    pub(super) set_active: bool,
    pub(super) force: bool,
    pub(super) preview: Option<crate::config::SyncCodexAuthFromCodexReport>,
    pub(super) last_error: Option<String>,
}

impl Default for ImportCodexModalState {
    fn default() -> Self {
        Self {
            open: false,
            add_missing: true,
            set_active: true,
            force: false,
            preview: None,
            last_error: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct SessionsViewState {
    pub active_only: bool,
    pub errors_only: bool,
    pub overrides_only: bool,
    pub lock_order: bool,
    pub search: String,
    pub default_profile_selection: Option<String>,
    pub selected_session_id: Option<String>,
    pub selected_idx: usize,
    pub(super) ordered_session_ids: Vec<Option<String>>,
    pub(super) last_active_set: HashSet<Option<String>>,
    pub(super) editor: SessionOverrideEditor,
}

#[derive(Debug)]
pub struct RequestsViewState {
    pub errors_only: bool,
    pub scope_session: bool,
    pub focused_session_id: Option<String>,
    pub selected_idx: usize,
}

impl Default for RequestsViewState {
    fn default() -> Self {
        Self {
            errors_only: false,
            scope_session: true,
            focused_session_id: None,
            selected_idx: 0,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct SessionOverrideEditor {
    pub(super) sid: Option<String>,
    pub(super) profile_selection: Option<String>,
    pub(super) model_override: String,
    pub(super) config_override: Option<String>,
    pub(super) effort_override: Option<String>,
    pub(super) custom_effort: String,
    pub(super) service_tier_override: Option<String>,
    pub(super) custom_service_tier: String,
}
