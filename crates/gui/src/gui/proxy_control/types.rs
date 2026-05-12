use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use reqwest::Client;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::{
    PersistedProviderSpec, PersistedStationProviderRef, PersistedStationSpec, ProxyConfig,
    ResolvedRetryConfig, RetryConfig, ServiceKind,
};
use crate::dashboard_core::snapshot::DashboardSnapshot;
use crate::dashboard_core::{
    ControlProfileOption, HostLocalControlPlaneCapabilities, OperatorHealthSummary,
    OperatorRetrySummary, OperatorRuntimeSummary, OperatorSummaryCounts, OperatorSummaryLinks,
    ProviderOption, RemoteAdminAccessCapabilities, SharedControlPlaneCapabilities, StationOption,
    WindowStats,
};
use crate::logging::ControlTraceLogEntry;
use crate::pricing::{ModelPriceCatalogSnapshot, bundled_model_price_catalog_snapshot};
use crate::proxy::{local_admin_base_url_for_proxy_port, local_proxy_base_url};
use crate::routing_explain::RoutingExplainResponse;
use crate::state::{
    ActiveRequest, FinishedRequest, HealthCheckStatus, LbConfigView, ProviderBalanceSnapshot,
    ProxyState, SessionIdentityCard, SessionStats, StationHealth, UsageRollupView,
};

use super::attached_discovery::DiscoveredProxy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortInUseAction {
    Ask,
    Attach,
    StartNewPort,
    Exit,
}

