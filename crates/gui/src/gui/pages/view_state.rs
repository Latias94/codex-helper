use super::*;

#[derive(Debug, Default)]
pub struct ViewState {
    pub requested_page: Option<Page>,
    pub setup: SetupViewState,
    pub stations: StationsViewState,
    pub doctor: DoctorViewState,
    pub stats: StatsViewState,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ControlTraceKindFilter {
    #[default]
    All,
    RequestCompleted,
    RetryTrace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ControlTraceSourceKind {
    #[default]
    LocalFile,
    AttachedApi,
    AttachedFallbackLocal,
}

#[derive(Debug, Clone, Default)]
pub struct ControlTraceRecordState {
    pub ts_ms: u64,
    pub kind: String,
    pub service: Option<String>,
    pub request_id: Option<u64>,
    pub event: Option<String>,
    pub summary: String,
}

#[derive(Debug)]
pub struct StatsViewState {
    pub control_trace_limit: usize,
    pub control_trace_loaded_limit: usize,
    pub control_trace_loaded_signature: Option<String>,
    pub control_trace_source_kind: ControlTraceSourceKind,
    pub control_trace_source_detail: Option<String>,
    pub control_trace_kind: ControlTraceKindFilter,
    pub control_trace_query: String,
    pub control_trace_entries: Vec<ControlTraceRecordState>,
    pub control_trace_last_loaded_ms: Option<u64>,
    pub control_trace_last_error: Option<String>,
}

impl Default for StatsViewState {
    fn default() -> Self {
        Self {
            control_trace_limit: 80,
            control_trace_loaded_limit: 0,
            control_trace_loaded_signature: None,
            control_trace_source_kind: ControlTraceSourceKind::LocalFile,
            control_trace_source_detail: None,
            control_trace_kind: ControlTraceKindFilter::All,
            control_trace_query: String::new(),
            control_trace_entries: Vec::new(),
            control_trace_last_loaded_ms: None,
            control_trace_last_error: None,
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ConfigV2Section {
    Stations,
    Providers,
    #[default]
    Profiles,
}

#[derive(Debug)]
pub struct ConfigViewState {
    pub(super) mode: ConfigMode,
    pub(super) v2_section: ConfigV2Section,
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
            v2_section: ConfigV2Section::default(),
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
