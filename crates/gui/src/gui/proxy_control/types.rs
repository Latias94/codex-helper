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
use crate::dashboard_core::{
    ControlProfileOption, HostLocalControlPlaneCapabilities, OperatorHealthSummary,
    OperatorRetrySummary, OperatorRuntimeSummary, OperatorSummaryCounts, OperatorSummaryLinks,
    ProviderOption, RemoteAdminAccessCapabilities, SharedControlPlaneCapabilities, StationOption,
    WindowStats,
};
use crate::logging::ControlTraceLogEntry;
use crate::proxy::{local_admin_base_url_for_proxy_port, local_proxy_base_url};
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
    pub lb_view: HashMap<String, LbConfigView>,
    pub runtime_loaded_at_ms: Option<u64>,
    pub runtime_source_mtime_ms: Option<u64>,
    pub operator_runtime_summary: Option<OperatorRuntimeSummary>,
    pub operator_retry_summary: Option<OperatorRetrySummary>,
    pub operator_health_summary: Option<OperatorHealthSummary>,
    pub operator_counts: Option<OperatorSummaryCounts>,
    pub operator_summary_links: Option<OperatorSummaryLinks>,
    pub supports_operator_summary_api: bool,
    pub configured_retry: Option<RetryConfig>,
    pub resolved_retry: Option<ResolvedRetryConfig>,
    pub supports_retry_config_api: bool,
    pub persisted_providers: BTreeMap<String, PersistedProviderSpec>,
    pub supports_provider_spec_api: bool,
    pub persisted_stations: BTreeMap<String, PersistedStationSpec>,
    pub persisted_station_providers: BTreeMap<String, PersistedStationProviderRef>,
    pub supports_station_spec_api: bool,
    pub supports_persisted_station_settings: bool,
    pub supports_default_profile_override: bool,
    pub supports_station_runtime_override: bool,
    pub supports_session_override_reset: bool,
    pub supports_control_trace_api: bool,
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
            lb_view: HashMap::new(),
            runtime_loaded_at_ms: None,
            runtime_source_mtime_ms: None,
            operator_runtime_summary: None,
            operator_retry_summary: None,
            operator_health_summary: None,
            operator_counts: None,
            operator_summary_links: None,
            supports_operator_summary_api: false,
            configured_retry: None,
            resolved_retry: None,
            supports_retry_config_api: false,
            persisted_providers: BTreeMap::new(),
            supports_provider_spec_api: false,
            persisted_stations: BTreeMap::new(),
            persisted_station_providers: BTreeMap::new(),
            supports_station_spec_api: false,
            supports_persisted_station_settings: false,
            supports_default_profile_override: false,
            supports_station_runtime_override: false,
            supports_session_override_reset: false,
            supports_control_trace_api: false,
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
    pub providers: Vec<ProviderOption>,
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
    pub operator_runtime_summary: Option<OperatorRuntimeSummary>,
    pub operator_retry_summary: Option<OperatorRetrySummary>,
    pub operator_health_summary: Option<OperatorHealthSummary>,
    pub operator_counts: Option<OperatorSummaryCounts>,
    pub supports_operator_summary_api: bool,
    pub configured_retry: Option<RetryConfig>,
    pub resolved_retry: Option<ResolvedRetryConfig>,
    pub supports_v1: bool,
    pub supports_retry_config_api: bool,
    pub supports_persisted_station_settings: bool,
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
    pub configured_retry: Option<RetryConfig>,
    pub resolved_retry: Option<ResolvedRetryConfig>,
    pub lb_view: HashMap<String, LbConfigView>,
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

pub(super) struct PortInUseModal {
    pub(super) port: u16,
    pub(super) remember_choice: bool,
    pub(super) chosen_new_port: u16,
}