impl PortInUseAction {
    pub fn as_str(self) -> &'static str {
        match self {
            PortInUseAction::Ask => "ask",
            PortInUseAction::Attach => "attach",
            PortInUseAction::StartNewPort => "start_new_port",
            PortInUseAction::Exit => "exit",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "attach" => PortInUseAction::Attach,
            "start_new_port" | "start-new-port" | "new_port" => PortInUseAction::StartNewPort,
            "exit" => PortInUseAction::Exit,
            _ => PortInUseAction::Ask,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyModeKind {
    Stopped,
    Starting,
    Running,
    Attached,
}

pub(super) struct RunningRefreshResult {
    pub(super) snapshot: DashboardSnapshot,
    pub(super) configured_active_station: Option<String>,
    pub(super) effective_active_station: Option<String>,
    pub(super) configured_default_profile: Option<String>,
    pub(super) default_profile: Option<String>,
    pub(super) profiles: Vec<ControlProfileOption>,
    pub(super) stations: Vec<StationOption>,
    pub(super) routing_explain: Option<RoutingExplainResponse>,
}

pub(super) struct AttachedRefreshResult {
    pub(super) management_base_url: String,
    pub(super) api_version: Option<u32>,
    pub(super) service_name: Option<String>,
    pub(super) active: Vec<ActiveRequest>,
    pub(super) recent: Vec<FinishedRequest>,
    pub(super) session_cards: Vec<SessionIdentityCard>,
    pub(super) global_station_override: Option<String>,
    pub(super) configured_active_station: Option<String>,
    pub(super) effective_active_station: Option<String>,
    pub(super) configured_default_profile: Option<String>,
    pub(super) default_profile: Option<String>,
    pub(super) profiles: Vec<ControlProfileOption>,
    pub(super) providers: Vec<ProviderOption>,
    pub(super) session_model: HashMap<String, String>,
    pub(super) session_station: HashMap<String, String>,
    pub(super) session_effort: HashMap<String, String>,
    pub(super) session_service_tier: HashMap<String, String>,
    pub(super) session_stats: HashMap<String, SessionStats>,
    pub(super) stations: Vec<StationOption>,
    pub(super) station_health: HashMap<String, StationHealth>,
    pub(super) provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub(super) health_checks: HashMap<String, HealthCheckStatus>,
    pub(super) usage_rollup: UsageRollupView,
    pub(super) stats_5m: WindowStats,
    pub(super) stats_1h: WindowStats,
    pub(super) pricing_catalog: ModelPriceCatalogSnapshot,
    pub(super) routing_explain: Option<RoutingExplainResponse>,
    pub(super) lb_view: HashMap<String, LbConfigView>,
    pub(super) runtime_loaded_at_ms: Option<u64>,
    pub(super) runtime_source_mtime_ms: Option<u64>,
    pub(super) operator_runtime_summary: Option<OperatorRuntimeSummary>,
    pub(super) operator_retry_summary: Option<OperatorRetrySummary>,
    pub(super) operator_health_summary: Option<OperatorHealthSummary>,
    pub(super) operator_counts: Option<OperatorSummaryCounts>,
    pub(super) operator_summary_links: Option<OperatorSummaryLinks>,
    pub(super) supports_operator_summary_api: bool,
    pub(super) supports_pricing_catalog_api: bool,
    pub(super) supports_routing_explain_api: bool,
    pub(super) configured_retry: Option<RetryConfig>,
    pub(super) resolved_retry: Option<ResolvedRetryConfig>,
    pub(super) supports_retry_config_api: bool,
    pub(super) persisted_providers: BTreeMap<String, PersistedProviderSpec>,
    pub(super) supports_provider_spec_api: bool,
    pub(super) persisted_stations: BTreeMap<String, PersistedStationSpec>,
    pub(super) persisted_station_providers: BTreeMap<String, PersistedStationProviderRef>,
    pub(super) supports_station_spec_api: bool,
    pub(super) supports_default_profile_override: bool,
    pub(super) supports_station_runtime_override: bool,
    pub(super) supports_session_override_reset: bool,
    pub(super) supports_control_trace_api: bool,
    pub(super) supports_request_ledger_api: bool,
    pub(super) supports_request_ledger_summary_api: bool,
    pub(super) supports_station_api: bool,
    pub(super) shared_capabilities: SharedControlPlaneCapabilities,
    pub(super) host_local_capabilities: HostLocalControlPlaneCapabilities,
    pub(super) remote_admin_access: RemoteAdminAccessCapabilities,
}

pub(super) enum ProxyBackgroundRefreshResult {
    Running(Box<RunningRefreshResult>),
    Attached(Box<AttachedRefreshResult>),
}

pub(super) struct ProxyBackgroundRefreshTask {
    pub(super) rx: std::sync::mpsc::Receiver<anyhow::Result<ProxyBackgroundRefreshResult>>,
    pub(super) join: JoinHandle<()>,
}

pub struct AttachedStatus {
    pub base_url: String,
    pub admin_base_url: String,
    pub port: u16,
    pub last_refresh: Option<Instant>,
    pub last_error: Option<String>,
    pub api_version: Option<u32>,
    pub service_name: Option<String>,
    pub active: Vec<ActiveRequest>,
    pub recent: Vec<FinishedRequest>,
    pub session_cards: Vec<SessionIdentityCard>,
    pub global_station_override: Option<String>,
    pub configured_active_station: Option<String>,
    pub effective_active_station: Option<String>,
    pub configured_default_profile: Option<String>,
    pub default_profile: Option<String>,
    pub profiles: Vec<ControlProfileOption>,
    pub providers: Vec<ProviderOption>,
    pub session_model_overrides: HashMap<String, String>,
    pub session_station_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub stations: Vec<StationOption>,
    pub station_health: HashMap<String, StationHealth>,
    pub provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
    pub pricing_catalog: ModelPriceCatalogSnapshot,
    pub routing_explain: Option<RoutingExplainResponse>,
    pub supports_routing_explain_api: bool,
    pub lb_view: HashMap<String, LbConfigView>,
    pub runtime_loaded_at_ms: Option<u64>,
    pub runtime_source_mtime_ms: Option<u64>,
    pub operator_runtime_summary: Option<OperatorRuntimeSummary>,
    pub operator_retry_summary: Option<OperatorRetrySummary>,
    pub operator_health_summary: Option<OperatorHealthSummary>,
    pub operator_counts: Option<OperatorSummaryCounts>,
    pub operator_summary_links: Option<OperatorSummaryLinks>,
    pub supports_operator_summary_api: bool,
    pub supports_pricing_catalog_api: bool,
    pub configured_retry: Option<RetryConfig>,
    pub resolved_retry: Option<ResolvedRetryConfig>,
    pub supports_retry_config_api: bool,
    pub persisted_providers: BTreeMap<String, PersistedProviderSpec>,
    pub supports_provider_spec_api: bool,
    pub persisted_stations: BTreeMap<String, PersistedStationSpec>,
    pub persisted_station_providers: BTreeMap<String, PersistedStationProviderRef>,
    pub supports_station_spec_api: bool,
    pub supports_default_profile_override: bool,
    pub supports_station_runtime_override: bool,
    pub supports_session_override_reset: bool,
    pub supports_control_trace_api: bool,
    pub supports_request_ledger_api: bool,
    pub supports_request_ledger_summary_api: bool,
    pub supports_station_api: bool,
    pub shared_capabilities: SharedControlPlaneCapabilities,
    pub host_local_capabilities: HostLocalControlPlaneCapabilities,
    pub remote_admin_access: RemoteAdminAccessCapabilities,
}

impl AttachedStatus {
    pub(super) fn new(port: u16) -> Self {
        Self {
            base_url: local_proxy_base_url(port),
            admin_base_url: local_admin_base_url_for_proxy_port(port),
            port,
            last_refresh: None,
            last_error: None,
            api_version: None,
            service_name: None,
            active: Vec::new(),
            recent: Vec::new(),
            session_cards: Vec::new(),
            global_station_override: None,
            configured_active_station: None,
            effective_active_station: None,
            configured_default_profile: None,
            default_profile: None,
            profiles: Vec::new(),
            providers: Vec::new(),
            session_model_overrides: HashMap::new(),
            session_station_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_service_tier_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            stations: Vec::new(),
            station_health: HashMap::new(),
            provider_balances: HashMap::new(),
            health_checks: HashMap::new(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            pricing_catalog: bundled_model_price_catalog_snapshot(),
            routing_explain: None,
            supports_routing_explain_api: false,
            lb_view: HashMap::new(),
            runtime_loaded_at_ms: None,
            runtime_source_mtime_ms: None,
            operator_runtime_summary: None,
            operator_retry_summary: None,
            operator_health_summary: None,
            operator_counts: None,
            operator_summary_links: None,
            supports_operator_summary_api: false,
            supports_pricing_catalog_api: false,
            configured_retry: None,
            resolved_retry: None,
            supports_retry_config_api: false,
            persisted_providers: BTreeMap::new(),
            supports_provider_spec_api: false,
            persisted_stations: BTreeMap::new(),
            persisted_station_providers: BTreeMap::new(),
            supports_station_spec_api: false,
            supports_default_profile_override: false,
            supports_station_runtime_override: false,
            supports_session_override_reset: false,
            supports_control_trace_api: false,
            supports_request_ledger_api: false,
            supports_request_ledger_summary_api: false,
            supports_station_api: false,
            shared_capabilities: SharedControlPlaneCapabilities::default(),
            host_local_capabilities: HostLocalControlPlaneCapabilities::default(),
            remote_admin_access: RemoteAdminAccessCapabilities::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GuiRuntimeSnapshot {
    pub kind: ProxyModeKind,
    pub base_url: Option<String>,
    pub port: Option<u16>,
    pub service_name: Option<String>,
    pub last_error: Option<String>,
    pub active: Vec<ActiveRequest>,
    pub recent: Vec<FinishedRequest>,
    pub session_cards: Vec<SessionIdentityCard>,
    pub global_station_override: Option<String>,
    pub configured_active_station: Option<String>,
    pub effective_active_station: Option<String>,
    pub configured_default_profile: Option<String>,
    pub default_profile: Option<String>,
    pub profiles: Vec<ControlProfileOption>,
    pub session_model_overrides: HashMap<String, String>,
    pub session_station_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub stations: Vec<StationOption>,
    pub usage_rollup: UsageRollupView,
    pub provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
    pub pricing_catalog: ModelPriceCatalogSnapshot,
    pub routing_explain: Option<RoutingExplainResponse>,
    pub supports_routing_explain_api: bool,
    pub operator_retry_summary: Option<OperatorRetrySummary>,
    pub supports_pricing_catalog_api: bool,
    pub configured_retry: Option<RetryConfig>,
    pub resolved_retry: Option<ResolvedRetryConfig>,
    pub supports_v1: bool,
    pub supports_retry_config_api: bool,
    pub supports_default_profile_override: bool,
    pub supports_station_runtime_override: bool,
    pub supports_session_override_reset: bool,
    pub shared_capabilities: SharedControlPlaneCapabilities,
    pub host_local_capabilities: HostLocalControlPlaneCapabilities,
    pub remote_admin_access: RemoteAdminAccessCapabilities,
}

pub struct RunningProxy {
    pub service_name: &'static str,
    pub port: u16,
    pub admin_port: u16,
    pub state: Arc<ProxyState>,
    pub cfg: Arc<ProxyConfig>,
    pub last_refresh: Option<Instant>,
    pub last_error: Option<String>,
    pub active: Vec<ActiveRequest>,
    pub recent: Vec<FinishedRequest>,
    pub session_cards: Vec<SessionIdentityCard>,
    pub global_station_override: Option<String>,
    pub configured_active_station: Option<String>,
    pub effective_active_station: Option<String>,
    pub configured_default_profile: Option<String>,
    pub default_profile: Option<String>,
    pub profiles: Vec<ControlProfileOption>,
    pub session_model_overrides: HashMap<String, String>,
    pub session_station_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub stations: Vec<StationOption>,
    pub station_health: HashMap<String, StationHealth>,
    pub provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
    pub configured_retry: Option<RetryConfig>,
    pub resolved_retry: Option<ResolvedRetryConfig>,
    pub lb_view: HashMap<String, LbConfigView>,
    pub routing_explain: Option<RoutingExplainResponse>,
    pub(super) shutdown_tx: watch::Sender<bool>,
    pub(super) server_handle: Option<JoinHandle<anyhow::Result<()>>>,
}

#[allow(clippy::large_enum_variant)]
pub enum ProxyMode {
    Stopped,
    Starting,
    Running(RunningProxy),
    Attached(AttachedStatus),
}

pub struct ProxyController {
    pub(super) mode: ProxyMode,
    pub(super) desired_port: u16,
    pub(super) desired_service: ServiceKind,
    pub(super) last_start_error: Option<String>,
    pub(super) port_in_use_modal: Option<PortInUseModal>,
    pub(super) http_client: Client,
    pub(super) discovered: Vec<DiscoveredProxy>,
    pub(super) last_discovery_scan: Option<Instant>,
    pub(super) background_refresh: Option<ProxyBackgroundRefreshTask>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlTraceDataSource {
    LocalFile {
        path: PathBuf,
    },
    AttachedApi {
        admin_base_url: String,
    },
    AttachedFallbackLocal {
        admin_base_url: String,
        path: PathBuf,
    },
}

impl ControlTraceDataSource {
    pub fn signature(&self) -> String {
        match self {
            ControlTraceDataSource::LocalFile { path } => format!("local:{}", path.display()),
            ControlTraceDataSource::AttachedApi { admin_base_url } => {
                format!("attached-api:{admin_base_url}")
            }
            ControlTraceDataSource::AttachedFallbackLocal {
                admin_base_url,
                path,
            } => format!("attached-fallback:{admin_base_url}:{}", path.display()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControlTraceReadResult {
    pub source: ControlTraceDataSource,
    pub entries: Vec<ControlTraceLogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestLedgerDataSource {
    LocalFile { path: PathBuf },
    AttachedApi { admin_base_url: String },
}

impl RequestLedgerDataSource {
    pub fn signature(&self) -> String {
        match self {
            RequestLedgerDataSource::LocalFile { path } => {
                format!("local:{}", path.display())
            }
            RequestLedgerDataSource::AttachedApi { admin_base_url } => {
                format!("attached-api:{admin_base_url}")
            }
        }
    }

    pub fn display_detail(&self) -> String {
        match self {
            RequestLedgerDataSource::LocalFile { path } => format!("path: {}", path.display()),
            RequestLedgerDataSource::AttachedApi { admin_base_url } => {
                format!("admin API: {admin_base_url}")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestLedgerReadResult {
    pub source: RequestLedgerDataSource,
    pub records: Vec<FinishedRequest>,
}

#[derive(Debug, Clone)]
pub struct RequestLedgerSummaryReadResult {
    pub source: RequestLedgerDataSource,
    pub rows: Vec<crate::request_ledger::RequestUsageSummaryRow>,
}

pub(super) struct PortInUseModal {
    pub(super) port: u16,
    pub(super) remember_choice: bool,
    pub(super) chosen_new_port: u16,
}
