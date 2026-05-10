use super::*;

#[derive(Debug, Default)]
pub struct ViewState {
    pub requested_page: Option<Page>,
    pub(super) history_open_seq: u64,
    pub(super) history_open_load: Option<HistoryOpenLoad>,
    pub setup: SetupViewState,
    pub discovery: DiscoveryViewState,
    pub stations: StationsViewState,
    pub doctor: DoctorViewState,
    pub stats: StatsViewState,
    pub sessions: SessionsViewState,
    pub requests: RequestsViewState,
    pub proxy_settings: ProxySettingsViewState,
    pub history: HistoryViewState,
}

#[derive(Debug)]
pub(in crate::gui::pages) struct HistoryOpenLoad {
    pub(super) seq: u64,
    pub(super) origin: super::history_external::ExternalHistoryOrigin,
    pub(super) require_local: bool,
    pub(super) rx: std::sync::mpsc::Receiver<(u64, anyhow::Result<Option<SessionSummary>>)>,
    pub(super) join: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Default)]
pub struct DiscoveryViewState {
    pub recommended_only: bool,
    pub station_control_only: bool,
    pub session_control_only: bool,
    pub retry_write_only: bool,
    pub remote_admin_only: bool,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestLedgerSummaryFilterState {
    pub session: String,
    pub model: String,
    pub station: String,
    pub provider: String,
    pub status_min: String,
    pub status_max: String,
    pub fast_only: bool,
    pub retried_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestLedgerSummaryFilterParseError {
    StatusMin,
    StatusMax,
}

impl RequestLedgerSummaryFilterState {
    pub fn to_request_log_filters(
        &self,
    ) -> Result<crate::request_ledger::RequestLogFilters, RequestLedgerSummaryFilterParseError>
    {
        Ok(crate::request_ledger::RequestLogFilters {
            session: normalize_text_filter(&self.session),
            model: normalize_text_filter(&self.model),
            station: normalize_text_filter(&self.station),
            provider: normalize_text_filter(&self.provider),
            status_min: parse_status_bound(
                &self.status_min,
                RequestLedgerSummaryFilterParseError::StatusMin,
            )?,
            status_max: parse_status_bound(
                &self.status_max,
                RequestLedgerSummaryFilterParseError::StatusMax,
            )?,
            fast: self.fast_only,
            retried: self.retried_only,
        })
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Default)]
pub struct DoctorViewState {
    pub report: Option<crate::doctor::DoctorReport>,
    pub last_error: Option<String>,
    pub loaded_at_ms: Option<u64>,
    pub(super) load_seq: u64,
    pub(super) load: Option<DoctorLoad>,
}

#[derive(Debug)]
pub(super) struct DoctorLoad {
    pub(super) seq: u64,
    pub(super) rx: std::sync::mpsc::Receiver<(u64, anyhow::Result<crate::doctor::DoctorReport>)>,
    pub(super) join: tokio::task::JoinHandle<()>,
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
    pub trace_id: Option<String>,
    pub event: Option<String>,
    pub summary: String,
}

#[derive(Debug)]
pub(super) struct ControlTraceLoad {
    pub(super) seq: u64,
    pub(super) source_signature: Option<String>,
    pub(super) limit: usize,
    pub(super) rx: std::sync::mpsc::Receiver<(
        u64,
        anyhow::Result<crate::gui::proxy_control::ControlTraceReadResult>,
    )>,
    pub(super) join: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
pub(super) struct RequestLedgerSummaryLoad {
    pub(super) seq: u64,
    pub(super) source_signature: Option<String>,
    pub(super) group: crate::request_ledger::RequestUsageSummaryGroup,
    pub(super) limit: usize,
    pub(super) filters: crate::request_ledger::RequestLogFilters,
    pub(super) rx: std::sync::mpsc::Receiver<(
        u64,
        anyhow::Result<crate::gui::proxy_control::RequestLedgerSummaryReadResult>,
    )>,
    pub(super) join: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
pub struct StatsViewState {
    pub(super) pricing_editor: StatsPricingEditorState,
    pub control_trace_limit: usize,
    pub(super) control_trace_requested_signature: Option<String>,
    pub(super) control_trace_requested_limit: usize,
    pub(super) control_trace_load_seq: u64,
    pub(super) control_trace_load: Option<ControlTraceLoad>,
    pub control_trace_loaded_limit: usize,
    pub control_trace_loaded_signature: Option<String>,
    pub control_trace_source_kind: ControlTraceSourceKind,
    pub control_trace_source_detail: Option<String>,
    pub control_trace_kind: ControlTraceKindFilter,
    pub control_trace_query: String,
    pub control_trace_entries: Vec<ControlTraceRecordState>,
    pub control_trace_last_loaded_ms: Option<u64>,
    pub control_trace_last_error: Option<String>,
    pub request_ledger_summary_filters: RequestLedgerSummaryFilterState,
    pub request_ledger_summary_group: crate::request_ledger::RequestUsageSummaryGroup,
    pub request_ledger_summary_limit: usize,
    pub(super) request_ledger_summary_requested_signature: Option<String>,
    pub(super) request_ledger_summary_requested_group:
        crate::request_ledger::RequestUsageSummaryGroup,
    pub(super) request_ledger_summary_requested_limit: usize,
    pub(super) request_ledger_summary_requested_filters: crate::request_ledger::RequestLogFilters,
    pub(super) request_ledger_summary_load_seq: u64,
    pub(super) request_ledger_summary_load: Option<RequestLedgerSummaryLoad>,
    pub request_ledger_summary_loaded_signature: Option<String>,
    pub request_ledger_summary_loaded_group: crate::request_ledger::RequestUsageSummaryGroup,
    pub request_ledger_summary_loaded_limit: usize,
    pub request_ledger_summary_loaded_filters: crate::request_ledger::RequestLogFilters,
    pub request_ledger_summary_source_detail: Option<String>,
    pub request_ledger_summary_rows: Vec<crate::request_ledger::RequestUsageSummaryRow>,
    pub request_ledger_summary_loaded_at_ms: Option<u64>,
    pub request_ledger_summary_last_error: Option<String>,
}

impl Default for StatsViewState {
    fn default() -> Self {
        Self {
            pricing_editor: StatsPricingEditorState::default(),
            control_trace_limit: 80,
            control_trace_requested_signature: None,
            control_trace_requested_limit: 0,
            control_trace_load_seq: 0,
            control_trace_load: None,
            control_trace_loaded_limit: 0,
            control_trace_loaded_signature: None,
            control_trace_source_kind: ControlTraceSourceKind::LocalFile,
            control_trace_source_detail: None,
            control_trace_kind: ControlTraceKindFilter::All,
            control_trace_query: String::new(),
            control_trace_entries: Vec::new(),
            control_trace_last_loaded_ms: None,
            control_trace_last_error: None,
            request_ledger_summary_filters: RequestLedgerSummaryFilterState::default(),
            request_ledger_summary_group: crate::request_ledger::RequestUsageSummaryGroup::Station,
            request_ledger_summary_limit: 30,
            request_ledger_summary_requested_signature: None,
            request_ledger_summary_requested_group:
                crate::request_ledger::RequestUsageSummaryGroup::Station,
            request_ledger_summary_requested_limit: 0,
            request_ledger_summary_requested_filters:
                crate::request_ledger::RequestLogFilters::default(),
            request_ledger_summary_load_seq: 0,
            request_ledger_summary_load: None,
            request_ledger_summary_loaded_signature: None,
            request_ledger_summary_loaded_group:
                crate::request_ledger::RequestUsageSummaryGroup::Station,
            request_ledger_summary_loaded_limit: 0,
            request_ledger_summary_loaded_filters:
                crate::request_ledger::RequestLogFilters::default(),
            request_ledger_summary_source_detail: None,
            request_ledger_summary_rows: Vec::new(),
            request_ledger_summary_loaded_at_ms: None,
            request_ledger_summary_last_error: None,
        }
    }
}

fn normalize_text_filter(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_status_bound(
    value: &str,
    error: RequestLedgerSummaryFilterParseError,
) -> Result<Option<u64>, RequestLedgerSummaryFilterParseError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value.parse::<u64>().map(Some).map_err(|_| error)
}

#[derive(Debug)]
pub(super) struct StatsPricingEditorState {
    pub(super) selected_model_id: Option<String>,
    pub(super) draft_model_id: String,
    pub(super) display_name: String,
    pub(super) aliases: String,
    pub(super) input_per_1m_usd: String,
    pub(super) output_per_1m_usd: String,
    pub(super) cache_read_input_per_1m_usd: String,
    pub(super) cache_creation_input_per_1m_usd: String,
    pub(super) confidence: crate::pricing::CostConfidence,
}

impl Default for StatsPricingEditorState {
    fn default() -> Self {
        Self {
            selected_model_id: None,
            draft_model_id: String::new(),
            display_name: String::new(),
            aliases: String::new(),
            input_per_1m_usd: String::new(),
            output_per_1m_usd: String::new(),
            cache_read_input_per_1m_usd: String::new(),
            cache_creation_input_per_1m_usd: String::new(),
            confidence: crate::pricing::CostConfidence::Estimated,
        }
    }
}

#[derive(Debug)]
pub struct SetupViewState {
    pub import_codex_on_init: bool,
    pub(super) config_init_seq: u64,
    pub(super) config_init_load: Option<SetupConfigInitLoad>,
}

impl Default for SetupViewState {
    fn default() -> Self {
        Self {
            import_codex_on_init: true,
            config_init_seq: 0,
            config_init_load: None,
        }
    }
}

#[derive(Debug)]
pub(super) struct SetupConfigInitLoad {
    pub(super) seq: u64,
    pub(super) rx: std::sync::mpsc::Receiver<(u64, anyhow::Result<(std::path::PathBuf, String)>)>,
    pub(super) join: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ProxySettingsMode {
    #[default]
    Form,
    Raw,
}

#[derive(Debug)]
pub struct ProxySettingsViewState {
    pub(super) mode: ProxySettingsMode,
    pub(super) working: Option<ProxySettingsWorkingDocument>,
    pub(super) load_error: Option<String>,
    pub(super) save_seq: u64,
    pub(super) save_load: Option<ProxySettingsSaveLoad>,
    pub(super) import_codex: ImportCodexModalState,
    pub(super) provider_editor: ProxySettingsProviderEditorState,
    pub(super) routing_editor: ProxySettingsRoutingEditorState,
}

impl Default for ProxySettingsViewState {
    fn default() -> Self {
        Self {
            mode: ProxySettingsMode::Form,
            working: None,
            load_error: None,
            save_seq: 0,
            save_load: None,
            import_codex: ImportCodexModalState::default(),
            provider_editor: ProxySettingsProviderEditorState::default(),
            routing_editor: ProxySettingsRoutingEditorState::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum ProxySettingsWorkingDocument {
    V3(crate::config::ProxyConfigV3),
}

#[derive(Debug)]
pub(super) struct ProxySettingsSaveLoad {
    pub(super) seq: u64,
    pub(super) message: String,
    pub(super) reload_runtime: bool,
    pub(super) rx: std::sync::mpsc::Receiver<(u64, anyhow::Result<String>)>,
    pub(super) join: tokio::task::JoinHandle<()>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ProxySettingsProviderEditorService {
    #[default]
    Codex,
    Claude,
}

#[derive(Debug)]
pub(super) struct ProxySettingsProviderEditorState {
    pub(super) service: ProxySettingsProviderEditorService,
    pub(super) selected_provider: Option<String>,
    pub(super) draft_name: String,
    pub(super) alias: String,
    pub(super) base_url: String,
    pub(super) auth_token_env: String,
    pub(super) api_key_env: String,
    pub(super) tags: String,
    pub(super) enabled: bool,
}

impl Default for ProxySettingsProviderEditorState {
    fn default() -> Self {
        Self {
            service: ProxySettingsProviderEditorService::Codex,
            selected_provider: None,
            draft_name: String::new(),
            alias: String::new(),
            base_url: String::new(),
            auth_token_env: String::new(),
            api_key_env: String::new(),
            tags: String::new(),
            enabled: true,
        }
    }
}

#[derive(Debug)]
pub(super) struct ProxySettingsRoutingEditorState {
    pub(super) source_signature: Option<String>,
    pub(super) policy: crate::config::RoutingPolicyV3,
    pub(super) target: String,
    pub(super) order: String,
    pub(super) prefer_tags: String,
    pub(super) on_exhausted: crate::config::RoutingExhaustedActionV3,
}

impl Default for ProxySettingsRoutingEditorState {
    fn default() -> Self {
        Self {
            source_signature: None,
            policy: crate::config::RoutingPolicyV3::OrderedFailover,
            target: String::new(),
            order: String::new(),
            prefer_tags: String::new(),
            on_exhausted: crate::config::RoutingExhaustedActionV3::Continue,
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
pub(super) struct RequestLedgerLoad {
    pub(super) seq: u64,
    pub(super) source_signature: Option<String>,
    pub(super) limit: usize,
    pub(super) rx: std::sync::mpsc::Receiver<(
        u64,
        anyhow::Result<crate::gui::proxy_control::RequestLedgerReadResult>,
    )>,
    pub(super) join: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
pub struct RequestsViewState {
    pub errors_only: bool,
    pub scope_session: bool,
    pub model_filter: String,
    pub station_filter: String,
    pub provider_filter: String,
    pub fast_only: bool,
    pub retried_only: bool,
    pub focused_session_id: Option<String>,
    pub selected_idx: usize,
    pub include_request_ledger: bool,
    pub request_ledger_limit: usize,
    pub(super) request_ledger_requested_signature: Option<String>,
    pub(super) request_ledger_requested_limit: usize,
    pub(super) request_ledger_load_seq: u64,
    pub(super) request_ledger_load: Option<RequestLedgerLoad>,
    pub request_ledger_loaded_limit: usize,
    pub request_ledger_loaded_signature: Option<String>,
    pub request_ledger_loaded_at_ms: Option<u64>,
    pub request_ledger_last_error: Option<String>,
    pub request_ledger_source_detail: Option<String>,
    pub request_ledger_records: Vec<FinishedRequest>,
}

impl Default for RequestsViewState {
    fn default() -> Self {
        Self {
            errors_only: false,
            scope_session: true,
            model_filter: String::new(),
            station_filter: String::new(),
            provider_filter: String::new(),
            fast_only: false,
            retried_only: false,
            focused_session_id: None,
            selected_idx: 0,
            include_request_ledger: false,
            request_ledger_limit: 1000,
            request_ledger_requested_signature: None,
            request_ledger_requested_limit: 0,
            request_ledger_load_seq: 0,
            request_ledger_load: None,
            request_ledger_loaded_limit: 0,
            request_ledger_loaded_signature: None,
            request_ledger_loaded_at_ms: None,
            request_ledger_last_error: None,
            request_ledger_source_detail: None,
            request_ledger_records: Vec::new(),
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
