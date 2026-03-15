use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use futures_util::future::join_all;
use reqwest::Client;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use std::collections::{BTreeMap, HashMap};

use crate::config::{
    PersistedProviderSpec, PersistedStationProviderRef, PersistedStationSpec, ProxyConfig,
    ResolvedRetryConfig, RetryConfig, ServiceKind, load_or_bootstrap_for_service,
    model_routing_warnings,
};
use crate::dashboard_core::{
    ApiV1Capabilities, ApiV1Snapshot, ControlPlaneSurfaceCapabilities, ControlProfileOption,
    HostLocalControlPlaneCapabilities, RemoteAdminAccessCapabilities,
    SharedControlPlaneCapabilities, StationOption, WindowStats, build_dashboard_snapshot,
    build_profile_options_from_mgr, build_station_options_from_mgr,
};
use crate::logging::{ControlTraceLogEntry, control_trace_path, read_recent_control_trace_entries};
use crate::proxy::{
    ProxyService, admin_listener_router, admin_port_for_proxy_port,
    local_admin_base_url_for_proxy_port, local_proxy_base_url,
    proxy_only_router_with_admin_base_url,
};
use crate::state::{
    ActiveRequest, FinishedRequest, HealthCheckStatus, LbConfigView, ProxyState,
    RuntimeConfigState, SessionIdentityCard, SessionManualOverrides, SessionStats, StationHealth,
    UsageRollupView,
};

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
    pub session_model_overrides: HashMap<String, String>,
    pub session_station_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub stations: Vec<StationOption>,
    pub station_health: HashMap<String, StationHealth>,
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
    pub lb_view: HashMap<String, LbConfigView>,
    pub runtime_loaded_at_ms: Option<u64>,
    pub runtime_source_mtime_ms: Option<u64>,
    pub configured_retry: Option<RetryConfig>,
    pub resolved_retry: Option<ResolvedRetryConfig>,
    pub supports_retry_config_api: bool,
    pub persisted_providers: BTreeMap<String, PersistedProviderSpec>,
    pub supports_provider_spec_api: bool,
    pub persisted_stations: BTreeMap<String, PersistedStationSpec>,
    pub persisted_station_providers: BTreeMap<String, PersistedStationProviderRef>,
    pub supports_station_spec_api: bool,
    pub supports_persisted_station_config: bool,
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
    fn new(port: u16) -> Self {
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
            session_model_overrides: HashMap::new(),
            session_station_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_service_tier_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            stations: Vec::new(),
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            lb_view: HashMap::new(),
            runtime_loaded_at_ms: None,
            runtime_source_mtime_ms: None,
            configured_retry: None,
            resolved_retry: None,
            supports_retry_config_api: false,
            persisted_providers: BTreeMap::new(),
            supports_provider_spec_api: false,
            persisted_stations: BTreeMap::new(),
            persisted_station_providers: BTreeMap::new(),
            supports_station_spec_api: false,
            supports_persisted_station_config: false,
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
    pub session_model_overrides: HashMap<String, String>,
    pub session_station_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub stations: Vec<StationOption>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
    pub configured_retry: Option<RetryConfig>,
    pub resolved_retry: Option<ResolvedRetryConfig>,
    pub supports_v1: bool,
    pub supports_retry_config_api: bool,
    pub supports_persisted_station_config: bool,
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
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
    pub configured_retry: Option<RetryConfig>,
    pub resolved_retry: Option<ResolvedRetryConfig>,
    pub lb_view: HashMap<String, LbConfigView>,
    shutdown_tx: watch::Sender<bool>,
    server_handle: Option<JoinHandle<anyhow::Result<()>>>,
}

pub enum ProxyMode {
    Stopped,
    Starting,
    Running(RunningProxy),
    Attached(AttachedStatus),
}

pub struct ProxyController {
    mode: ProxyMode,
    desired_port: u16,
    desired_service: ServiceKind,
    last_start_error: Option<String>,

    port_in_use_modal: Option<PortInUseModal>,
    http_client: Client,

    discovered: Vec<DiscoveredProxy>,
    last_discovery_scan: Option<Instant>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredProxy {
    pub port: u16,
    pub base_url: String,
    pub admin_base_url: String,
    pub api_version: Option<u32>,
    pub service_name: Option<String>,
    pub endpoints: Vec<String>,
    pub surface_capabilities: ControlPlaneSurfaceCapabilities,
    pub runtime_loaded_at_ms: Option<u64>,
    pub last_error: Option<String>,
    pub shared_capabilities: SharedControlPlaneCapabilities,
    pub host_local_capabilities: HostLocalControlPlaneCapabilities,
    pub remote_admin_access: RemoteAdminAccessCapabilities,
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

struct PortInUseModal {
    port: u16,
    remember_choice: bool,
    chosen_new_port: u16,
}

fn attached_management_candidates(att: &AttachedStatus) -> Vec<String> {
    let mut out = vec![att.admin_base_url.clone()];
    if att.base_url != att.admin_base_url {
        out.push(att.base_url.clone());
    }
    out
}

fn local_shared_control_plane_capabilities() -> SharedControlPlaneCapabilities {
    SharedControlPlaneCapabilities {
        session_observability: true,
        request_history: true,
    }
}

fn local_host_local_control_plane_capabilities() -> HostLocalControlPlaneCapabilities {
    let host_local_history = crate::config::codex_sessions_dir().is_dir();
    HostLocalControlPlaneCapabilities {
        session_history: host_local_history,
        transcript_read: host_local_history,
        cwd_enrichment: host_local_history,
    }
}

fn local_remote_admin_access_capabilities() -> RemoteAdminAccessCapabilities {
    RemoteAdminAccessCapabilities {
        loopback_without_token: true,
        remote_requires_token: true,
        remote_enabled: std::env::var(crate::proxy::ADMIN_TOKEN_ENV_VAR)
            .ok()
            .is_some_and(|value| !value.trim().is_empty()),
        token_header: crate::proxy::ADMIN_TOKEN_HEADER.to_string(),
        token_env_var: crate::proxy::ADMIN_TOKEN_ENV_VAR.to_string(),
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ResolvedApiV1Surface {
    snapshot: bool,
    profiles: bool,
    retry_config: bool,
    provider_specs: bool,
    station_specs: bool,
    persisted_station_config: bool,
    default_profile_override: bool,
    session_override_reset: bool,
    control_trace: bool,
    station_api: bool,
    station_runtime: bool,
    session_override_aggregate: bool,
    session_station: bool,
    session_reasoning_effort: bool,
    session_model: bool,
    session_service_tier: bool,
}

fn supports_capability_flag(flag: bool, endpoints: &[String], endpoint: &str) -> bool {
    flag || endpoints.iter().any(|candidate| candidate == endpoint)
}

fn supports_any_capability_flag(
    flag: bool,
    endpoints: &[String],
    endpoint_candidates: &[&str],
) -> bool {
    flag || endpoint_candidates
        .iter()
        .any(|endpoint| supports_capability_flag(false, endpoints, endpoint))
}

fn resolve_api_v1_surface(
    surface: &ControlPlaneSurfaceCapabilities,
    endpoints: &[String],
) -> ResolvedApiV1Surface {
    ResolvedApiV1Surface {
        snapshot: supports_capability_flag(
            surface.snapshot,
            endpoints,
            "/__codex_helper/api/v1/snapshot",
        ),
        profiles: supports_capability_flag(
            surface.profiles,
            endpoints,
            "/__codex_helper/api/v1/profiles",
        ),
        retry_config: supports_capability_flag(
            surface.retry_config,
            endpoints,
            "/__codex_helper/api/v1/retry/config",
        ),
        provider_specs: supports_capability_flag(
            surface.provider_specs,
            endpoints,
            "/__codex_helper/api/v1/providers/specs",
        ),
        station_specs: supports_capability_flag(
            surface.station_specs,
            endpoints,
            "/__codex_helper/api/v1/stations/specs",
        ),
        persisted_station_config: supports_any_capability_flag(
            surface.station_persisted_config,
            endpoints,
            &[
                "/__codex_helper/api/v1/stations/config-active",
                "/__codex_helper/api/v1/stations/{name}",
            ],
        ),
        default_profile_override: supports_capability_flag(
            surface.default_profile_override,
            endpoints,
            "/__codex_helper/api/v1/profiles/default",
        ),
        session_override_reset: supports_capability_flag(
            surface.session_override_reset,
            endpoints,
            "/__codex_helper/api/v1/overrides/session/reset",
        ),
        control_trace: supports_capability_flag(
            surface.control_trace,
            endpoints,
            "/__codex_helper/api/v1/control-trace",
        ),
        station_api: supports_any_capability_flag(
            surface.stations || surface.station_runtime || surface.station_probe,
            endpoints,
            &[
                "/__codex_helper/api/v1/stations",
                "/__codex_helper/api/v1/stations/runtime",
                "/__codex_helper/api/v1/stations/probe",
            ],
        ),
        station_runtime: supports_capability_flag(
            surface.station_runtime,
            endpoints,
            "/__codex_helper/api/v1/stations/runtime",
        ),
        session_override_aggregate: supports_capability_flag(
            surface.session_overrides,
            endpoints,
            "/__codex_helper/api/v1/overrides/session",
        ),
        session_station: supports_capability_flag(
            surface.session_station_override,
            endpoints,
            "/__codex_helper/api/v1/overrides/session/station",
        ),
        session_reasoning_effort: supports_capability_flag(
            surface.session_reasoning_effort_override,
            endpoints,
            "/__codex_helper/api/v1/overrides/session/effort",
        ),
        session_model: supports_capability_flag(
            surface.session_model_override,
            endpoints,
            "/__codex_helper/api/v1/overrides/session/model",
        ),
        session_service_tier: supports_capability_flag(
            surface.session_service_tier_override,
            endpoints,
            "/__codex_helper/api/v1/overrides/session/service-tier",
        ),
    }
}

fn admin_auth_token() -> Option<String> {
    std::env::var(crate::proxy::ADMIN_TOKEN_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn with_admin_auth(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    if let Some(token) = admin_auth_token() {
        builder.header(crate::proxy::ADMIN_TOKEN_HEADER, token)
    } else {
        builder
    }
}

async fn send_admin_request(builder: reqwest::RequestBuilder) -> anyhow::Result<reqwest::Response> {
    let response = with_admin_auth(builder).send().await?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::FORBIDDEN
        && (body.contains(crate::proxy::ADMIN_TOKEN_HEADER)
            || body.contains(crate::proxy::ADMIN_TOKEN_ENV_VAR))
    {
        bail!("admin access denied: {body}");
    }

    if body.trim().is_empty() {
        bail!("admin request failed: {status}");
    }
    bail!("admin request failed: {status}: {body}");
}

impl ProxyController {
    pub fn new(default_port: u16, default_service: ServiceKind) -> Self {
        Self {
            mode: ProxyMode::Stopped,
            desired_port: default_port,
            desired_service: default_service,
            last_start_error: None,
            port_in_use_modal: None,
            http_client: Client::new(),
            discovered: Vec::new(),
            last_discovery_scan: None,
        }
    }

    pub fn set_defaults(&mut self, port: u16, service: ServiceKind) {
        self.desired_port = port;
        self.desired_service = service;
    }

    pub fn desired_port(&self) -> u16 {
        self.desired_port
    }

    pub fn desired_service(&self) -> ServiceKind {
        self.desired_service
    }

    pub fn set_desired_port(&mut self, port: u16) {
        self.desired_port = port;
    }

    pub fn set_desired_service(&mut self, service: ServiceKind) {
        self.desired_service = service;
    }

    pub fn kind(&self) -> ProxyModeKind {
        match self.mode {
            ProxyMode::Stopped => ProxyModeKind::Stopped,
            ProxyMode::Starting => ProxyModeKind::Starting,
            ProxyMode::Running(_) => ProxyModeKind::Running,
            ProxyMode::Attached(_) => ProxyModeKind::Attached,
        }
    }

    pub fn last_start_error(&self) -> Option<&str> {
        self.last_start_error.as_deref()
    }

    pub fn discovered_proxies(&self) -> &[DiscoveredProxy] {
        &self.discovered
    }

    pub fn last_discovery_scan(&self) -> Option<Instant> {
        self.last_discovery_scan
    }

    pub fn running(&self) -> Option<&RunningProxy> {
        match &self.mode {
            ProxyMode::Running(r) => Some(r),
            _ => None,
        }
    }

    pub fn attached(&self) -> Option<&AttachedStatus> {
        match &self.mode {
            ProxyMode::Attached(s) => Some(s),
            _ => None,
        }
    }

    pub fn control_trace_source(&self) -> Option<ControlTraceDataSource> {
        match &self.mode {
            ProxyMode::Running(_) => Some(ControlTraceDataSource::LocalFile {
                path: control_trace_path(),
            }),
            ProxyMode::Attached(att) if att.supports_control_trace_api => {
                Some(ControlTraceDataSource::AttachedApi {
                    admin_base_url: att.admin_base_url.clone(),
                })
            }
            ProxyMode::Attached(att) => Some(ControlTraceDataSource::AttachedFallbackLocal {
                admin_base_url: att.admin_base_url.clone(),
                path: control_trace_path(),
            }),
            _ => None,
        }
    }

    pub fn control_trace_source_signature(&self) -> Option<String> {
        self.control_trace_source().map(|source| source.signature())
    }

    pub fn read_control_trace_entries(
        &self,
        rt: &tokio::runtime::Runtime,
        limit: usize,
    ) -> anyhow::Result<ControlTraceReadResult> {
        let limit = limit.clamp(20, 400);
        let source = self
            .control_trace_source()
            .ok_or_else(|| anyhow::anyhow!("proxy is not running/attached"))?;

        let entries = match &source {
            ControlTraceDataSource::LocalFile { .. }
            | ControlTraceDataSource::AttachedFallbackLocal { .. } => {
                read_recent_control_trace_entries(limit)?
            }
            ControlTraceDataSource::AttachedApi { admin_base_url } => {
                let client = self.http_client.clone();
                let admin_base_url = admin_base_url.clone();
                rt.block_on(async move {
                    let response = send_admin_request(
                        client
                            .get(format!(
                                "{admin_base_url}/__codex_helper/api/v1/control-trace?limit={limit}"
                            ))
                            .timeout(Duration::from_millis(800)),
                    )
                    .await?;
                    let entries = response.json::<Vec<ControlTraceLogEntry>>().await?;
                    Ok::<Vec<ControlTraceLogEntry>, anyhow::Error>(entries)
                })?
            }
        };

        Ok(ControlTraceReadResult { source, entries })
    }

    pub fn snapshot(&self) -> Option<GuiRuntimeSnapshot> {
        match &self.mode {
            ProxyMode::Running(r) => Some(GuiRuntimeSnapshot {
                kind: ProxyModeKind::Running,
                base_url: Some(local_proxy_base_url(r.port)),
                port: Some(r.port),
                service_name: Some(r.service_name.to_string()),
                last_error: r.last_error.clone(),
                active: r.active.clone(),
                recent: r.recent.clone(),
                session_cards: r.session_cards.clone(),
                global_station_override: r.global_station_override.clone(),
                configured_active_station: r.configured_active_station.clone(),
                effective_active_station: r.effective_active_station.clone(),
                configured_default_profile: r.configured_default_profile.clone(),
                default_profile: r.default_profile.clone(),
                profiles: r.profiles.clone(),
                session_model_overrides: r.session_model_overrides.clone(),
                session_station_overrides: r.session_station_overrides.clone(),
                session_effort_overrides: r.session_effort_overrides.clone(),
                session_service_tier_overrides: r.session_service_tier_overrides.clone(),
                session_stats: r.session_stats.clone(),
                stations: r.stations.clone(),
                usage_rollup: r.usage_rollup.clone(),
                stats_5m: r.stats_5m.clone(),
                stats_1h: r.stats_1h.clone(),
                configured_retry: r.configured_retry.clone(),
                resolved_retry: r.resolved_retry.clone(),
                supports_v1: true,
                supports_retry_config_api: true,
                supports_persisted_station_config: true,
                supports_default_profile_override: true,
                supports_station_runtime_override: true,
                supports_session_override_reset: true,
                shared_capabilities: local_shared_control_plane_capabilities(),
                host_local_capabilities: local_host_local_control_plane_capabilities(),
                remote_admin_access: local_remote_admin_access_capabilities(),
            }),
            ProxyMode::Attached(a) => Some(GuiRuntimeSnapshot {
                kind: ProxyModeKind::Attached,
                base_url: Some(a.base_url.clone()),
                port: Some(a.port),
                service_name: a.service_name.clone(),
                last_error: a.last_error.clone(),
                active: a.active.clone(),
                recent: a.recent.clone(),
                session_cards: a.session_cards.clone(),
                global_station_override: a.global_station_override.clone(),
                configured_active_station: a.configured_active_station.clone(),
                effective_active_station: a.effective_active_station.clone(),
                configured_default_profile: a.configured_default_profile.clone(),
                default_profile: a.default_profile.clone(),
                profiles: a.profiles.clone(),
                session_model_overrides: a.session_model_overrides.clone(),
                session_station_overrides: a.session_station_overrides.clone(),
                session_effort_overrides: a.session_effort_overrides.clone(),
                session_service_tier_overrides: a.session_service_tier_overrides.clone(),
                session_stats: a.session_stats.clone(),
                stations: a.stations.clone(),
                usage_rollup: a.usage_rollup.clone(),
                stats_5m: a.stats_5m.clone(),
                stats_1h: a.stats_1h.clone(),
                configured_retry: a.configured_retry.clone(),
                resolved_retry: a.resolved_retry.clone(),
                supports_v1: a.api_version == Some(1),
                supports_retry_config_api: a.supports_retry_config_api,
                supports_persisted_station_config: a.supports_persisted_station_config,
                supports_default_profile_override: a.supports_default_profile_override,
                supports_station_runtime_override: a.supports_station_runtime_override,
                supports_session_override_reset: a.supports_session_override_reset,
                shared_capabilities: a.shared_capabilities.clone(),
                host_local_capabilities: a.host_local_capabilities.clone(),
                remote_admin_access: a.remote_admin_access.clone(),
            }),
            _ => None,
        }
    }

    pub fn refresh_current_if_due(
        &mut self,
        rt: &tokio::runtime::Runtime,
        refresh_every: Duration,
    ) {
        match self.kind() {
            ProxyModeKind::Running => self.refresh_running_if_due(rt, refresh_every),
            ProxyModeKind::Attached => self.refresh_attached_if_due(rt, refresh_every),
            _ => {}
        }
    }

    pub fn show_port_in_use_modal(&self) -> bool {
        self.port_in_use_modal.is_some()
    }

    pub fn clear_port_in_use_modal(&mut self) {
        self.port_in_use_modal = None;
    }

    pub fn stop(&mut self, rt: &tokio::runtime::Runtime) -> anyhow::Result<()> {
        let ProxyMode::Running(mut running) = std::mem::replace(&mut self.mode, ProxyMode::Stopped)
        else {
            self.mode = ProxyMode::Stopped;
            return Ok(());
        };

        let _ = running.shutdown_tx.send(true);
        if let Some(mut handle) = running.server_handle.take() {
            let joined = rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(2), &mut handle).await
            });
            match joined {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => {
                    return Err(e);
                }
                Ok(Err(join_err)) => {
                    return Err(anyhow::anyhow!("server task join error: {join_err}"));
                }
                Err(_) => {
                    handle.abort();
                }
            }
        }
        Ok(())
    }

    pub fn request_attach(&mut self, port: u16) {
        self.request_attach_with_admin_base(port, None);
    }

    pub fn request_attach_with_admin_base(&mut self, port: u16, admin_base_url: Option<String>) {
        let mut attached = AttachedStatus::new(port);
        if let Some(admin_base_url) = admin_base_url {
            attached.admin_base_url = admin_base_url.clone();
            if let Some(discovered) = self.discovered.iter().find(|candidate| {
                candidate.port == port && candidate.admin_base_url == admin_base_url
            }) {
                let resolved_surface =
                    resolve_api_v1_surface(&discovered.surface_capabilities, &discovered.endpoints);
                attached.api_version = discovered.api_version;
                attached.service_name = discovered.service_name.clone();
                attached.supports_control_trace_api = resolved_surface.control_trace;
                attached.shared_capabilities = discovered.shared_capabilities.clone();
                attached.host_local_capabilities = discovered.host_local_capabilities.clone();
                attached.remote_admin_access = discovered.remote_admin_access.clone();
            }
        }
        self.mode = ProxyMode::Attached(attached);
        self.last_start_error = None;
        self.port_in_use_modal = None;
    }

    pub fn scan_local_proxies(
        &mut self,
        rt: &tokio::runtime::Runtime,
        ports: std::ops::RangeInclusive<u16>,
    ) -> anyhow::Result<()> {
        #[derive(Debug, serde::Deserialize)]
        struct RuntimeConfigStatus {
            loaded_at_ms: u64,
        }

        #[derive(Debug, serde::Deserialize)]
        struct AdminDiscovery {
            admin_base_url: String,
        }

        async fn get_json<T: serde::de::DeserializeOwned>(
            client: &Client,
            url: String,
            timeout: Duration,
        ) -> anyhow::Result<T> {
            Ok(send_admin_request(client.get(url).timeout(timeout))
                .await?
                .json::<T>()
                .await?)
        }

        async fn get_runtime_status(
            client: &Client,
            base: &str,
            timeout: Duration,
        ) -> anyhow::Result<RuntimeConfigStatus> {
            get_json::<RuntimeConfigStatus>(
                client,
                format!("{base}/__codex_helper/api/v1/runtime/status"),
                timeout,
            )
            .await
        }

        async fn scan_port(client: Client, port: u16) -> Option<DiscoveredProxy> {
            let base_url = local_proxy_base_url(port);
            let admin_base_url = local_admin_base_url_for_proxy_port(port);
            let timeout = Duration::from_millis(250);

            let caps = get_json::<ApiV1Capabilities>(
                &client,
                format!("{admin_base_url}/__codex_helper/api/v1/capabilities"),
                timeout,
            )
            .await;

            if let Ok(c) = caps {
                let runtime = get_runtime_status(&client, &admin_base_url, timeout)
                    .await
                    .ok();

                return Some(DiscoveredProxy {
                    port,
                    base_url,
                    admin_base_url,
                    api_version: Some(c.api_version),
                    service_name: Some(c.service_name),
                    endpoints: c.endpoints,
                    surface_capabilities: c.surface_capabilities,
                    runtime_loaded_at_ms: runtime.as_ref().map(|r| r.loaded_at_ms),
                    last_error: None,
                    shared_capabilities: c.shared_capabilities,
                    host_local_capabilities: c.host_local_capabilities,
                    remote_admin_access: c.remote_admin_access,
                });
            }

            let caps = get_json::<ApiV1Capabilities>(
                &client,
                format!("{base_url}/__codex_helper/api/v1/capabilities"),
                timeout,
            )
            .await;

            if let Ok(c) = caps {
                let runtime = get_runtime_status(&client, &base_url, timeout).await.ok();

                return Some(DiscoveredProxy {
                    port,
                    base_url: base_url.clone(),
                    admin_base_url: base_url,
                    api_version: Some(c.api_version),
                    service_name: Some(c.service_name),
                    endpoints: c.endpoints,
                    surface_capabilities: c.surface_capabilities,
                    runtime_loaded_at_ms: runtime.as_ref().map(|r| r.loaded_at_ms),
                    last_error: None,
                    shared_capabilities: c.shared_capabilities,
                    host_local_capabilities: c.host_local_capabilities,
                    remote_admin_access: c.remote_admin_access,
                });
            }

            let discovered_admin_base = get_json::<AdminDiscovery>(
                &client,
                format!("{base_url}/.well-known/codex-helper-admin"),
                timeout,
            )
            .await
            .ok()
            .map(|d| d.admin_base_url.trim_end_matches('/').to_string());

            if let Some(discovered_admin_base) = discovered_admin_base
                && discovered_admin_base != admin_base_url
                && discovered_admin_base != base_url
            {
                let caps = get_json::<ApiV1Capabilities>(
                    &client,
                    format!("{discovered_admin_base}/__codex_helper/api/v1/capabilities"),
                    timeout,
                )
                .await;

                if let Ok(c) = caps {
                    let runtime = get_runtime_status(&client, &discovered_admin_base, timeout)
                        .await
                        .ok();

                    return Some(DiscoveredProxy {
                        port,
                        base_url,
                        admin_base_url: discovered_admin_base,
                        api_version: Some(c.api_version),
                        service_name: Some(c.service_name),
                        endpoints: c.endpoints,
                        surface_capabilities: c.surface_capabilities,
                        runtime_loaded_at_ms: runtime.as_ref().map(|r| r.loaded_at_ms),
                        last_error: None,
                        shared_capabilities: c.shared_capabilities,
                        host_local_capabilities: c.host_local_capabilities,
                        remote_admin_access: c.remote_admin_access,
                    });
                }
            }

            None
        }

        let client = self.http_client.clone();
        let ports_vec = ports.collect::<Vec<_>>();
        let fut = async move {
            let tasks = ports_vec
                .into_iter()
                .map(|port| scan_port(client.clone(), port));
            let mut found = join_all(tasks)
                .await
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            found.sort_by_key(|p| p.port);
            Ok::<_, anyhow::Error>(found)
        };

        let found = rt.block_on(fut)?;
        self.discovered = found;
        self.last_discovery_scan = Some(Instant::now());
        Ok(())
    }

    pub fn detach(&mut self) {
        self.mode = ProxyMode::Stopped;
        self.last_start_error = None;
        self.port_in_use_modal = None;
    }

    pub fn refresh_attached_if_due(
        &mut self,
        rt: &tokio::runtime::Runtime,
        refresh_every: Duration,
    ) {
        let refresh_every = refresh_every.max(Duration::from_secs(1));
        let base_candidates = match &mut self.mode {
            ProxyMode::Attached(att) => {
                if let Some(t) = att.last_refresh
                    && t.elapsed() < refresh_every
                {
                    return;
                }
                att.last_refresh = Some(Instant::now());
                attached_management_candidates(att)
            }
            _ => return,
        };

        let client = self.http_client.clone();
        let fut = async move {
            #[derive(Debug, serde::Deserialize)]
            struct RuntimeConfigStatus {
                loaded_at_ms: u64,
                #[serde(default)]
                source_mtime_ms: Option<u64>,
                #[serde(default)]
                retry: Option<ResolvedRetryConfig>,
            }

            let req_timeout = Duration::from_millis(800);
            async fn get_json<T: serde::de::DeserializeOwned>(
                client: &Client,
                url: String,
                timeout: Duration,
            ) -> anyhow::Result<T> {
                Ok(send_admin_request(client.get(url).timeout(timeout))
                    .await?
                    .json::<T>()
                    .await?)
            }

            async fn get_v1_runtime_status(
                client: &Client,
                base: &str,
                req_timeout: Duration,
            ) -> anyhow::Result<RuntimeConfigStatus> {
                get_json::<RuntimeConfigStatus>(
                    client,
                    format!("{base}/__codex_helper/api/v1/runtime/status"),
                    req_timeout,
                )
                .await
            }

            async fn get_v1_global_station_override(
                client: &Client,
                base: &str,
                req_timeout: Duration,
            ) -> anyhow::Result<Option<String>> {
                get_json::<Option<String>>(
                    client,
                    format!("{base}/__codex_helper/api/v1/overrides/global-station"),
                    req_timeout,
                )
                .await
            }

            async fn get_v1_station_health(
                client: &Client,
                base: &str,
                req_timeout: Duration,
            ) -> anyhow::Result<HashMap<String, StationHealth>> {
                get_json::<HashMap<String, StationHealth>>(
                    client,
                    format!("{base}/__codex_helper/api/v1/status/station-health"),
                    req_timeout,
                )
                .await
            }

            #[derive(Default)]
            struct RefreshResult {
                management_base_url: String,
                api_version: Option<u32>,
                service_name: Option<String>,
                active: Vec<ActiveRequest>,
                recent: Vec<FinishedRequest>,
                session_cards: Vec<SessionIdentityCard>,
                global_station_override: Option<String>,
                configured_active_station: Option<String>,
                effective_active_station: Option<String>,
                configured_default_profile: Option<String>,
                default_profile: Option<String>,
                profiles: Vec<ControlProfileOption>,
                session_model: HashMap<String, String>,
                session_station: HashMap<String, String>,
                session_effort: HashMap<String, String>,
                session_service_tier: HashMap<String, String>,
                session_stats: HashMap<String, SessionStats>,
                stations: Vec<StationOption>,
                station_health: HashMap<String, StationHealth>,
                health_checks: HashMap<String, HealthCheckStatus>,
                usage_rollup: UsageRollupView,
                stats_5m: WindowStats,
                stats_1h: WindowStats,
                lb_view: HashMap<String, LbConfigView>,
                runtime_loaded_at_ms: Option<u64>,
                runtime_source_mtime_ms: Option<u64>,
                configured_retry: Option<RetryConfig>,
                resolved_retry: Option<ResolvedRetryConfig>,
                supports_retry_config_api: bool,
                persisted_providers: BTreeMap<String, PersistedProviderSpec>,
                supports_provider_spec_api: bool,
                persisted_stations: BTreeMap<String, PersistedStationSpec>,
                persisted_station_providers: BTreeMap<String, PersistedStationProviderRef>,
                supports_station_spec_api: bool,
                supports_persisted_station_config: bool,
                supports_default_profile_override: bool,
                supports_station_runtime_override: bool,
                supports_session_override_reset: bool,
                supports_control_trace_api: bool,
                supports_station_api: bool,
                shared_capabilities: SharedControlPlaneCapabilities,
                host_local_capabilities: HostLocalControlPlaneCapabilities,
                remote_admin_access: RemoteAdminAccessCapabilities,
            }

            #[derive(Default, serde::Deserialize)]
            struct AttachedSessionManualOverridesListResponse {
                #[serde(default)]
                sessions: HashMap<String, SessionManualOverrides>,
            }

            async fn refresh_from_base(
                client: &Client,
                base: &str,
                req_timeout: Duration,
            ) -> anyhow::Result<RefreshResult> {
                let caps = get_json::<ApiV1Capabilities>(
                    client,
                    format!("{base}/__codex_helper/api/v1/capabilities"),
                    req_timeout,
                )
                .await?;
                if caps.api_version != 1 {
                    bail!(
                        "attached proxy reported unsupported api version: {}",
                        caps.api_version
                    );
                }
                let ApiV1Capabilities {
                    api_version,
                    service_name,
                    endpoints,
                    surface_capabilities,
                    shared_capabilities,
                    host_local_capabilities,
                    remote_admin_access,
                } = caps;
                let resolved_surface =
                    resolve_api_v1_surface(&surface_capabilities, endpoints.as_slice());
                let supports_snapshot = resolved_surface.snapshot;
                let supports_profiles = resolved_surface.profiles;
                let supports_retry_config_api = resolved_surface.retry_config;
                let supports_provider_spec_api = resolved_surface.provider_specs;
                let supports_station_spec_api = resolved_surface.station_specs;
                let supports_default_profile_override = resolved_surface.default_profile_override;
                let supports_session_override_reset = resolved_surface.session_override_reset;
                let supports_control_trace_api = resolved_surface.control_trace;
                let supports_session_override_aggregate =
                    resolved_surface.session_override_aggregate;
                let supports_session_station = resolved_surface.session_station;
                let supports_session_effort = resolved_surface.session_reasoning_effort;
                let supports_persisted_station_config = resolved_surface.persisted_station_config;
                let supports_station_api = resolved_surface.station_api;
                let supports_station_runtime_override = resolved_surface.station_runtime;

                let configured_profiles = if supports_profiles {
                    #[derive(serde::Deserialize)]
                    struct ProfilesResponse {
                        default_profile: Option<String>,
                        #[serde(default)]
                        configured_default_profile: Option<String>,
                        #[serde(default)]
                        profiles: Vec<ControlProfileOption>,
                    }

                    get_json::<ProfilesResponse>(
                        client,
                        format!("{base}/__codex_helper/api/v1/profiles"),
                        req_timeout,
                    )
                    .await
                    .ok()
                } else {
                    None
                };
                let configured_retry = if supports_retry_config_api {
                    #[derive(serde::Deserialize)]
                    struct RetryConfigResponse {
                        configured: RetryConfig,
                        resolved: ResolvedRetryConfig,
                    }

                    get_json::<RetryConfigResponse>(
                        client,
                        format!("{base}/__codex_helper/api/v1/retry/config"),
                        req_timeout,
                    )
                    .await
                    .ok()
                    .map(|response| (response.configured, response.resolved))
                } else {
                    None
                };
                let persisted_station_catalog = if supports_station_spec_api {
                    get_json::<crate::config::PersistedStationsCatalog>(
                        client,
                        format!("{base}/__codex_helper/api/v1/stations/specs"),
                        req_timeout,
                    )
                    .await
                    .ok()
                } else {
                    None
                };
                let persisted_provider_catalog = if supports_provider_spec_api {
                    get_json::<crate::config::PersistedProvidersCatalog>(
                        client,
                        format!("{base}/__codex_helper/api/v1/providers/specs"),
                        req_timeout,
                    )
                    .await
                    .ok()
                } else {
                    None
                };

                if supports_snapshot {
                    let api = get_json::<ApiV1Snapshot>(
                        client,
                        format!(
                            "{base}/__codex_helper/api/v1/snapshot?recent_limit=600&stats_days=21"
                        ),
                        req_timeout,
                    )
                    .await?;
                    let ApiV1Snapshot {
                        api_version,
                        service_name,
                        runtime_loaded_at_ms,
                        runtime_source_mtime_ms,
                        stations,
                        configured_active_station,
                        effective_active_station,
                        default_profile,
                        profiles,
                        snapshot,
                    } = api;
                    let configured_default_profile = configured_profiles
                        .as_ref()
                        .and_then(|response| response.configured_default_profile.clone())
                        .or_else(|| {
                            configured_profiles
                                .as_ref()
                                .and_then(|response| response.default_profile.clone())
                        });
                    let profiles = configured_profiles
                        .as_ref()
                        .map(|response| response.profiles.clone())
                        .unwrap_or(profiles);
                    let global_station_override = snapshot
                        .effective_global_station_override()
                        .map(str::to_owned);
                    let station_health = snapshot.effective_station_health().clone();

                    return Ok(RefreshResult {
                        management_base_url: base.to_string(),
                        api_version: Some(api_version),
                        service_name: Some(service_name),
                        active: snapshot.active,
                        recent: snapshot.recent,
                        session_cards: snapshot.session_cards,
                        global_station_override,
                        configured_active_station,
                        effective_active_station,
                        configured_default_profile,
                        default_profile,
                        profiles,
                        session_model: snapshot.session_model_overrides,
                        session_station: snapshot.session_station_overrides,
                        session_effort: snapshot.session_effort_overrides,
                        session_service_tier: snapshot.session_service_tier_overrides,
                        session_stats: snapshot.session_stats,
                        stations,
                        station_health,
                        health_checks: snapshot.health_checks,
                        usage_rollup: snapshot.usage_rollup,
                        stats_5m: snapshot.stats_5m,
                        stats_1h: snapshot.stats_1h,
                        lb_view: snapshot.lb_view,
                        runtime_loaded_at_ms,
                        runtime_source_mtime_ms,
                        configured_retry: configured_retry
                            .as_ref()
                            .map(|(configured, _)| configured.clone()),
                        resolved_retry: configured_retry
                            .as_ref()
                            .map(|(_, resolved)| resolved.clone()),
                        supports_retry_config_api,
                        persisted_providers: persisted_provider_catalog
                            .as_ref()
                            .map(|catalog| {
                                catalog
                                    .providers
                                    .iter()
                                    .cloned()
                                    .map(|provider| (provider.name.clone(), provider))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        supports_provider_spec_api,
                        persisted_stations: persisted_station_catalog
                            .as_ref()
                            .map(|catalog| {
                                catalog
                                    .stations
                                    .iter()
                                    .cloned()
                                    .map(|station| (station.name.clone(), station))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        persisted_station_providers: persisted_station_catalog
                            .as_ref()
                            .map(|catalog| {
                                catalog
                                    .providers
                                    .iter()
                                    .cloned()
                                    .map(|provider| (provider.name.clone(), provider))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        supports_station_spec_api,
                        supports_persisted_station_config,
                        supports_default_profile_override,
                        supports_station_runtime_override,
                        supports_session_override_reset,
                        supports_control_trace_api,
                        supports_station_api,
                        shared_capabilities,
                        host_local_capabilities,
                        remote_admin_access,
                    });
                }

                let (
                    active,
                    recent,
                    runtime,
                    global_station_override,
                    stats,
                    stations,
                    station_health,
                    health_checks,
                ) = tokio::try_join!(
                    get_json::<Vec<ActiveRequest>>(
                        client,
                        format!("{base}/__codex_helper/api/v1/status/active"),
                        req_timeout,
                    ),
                    get_json::<Vec<FinishedRequest>>(
                        client,
                        format!("{base}/__codex_helper/api/v1/status/recent?limit=200"),
                        req_timeout,
                    ),
                    get_v1_runtime_status(client, base, req_timeout),
                    get_v1_global_station_override(client, base, req_timeout),
                    get_json::<HashMap<String, SessionStats>>(
                        client,
                        format!("{base}/__codex_helper/api/v1/status/session-stats"),
                        req_timeout,
                    ),
                    get_json::<Vec<StationOption>>(
                        client,
                        format!("{base}/__codex_helper/api/v1/stations"),
                        req_timeout,
                    ),
                    get_v1_station_health(client, base, req_timeout),
                    get_json::<HashMap<String, HealthCheckStatus>>(
                        client,
                        format!("{base}/__codex_helper/api/v1/status/health-checks"),
                        req_timeout,
                    ),
                )?;

                let supports_session_model = resolved_surface.session_model;
                let supports_session_service_tier = resolved_surface.session_service_tier;
                let (session_station, session_effort, session_model, session_service_tier) =
                    if supports_session_override_aggregate {
                        let aggregate = get_json::<AttachedSessionManualOverridesListResponse>(
                            client,
                            format!("{base}/__codex_helper/api/v1/overrides/session"),
                            req_timeout,
                        )
                        .await
                        .ok()
                        .unwrap_or_default();
                        let mut session_station = HashMap::new();
                        let mut session_effort = HashMap::new();
                        let mut session_model = HashMap::new();
                        let mut session_service_tier = HashMap::new();
                        for (session_id, overrides) in aggregate.sessions {
                            if let Some(station_name) = overrides.station_name {
                                session_station.insert(session_id.clone(), station_name);
                            }
                            if let Some(reasoning_effort) = overrides.reasoning_effort {
                                session_effort.insert(session_id.clone(), reasoning_effort);
                            }
                            if let Some(model) = overrides.model {
                                session_model.insert(session_id.clone(), model);
                            }
                            if let Some(service_tier) = overrides.service_tier {
                                session_service_tier.insert(session_id, service_tier);
                            }
                        }
                        (
                            session_station,
                            session_effort,
                            session_model,
                            session_service_tier,
                        )
                    } else {
                        let session_station = if supports_session_station {
                            get_json::<HashMap<String, String>>(
                                client,
                                format!("{base}/__codex_helper/api/v1/overrides/session/station"),
                                req_timeout,
                            )
                            .await
                            .ok()
                            .unwrap_or_default()
                        } else {
                            HashMap::new()
                        };
                        let session_effort = if supports_session_effort {
                            get_json::<HashMap<String, String>>(
                                client,
                                format!("{base}/__codex_helper/api/v1/overrides/session/effort"),
                                req_timeout,
                            )
                            .await
                            .ok()
                            .unwrap_or_default()
                        } else {
                            HashMap::new()
                        };
                        let session_model = if supports_session_model {
                            get_json::<HashMap<String, String>>(
                                client,
                                format!("{base}/__codex_helper/api/v1/overrides/session/model"),
                                req_timeout,
                            )
                            .await
                            .ok()
                            .unwrap_or_default()
                        } else {
                            HashMap::new()
                        };
                        let session_service_tier = if supports_session_service_tier {
                            get_json::<HashMap<String, String>>(
                                client,
                                format!(
                                    "{base}/__codex_helper/api/v1/overrides/session/service-tier"
                                ),
                                req_timeout,
                            )
                            .await
                            .ok()
                            .unwrap_or_default()
                        } else {
                            HashMap::new()
                        };
                        (
                            session_station,
                            session_effort,
                            session_model,
                            session_service_tier,
                        )
                    };

                let (configured_default_profile, default_profile, profiles) =
                    match configured_profiles {
                        Some(response) => (
                            response
                                .configured_default_profile
                                .clone()
                                .or_else(|| response.default_profile.clone()),
                            response.default_profile,
                            response.profiles,
                        ),
                        None => (None, None, Vec::new()),
                    };

                return Ok(RefreshResult {
                    management_base_url: base.to_string(),
                    api_version: Some(api_version),
                    service_name: Some(service_name),
                    active,
                    recent,
                    session_cards: Vec::new(),
                    global_station_override,
                    configured_active_station: None,
                    effective_active_station: None,
                    configured_default_profile,
                    default_profile,
                    profiles,
                    session_model,
                    session_station,
                    session_effort,
                    session_service_tier,
                    session_stats: stats,
                    stations,
                    station_health,
                    health_checks,
                    usage_rollup: UsageRollupView::default(),
                    stats_5m: WindowStats::default(),
                    stats_1h: WindowStats::default(),
                    lb_view: HashMap::new(),
                    runtime_loaded_at_ms: Some(runtime.loaded_at_ms),
                    runtime_source_mtime_ms: runtime.source_mtime_ms,
                    configured_retry: configured_retry
                        .as_ref()
                        .map(|(configured, _)| configured.clone()),
                    resolved_retry: configured_retry
                        .as_ref()
                        .map(|(_, resolved)| resolved.clone())
                        .or(runtime.retry),
                    supports_retry_config_api,
                    persisted_providers: persisted_provider_catalog
                        .as_ref()
                        .map(|catalog| {
                            catalog
                                .providers
                                .iter()
                                .cloned()
                                .map(|provider| (provider.name.clone(), provider))
                                .collect()
                        })
                        .unwrap_or_default(),
                    supports_provider_spec_api,
                    persisted_stations: persisted_station_catalog
                        .as_ref()
                        .map(|catalog| {
                            catalog
                                .stations
                                .iter()
                                .cloned()
                                .map(|station| (station.name.clone(), station))
                                .collect()
                        })
                        .unwrap_or_default(),
                    persisted_station_providers: persisted_station_catalog
                        .as_ref()
                        .map(|catalog| {
                            catalog
                                .providers
                                .iter()
                                .cloned()
                                .map(|provider| (provider.name.clone(), provider))
                                .collect()
                        })
                        .unwrap_or_default(),
                    supports_station_spec_api,
                    supports_persisted_station_config,
                    supports_default_profile_override,
                    supports_station_runtime_override,
                    supports_session_override_reset,
                    supports_control_trace_api,
                    supports_station_api,
                    shared_capabilities,
                    host_local_capabilities,
                    remote_admin_access,
                });
            }

            let mut last_err: Option<anyhow::Error> = None;
            for base in base_candidates {
                match refresh_from_base(&client, &base, req_timeout).await {
                    Ok(result) => return Ok::<_, anyhow::Error>(result),
                    Err(err) => last_err = Some(err),
                }
            }

            Err(last_err.unwrap_or_else(|| anyhow::anyhow!("attach refresh failed")))
        };

        match rt.block_on(fut) {
            Ok(result) => {
                if let ProxyMode::Attached(att) = &mut self.mode {
                    att.last_error = None;
                    att.admin_base_url = result.management_base_url;
                    att.api_version = result.api_version;
                    att.service_name = result.service_name;
                    att.active = result.active;
                    att.recent = result.recent;
                    att.session_cards = result.session_cards;
                    att.global_station_override = result.global_station_override;
                    att.configured_active_station = result.configured_active_station;
                    att.effective_active_station = result.effective_active_station;
                    att.configured_default_profile = result.configured_default_profile;
                    att.default_profile = result.default_profile;
                    att.profiles = result.profiles;
                    att.session_model_overrides = result.session_model;
                    att.session_station_overrides = result.session_station;
                    att.session_effort_overrides = result.session_effort;
                    att.session_service_tier_overrides = result.session_service_tier;
                    att.session_stats = result.session_stats;
                    att.stations = result.stations;
                    att.station_health = result.station_health;
                    att.health_checks = result.health_checks;
                    att.usage_rollup = result.usage_rollup;
                    att.stats_5m = result.stats_5m;
                    att.stats_1h = result.stats_1h;
                    att.lb_view = result.lb_view;
                    att.runtime_loaded_at_ms = result.runtime_loaded_at_ms;
                    att.runtime_source_mtime_ms = result.runtime_source_mtime_ms;
                    att.configured_retry = result.configured_retry;
                    att.resolved_retry = result.resolved_retry;
                    att.supports_retry_config_api = result.supports_retry_config_api;
                    att.persisted_providers = result.persisted_providers;
                    att.supports_provider_spec_api = result.supports_provider_spec_api;
                    att.persisted_stations = result.persisted_stations;
                    att.persisted_station_providers = result.persisted_station_providers;
                    att.supports_station_spec_api = result.supports_station_spec_api;
                    att.supports_persisted_station_config =
                        result.supports_persisted_station_config;
                    att.supports_default_profile_override =
                        result.supports_default_profile_override;
                    att.supports_station_runtime_override =
                        result.supports_station_runtime_override;
                    att.supports_session_override_reset = result.supports_session_override_reset;
                    att.supports_control_trace_api = result.supports_control_trace_api;
                    att.supports_station_api = result.supports_station_api;
                    att.shared_capabilities = result.shared_capabilities;
                    att.host_local_capabilities = result.host_local_capabilities;
                    att.remote_admin_access = result.remote_admin_access;
                }
            }
            Err(e) => {
                if let ProxyMode::Attached(att) = &mut self.mode {
                    att.last_error = Some(e.to_string());
                }
            }
        }
    }

    pub fn apply_session_effort_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        effort: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match effort {
                        Some(eff) => {
                            state
                                .set_session_effort_override(session_id, eff, now)
                                .await
                        }
                        None => state.clear_session_effort_override(&session_id).await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support session effort overrides (need api v1)");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    let payload = serde_json::json!({
                        "session_id": session_id,
                        "effort": effort,
                    });
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/effort"
                            ))
                            .timeout(Duration::from_millis(800))
                            .json(&payload),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_session_model_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        model: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match model {
                        Some(model) => {
                            state
                                .set_session_model_override(session_id, model, now)
                                .await
                        }
                        None => state.clear_session_model_override(&session_id).await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support session model overrides (need api v1)");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/model"
                            ))
                            .timeout(Duration::from_millis(800))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                                "model": model,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_session_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        profile_name: String,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let service_name = r.service_name;
                let cfg = r.cfg.clone();
                let now = now_ms();
                rt.block_on(async move {
                    let mgr = match service_name {
                        "claude" => &cfg.claude,
                        _ => &cfg.codex,
                    };
                    state
                        .apply_session_profile_binding(
                            service_name,
                            mgr,
                            session_id,
                            profile_name,
                            now,
                        )
                        .await
                })?;
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support session profile apply (need api v1)");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/profile"
                            ))
                            .timeout(Duration::from_millis(1200))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                                "profile_name": profile_name,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn clear_session_profile_binding(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                rt.block_on(async move {
                    state.clear_session_binding(session_id.as_str()).await;
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!(
                        "attached proxy does not support session profile binding clear (need api v1)"
                    );
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/profile"
                            ))
                            .timeout(Duration::from_millis(1200))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                                "profile_name": serde_json::Value::Null,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn clear_session_manual_overrides(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                rt.block_on(async move {
                    state
                        .clear_session_manual_overrides(session_id.as_str())
                        .await;
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_session_override_reset {
                    bail!(
                        "attached proxy does not support session manual override reset (need api v1)"
                    );
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/reset"
                            ))
                            .timeout(Duration::from_millis(1200))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn set_default_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let service_name = r.service_name;
                let cfg = r.cfg.clone();
                let now = now_ms();
                let effective_default = rt.block_on(async move {
                    let mgr = match service_name {
                        "claude" => &cfg.claude,
                        _ => &cfg.codex,
                    };
                    match profile_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                    {
                        Some(name) => {
                            if mgr.profile(name).is_none() {
                                bail!("profile not found: {name}");
                            }
                            state
                                .set_runtime_default_profile_override(
                                    service_name.to_string(),
                                    name.to_string(),
                                    now,
                                )
                                .await;
                        }
                        None => {
                            state
                                .clear_runtime_default_profile_override(service_name)
                                .await;
                        }
                    }

                    Ok::<_, anyhow::Error>(
                        effective_default_profile_from_cfg_state(
                            state.as_ref(),
                            service_name,
                            cfg.as_ref(),
                        )
                        .await,
                    )
                })?;
                r.default_profile = effective_default.clone();
                r.profiles = list_profiles_from_cfg(
                    r.cfg.as_ref(),
                    r.service_name,
                    effective_default.as_deref(),
                );
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if !att.supports_default_profile_override {
                    bail!("attached proxy does not support runtime default profile switch");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!("{base}/__codex_helper/api/v1/profiles/default"))
                            .timeout(Duration::from_millis(1200))
                            .json(&serde_json::json!({
                            "profile_name": profile_name,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    #[allow(dead_code)]
    pub fn set_persisted_default_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: Option<String>,
    ) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support persisted profile config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!(
                        "{base}/__codex_helper/api/v1/profiles/default/persisted"
                    ))
                    .timeout(Duration::from_millis(1200))
                    .json(&serde_json::json!({
                        "profile_name": profile_name,
                    })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn upsert_persisted_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: String,
        profile: crate::config::ServiceControlProfile,
    ) -> anyhow::Result<()> {
        if profile_name.trim().is_empty() {
            bail!("profile name is required");
        }

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support persisted profile config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .put(format!(
                        "{base}/__codex_helper/api/v1/profiles/{}",
                        profile_name.trim()
                    ))
                    .timeout(Duration::from_millis(1200))
                    .json(&profile),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_persisted_profile(
        &mut self,
        rt: &tokio::runtime::Runtime,
        profile_name: String,
    ) -> anyhow::Result<()> {
        if profile_name.trim().is_empty() {
            bail!("profile name is required");
        }

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support persisted profile config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .delete(format!(
                        "{base}/__codex_helper/api/v1/profiles/{}",
                        profile_name.trim()
                    ))
                    .timeout(Duration::from_millis(1200)),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn set_persisted_active_station(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: Option<String>,
    ) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_persisted_station_config {
                    bail!("attached proxy does not support persisted station config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!(
                        "{base}/__codex_helper/api/v1/stations/config-active"
                    ))
                    .timeout(Duration::from_millis(1200))
                    .json(&serde_json::json!({
                        "station_name": station_name,
                    })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn set_persisted_retry_config(
        &mut self,
        rt: &tokio::runtime::Runtime,
        retry: RetryConfig,
    ) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_retry_config_api {
                    bail!("attached proxy does not support persisted retry config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        #[derive(serde::Deserialize)]
        struct RetryConfigResponse {
            configured: RetryConfig,
            resolved: ResolvedRetryConfig,
        }

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!("{base}/__codex_helper/api/v1/retry/config"))
                    .timeout(Duration::from_millis(1200))
                    .json(&retry),
            )
            .await?
            .json::<RetryConfigResponse>()
            .await
            .map_err(anyhow::Error::from)
        };
        let response = rt.block_on(fut)?;

        match &mut self.mode {
            ProxyMode::Running(r) => {
                r.configured_retry = Some(response.configured.clone());
                r.resolved_retry = Some(response.resolved.clone());
            }
            ProxyMode::Attached(att) => {
                att.configured_retry = Some(response.configured.clone());
                att.resolved_retry = Some(response.resolved.clone());
                att.supports_retry_config_api = true;
            }
            _ => {}
        }

        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn update_persisted_station(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: String,
        enabled: bool,
        level: u8,
    ) -> anyhow::Result<()> {
        if station_name.trim().is_empty() {
            bail!("station name is required");
        }

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_persisted_station_config {
                    bail!("attached proxy does not support persisted station config (need api v1)");
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .put(format!(
                        "{base}/__codex_helper/api/v1/stations/{}",
                        station_name.trim()
                    ))
                    .timeout(Duration::from_millis(1200))
                    .json(&serde_json::json!({
                        "enabled": enabled,
                        "level": level,
                    })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn upsert_persisted_station_spec(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: String,
        station: PersistedStationSpec,
    ) -> anyhow::Result<()> {
        if station_name.trim().is_empty() {
            bail!("station name is required");
        }

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_station_spec_api {
                    bail!(
                        "attached proxy does not support persisted station spec API (need api v1)"
                    );
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .put(format!(
                        "{base}/__codex_helper/api/v1/stations/specs/{}",
                        station_name.trim()
                    ))
                    .timeout(Duration::from_millis(1500))
                    .json(&serde_json::json!({
                        "alias": station.alias,
                        "enabled": station.enabled,
                        "level": station.level,
                        "members": station.members,
                    })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_persisted_station_spec(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: String,
    ) -> anyhow::Result<()> {
        if station_name.trim().is_empty() {
            bail!("station name is required");
        }

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_station_spec_api {
                    bail!(
                        "attached proxy does not support persisted station spec API (need api v1)"
                    );
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .delete(format!(
                        "{base}/__codex_helper/api/v1/stations/specs/{}",
                        station_name.trim()
                    ))
                    .timeout(Duration::from_millis(1500)),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn upsert_persisted_provider_spec(
        &mut self,
        rt: &tokio::runtime::Runtime,
        provider_name: String,
        provider: PersistedProviderSpec,
    ) -> anyhow::Result<()> {
        if provider_name.trim().is_empty() {
            bail!("provider name is required");
        }

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_provider_spec_api {
                    bail!(
                        "attached proxy does not support persisted provider spec API (need api v1)"
                    );
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .put(format!(
                        "{base}/__codex_helper/api/v1/providers/specs/{}",
                        provider_name.trim()
                    ))
                    .timeout(Duration::from_millis(1500))
                    .json(&serde_json::json!({
                        "alias": provider.alias,
                        "enabled": provider.enabled,
                        "auth_token_env": provider.auth_token_env,
                        "api_key_env": provider.api_key_env,
                        "endpoints": provider.endpoints,
                    })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_persisted_provider_spec(
        &mut self,
        rt: &tokio::runtime::Runtime,
        provider_name: String,
    ) -> anyhow::Result<()> {
        if provider_name.trim().is_empty() {
            bail!("provider name is required");
        }

        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) || !att.supports_provider_spec_api {
                    bail!(
                        "attached proxy does not support persisted provider spec API (need api v1)"
                    );
                }
                att.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .delete(format!(
                        "{base}/__codex_helper/api/v1/providers/specs/{}",
                        provider_name.trim()
                    ))
                    .timeout(Duration::from_millis(1500)),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    pub fn set_runtime_station_meta(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: String,
        enabled: Option<Option<bool>>,
        level: Option<Option<u8>>,
        runtime_state: Option<Option<RuntimeConfigState>>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let service_name = r.service_name;
                let cfg = r.cfg.clone();
                let now = now_ms();
                let stations = rt.block_on(async move {
                    let mgr = match service_name {
                        "claude" => &cfg.claude,
                        _ => &cfg.codex,
                    };
                    if !mgr.contains_station(station_name.as_str()) {
                        bail!("station not found: {station_name}");
                    }

                    if let Some(enabled) = enabled {
                        match enabled {
                            Some(enabled) => {
                                state
                                    .set_station_enabled_override(
                                        service_name,
                                        station_name.clone(),
                                        enabled,
                                        now,
                                    )
                                    .await;
                            }
                            None => {
                                state
                                    .clear_station_enabled_override(
                                        service_name,
                                        station_name.as_str(),
                                    )
                                    .await;
                            }
                        }
                    }

                    if let Some(level) = level {
                        match level {
                            Some(level) => {
                                state
                                    .set_station_level_override(
                                        service_name,
                                        station_name.clone(),
                                        level.clamp(1, 10),
                                        now,
                                    )
                                    .await;
                            }
                            None => {
                                state
                                    .clear_station_level_override(
                                        service_name,
                                        station_name.as_str(),
                                    )
                                    .await;
                            }
                        }
                    }

                    if let Some(runtime_state) = runtime_state {
                        match runtime_state {
                            Some(runtime_state) => {
                                state
                                    .set_station_runtime_state_override(
                                        service_name,
                                        station_name.clone(),
                                        runtime_state,
                                        now,
                                    )
                                    .await;
                            }
                            None => {
                                state
                                    .clear_station_runtime_state_override(
                                        service_name,
                                        station_name.as_str(),
                                    )
                                    .await;
                            }
                        }
                    }

                    Ok::<_, anyhow::Error>(
                        effective_stations_from_cfg_state(
                            state.as_ref(),
                            service_name,
                            cfg.as_ref(),
                        )
                        .await,
                    )
                })?;
                r.stations = stations;
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if !att.supports_station_runtime_override {
                    bail!("attached proxy does not support runtime station meta control");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    let clear_enabled = matches!(enabled, Some(None));
                    let clear_level = matches!(level, Some(None));
                    let clear_runtime_state = matches!(runtime_state, Some(None));
                    let mut body = serde_json::Map::new();
                    body.insert(
                        "station_name".to_string(),
                        serde_json::Value::String(station_name),
                    );
                    body.insert("enabled".to_string(), serde_json::json!(enabled.flatten()));
                    body.insert("level".to_string(), serde_json::json!(level.flatten()));
                    body.insert(
                        "clear_enabled".to_string(),
                        serde_json::json!(clear_enabled),
                    );
                    body.insert("clear_level".to_string(), serde_json::json!(clear_level));
                    body.insert(
                        "runtime_state".to_string(),
                        serde_json::json!(runtime_state.flatten()),
                    );
                    body.insert(
                        "clear_runtime_state".to_string(),
                        serde_json::json!(clear_runtime_state),
                    );
                    send_admin_request(
                        client
                            .post(format!("{base}/__codex_helper/api/v1/stations/runtime"))
                            .timeout(Duration::from_millis(1200))
                            .json(&serde_json::Value::Object(body)),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_session_station_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        station_name: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match station_name {
                        Some(name) => {
                            state
                                .set_session_station_override(session_id, name, now)
                                .await
                        }
                        None => state.clear_session_station_override(&session_id).await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!(
                        "attached proxy does not support session station overrides (need api v1)"
                    );
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/station"
                            ))
                            .timeout(Duration::from_millis(800))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                                "station_name": station_name,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_session_service_tier_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        service_tier: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match service_tier {
                        Some(service_tier) => {
                            state
                                .set_session_service_tier_override(session_id, service_tier, now)
                                .await
                        }
                        None => state.clear_session_service_tier_override(&session_id).await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!(
                        "attached proxy does not support session service tier overrides (need api v1)"
                    );
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/session/service-tier"
                            ))
                            .timeout(Duration::from_millis(800))
                            .json(&serde_json::json!({
                                "session_id": session_id,
                                "service_tier": service_tier,
                            })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_global_station_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match station_name {
                        Some(name) => state.set_global_station_override(name, now).await,
                        None => state.clear_global_station_override().await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support global station override (need api v1)");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    send_admin_request(
                        client
                            .post(format!(
                                "{base}/__codex_helper/api/v1/overrides/global-station"
                            ))
                            .timeout(Duration::from_millis(800))
                            .json(&serde_json::json!({ "station_name": station_name })),
                    )
                    .await?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn reload_runtime_config(&mut self, rt: &tokio::runtime::Runtime) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support runtime reload (need api v1)");
                }
                a.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            let url = format!("{base}/__codex_helper/api/v1/runtime/reload");
            send_admin_request(client.post(url).timeout(Duration::from_millis(800))).await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    pub fn start_health_checks(
        &mut self,
        rt: &tokio::runtime::Runtime,
        all: bool,
        station_names: Vec<String>,
    ) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support health checks (need api v1)");
                }
                a.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!("{base}/__codex_helper/api/v1/healthcheck/start"))
                    .timeout(Duration::from_millis(800))
                    .json(&serde_json::json!({ "all": all, "station_names": station_names })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    pub fn probe_station(
        &mut self,
        rt: &tokio::runtime::Runtime,
        station_name: String,
    ) -> anyhow::Result<()> {
        let station_name = station_name.trim().to_string();
        if station_name.is_empty() {
            bail!("station_name cannot be empty");
        }

        let (base, use_station_api) = match &self.mode {
            ProxyMode::Running(r) => (local_proxy_base_url(r.admin_port), true),
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support manual probes (need api v1)");
                }
                (a.admin_base_url.clone(), a.supports_station_api)
            }
            _ => bail!("proxy is not running/attached"),
        };

        if !use_station_api {
            return self.start_health_checks(rt, false, vec![station_name]);
        }

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!("{base}/__codex_helper/api/v1/stations/probe"))
                    .timeout(Duration::from_millis(800))
                    .json(&serde_json::json!({ "station_name": station_name })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    pub fn cancel_health_checks(
        &mut self,
        rt: &tokio::runtime::Runtime,
        all: bool,
        station_names: Vec<String>,
    ) -> anyhow::Result<()> {
        let base = match &self.mode {
            ProxyMode::Running(r) => local_proxy_base_url(r.admin_port),
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support health checks (need api v1)");
                }
                a.admin_base_url.clone()
            }
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            send_admin_request(
                client
                    .post(format!("{base}/__codex_helper/api/v1/healthcheck/cancel"))
                    .timeout(Duration::from_millis(800))
                    .json(&serde_json::json!({ "all": all, "station_names": station_names })),
            )
            .await?;
            Ok::<(), anyhow::Error>(())
        };
        rt.block_on(fut)?;
        self.refresh_current_if_due(rt, Duration::from_secs(0));
        Ok(())
    }

    pub fn refresh_running_if_due(
        &mut self,
        rt: &tokio::runtime::Runtime,
        refresh_every: Duration,
    ) {
        let ProxyMode::Running(r) = &mut self.mode else {
            return;
        };
        if let Some(t) = r.last_refresh
            && t.elapsed() < refresh_every
        {
            return;
        }
        r.last_refresh = Some(Instant::now());

        let state = r.state.clone();
        let service_name = r.service_name.to_string();
        let cfg = r.cfg.clone();
        let fut = async move {
            let mut snapshot =
                build_dashboard_snapshot(&state, service_name.as_str(), 600, 21).await;
            let mgr = match service_name.as_str() {
                "claude" => &cfg.claude,
                _ => &cfg.codex,
            };
            crate::state::enrich_session_identity_cards_with_runtime(
                &mut snapshot.session_cards,
                mgr,
            );
            let configured_active_station = mgr.active.clone();
            let effective_active_station = mgr.active_station().map(|cfg| cfg.name.clone());
            let configured_default_profile = mgr.default_profile.clone();
            let default_profile = effective_default_profile_from_cfg_state(
                state.as_ref(),
                service_name.as_str(),
                cfg.as_ref(),
            )
            .await;
            let profiles = list_profiles_from_cfg(
                cfg.as_ref(),
                service_name.as_str(),
                default_profile.as_deref(),
            );
            let stations = effective_stations_from_cfg_state(
                state.as_ref(),
                service_name.as_str(),
                cfg.as_ref(),
            )
            .await;
            Ok::<_, anyhow::Error>((
                snapshot,
                configured_active_station,
                effective_active_station,
                configured_default_profile,
                default_profile,
                profiles,
                stations,
            ))
        };

        match rt.block_on(fut) {
            Ok((
                snap,
                configured_active_station,
                effective_active_station,
                configured_default_profile,
                default_profile,
                profiles,
                stations,
            )) => {
                let global_station_override =
                    snap.effective_global_station_override().map(str::to_owned);
                let station_health = snap.effective_station_health().clone();
                r.last_error = None;
                r.configured_active_station = configured_active_station;
                r.effective_active_station = effective_active_station;
                r.configured_default_profile = configured_default_profile;
                r.default_profile = default_profile;
                r.profiles = profiles;
                r.stations = stations;
                r.active = snap.active;
                r.recent = snap.recent;
                r.session_cards = snap.session_cards;
                r.global_station_override = global_station_override;
                r.session_model_overrides = snap.session_model_overrides;
                r.session_station_overrides = snap.session_station_overrides;
                r.session_effort_overrides = snap.session_effort_overrides;
                r.session_service_tier_overrides = snap.session_service_tier_overrides;
                r.session_stats = snap.session_stats;
                r.station_health = station_health;
                r.health_checks = snap.health_checks;
                r.usage_rollup = snap.usage_rollup;
                r.stats_5m = snap.stats_5m;
                r.stats_1h = snap.stats_1h;
                r.lb_view = snap.lb_view;
            }
            Err(e) => {
                r.last_error = Some(e.to_string());
            }
        }
    }

    pub fn request_start_or_prompt(
        &mut self,
        rt: &tokio::runtime::Runtime,
        port_in_use_action: PortInUseAction,
        remember_choice: bool,
    ) {
        self.last_start_error = None;

        let port = self.desired_port;
        let service = self.desired_service;
        match self.try_start(rt, service, port) {
            Ok(()) => {}
            Err(e) => {
                if is_addr_in_use(&e) {
                    let action = if remember_choice {
                        port_in_use_action
                    } else {
                        PortInUseAction::Ask
                    };
                    match action {
                        PortInUseAction::Attach => {
                            self.request_attach(port);
                        }
                        PortInUseAction::StartNewPort => {
                            let suggested = suggest_next_port(rt, service, port).unwrap_or(port);
                            self.desired_port = suggested;
                            let _ = self.try_start(rt, service, suggested).map_err(|e| {
                                self.last_start_error = Some(e.to_string());
                            });
                        }
                        PortInUseAction::Exit => {
                            self.last_start_error =
                                Some("port already in use; configured action is exit".to_string());
                        }
                        PortInUseAction::Ask => {
                            self.port_in_use_modal = Some(PortInUseModal {
                                port,
                                remember_choice: false,
                                chosen_new_port: suggest_next_port(rt, service, port)
                                    .unwrap_or(port.saturating_add(1)),
                            });
                        }
                    }
                } else {
                    self.last_start_error = Some(e.to_string());
                }
            }
        }
    }

    pub fn confirm_port_in_use_attach(&mut self) {
        let Some(m) = self.port_in_use_modal.as_ref() else {
            return;
        };
        self.request_attach(m.port);
    }

    pub fn confirm_port_in_use_new_port(&mut self, rt: &tokio::runtime::Runtime) {
        let Some(m) = self.port_in_use_modal.as_ref() else {
            return;
        };
        let port = m.chosen_new_port;
        self.desired_port = port;
        self.port_in_use_modal = None;
        if let Err(e) = self.try_start(rt, self.desired_service, port) {
            self.last_start_error = Some(e.to_string());
        }
    }

    pub fn confirm_port_in_use_exit(&mut self) {
        self.port_in_use_modal = None;
        self.last_start_error = Some("port already in use; user chose exit".to_string());
        self.mode = ProxyMode::Stopped;
    }

    pub fn set_port_in_use_modal_remember(&mut self, v: bool) {
        if let Some(m) = self.port_in_use_modal.as_mut() {
            m.remember_choice = v;
        }
    }

    pub fn port_in_use_modal_remember(&self) -> bool {
        self.port_in_use_modal
            .as_ref()
            .map(|m| m.remember_choice)
            .unwrap_or(false)
    }

    pub fn set_port_in_use_modal_new_port(&mut self, port: u16) {
        if let Some(m) = self.port_in_use_modal.as_mut() {
            m.chosen_new_port = port;
        }
    }

    pub fn port_in_use_modal_suggested_port(&self) -> Option<u16> {
        self.port_in_use_modal.as_ref().map(|m| m.chosen_new_port)
    }

    fn try_start(
        &mut self,
        rt: &tokio::runtime::Runtime,
        service: ServiceKind,
        port: u16,
    ) -> anyhow::Result<()> {
        self.mode = ProxyMode::Starting;

        let service_name: &'static str = match service {
            ServiceKind::Codex => "codex",
            ServiceKind::Claude => "claude",
        };

        let task = async move {
            let cfg = Arc::new(load_or_bootstrap_for_service(service).await?);
            let admin_port = admin_port_for_proxy_port(port);

            if service_name == "codex" {
                if !cfg.codex.has_stations() || cfg.codex.active_station().is_none() {
                    anyhow::bail!(
                        "No valid Codex upstream config; please configure ~/.codex-helper/config.toml (or config.json) first"
                    );
                }
            } else if !cfg.claude.has_stations() || cfg.claude.active_station().is_none() {
                anyhow::bail!(
                    "No valid Claude upstream config; please configure ~/.codex-helper/config.toml (or config.json) first"
                );
            }

            let warnings = model_routing_warnings(&cfg, service_name);
            if !warnings.is_empty() {
                tracing::warn!("======== Model routing config warnings ========");
                for w in warnings {
                    tracing::warn!("{}", w);
                }
                tracing::warn!("==============================================");
            }

            let client = Client::builder().build()?;
            let lb_states = Arc::new(Mutex::new(std::collections::HashMap::new()));
            let proxy = ProxyService::new(client, cfg.clone(), service_name, lb_states);
            let state = proxy.state_handle();
            let app = proxy_only_router_with_admin_base_url(
                proxy.clone(),
                Some(local_proxy_base_url(admin_port)),
            );
            let admin_app = admin_listener_router(proxy);

            let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], port));
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .with_context(|| format!("bind {}", addr))?;
            let admin_addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], admin_port));
            let admin_listener = tokio::net::TcpListener::bind(admin_addr)
                .await
                .with_context(|| format!("bind {}", admin_addr))?;

            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            let proxy_server_shutdown = {
                let mut rx = shutdown_rx.clone();
                async move {
                    let _ = rx.changed().await;
                }
            };
            let admin_server_shutdown = {
                let mut rx = shutdown_rx.clone();
                async move {
                    let _ = rx.changed().await;
                }
            };

            let handle = tokio::spawn(async move {
                tokio::try_join!(
                    axum::serve(
                        listener,
                        app.into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .with_graceful_shutdown(proxy_server_shutdown),
                    axum::serve(
                        admin_listener,
                        admin_app.into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .with_graceful_shutdown(admin_server_shutdown),
                )
                .map_err(|e| anyhow::anyhow!("serve error: {e}"))?;
                Ok::<(), anyhow::Error>(())
            });

            Ok::<
                (
                    watch::Sender<bool>,
                    JoinHandle<anyhow::Result<()>>,
                    Arc<ProxyState>,
                    Arc<ProxyConfig>,
                ),
                anyhow::Error,
            >((shutdown_tx, handle, state, cfg))
        };

        let (shutdown_tx, server_handle, state, cfg) = rt.block_on(task)?;

        let default_profile = match service_name {
            "claude" => cfg.claude.default_profile.clone(),
            _ => cfg.codex.default_profile.clone(),
        };
        let configured_active_station = match service_name {
            "claude" => cfg.claude.active.clone(),
            _ => cfg.codex.active.clone(),
        };
        let effective_active_station = match service_name {
            "claude" => cfg.claude.active_station().map(|cfg| cfg.name.clone()),
            _ => cfg.codex.active_station().map(|cfg| cfg.name.clone()),
        };
        let profiles =
            list_profiles_from_cfg(cfg.as_ref(), service_name, default_profile.as_deref());
        let stations =
            list_stations_from_cfg(cfg.as_ref(), service_name, HashMap::new(), HashMap::new());
        let configured_retry = cfg.retry.clone();
        let resolved_retry = configured_retry.resolve();

        self.mode = ProxyMode::Running(RunningProxy {
            service_name,
            port,
            admin_port: admin_port_for_proxy_port(port),
            state,
            cfg,
            last_refresh: None,
            last_error: None,
            active: Vec::new(),
            recent: Vec::new(),
            session_cards: Vec::new(),
            global_station_override: None,
            configured_active_station,
            effective_active_station,
            configured_default_profile: default_profile.clone(),
            default_profile,
            profiles,
            session_model_overrides: HashMap::new(),
            session_station_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_service_tier_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            stations,
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            configured_retry: Some(configured_retry),
            resolved_retry: Some(resolved_retry),
            lb_view: HashMap::new(),
            shutdown_tx,
            server_handle: Some(server_handle),
        });
        self.last_start_error = None;
        self.port_in_use_modal = None;
        Ok(())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn effective_default_profile_from_cfg_state(
    state: &ProxyState,
    service_name: &str,
    cfg: &ProxyConfig,
) -> Option<String> {
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    if let Some(name) = state
        .get_runtime_default_profile_override(service_name)
        .await
        && mgr.profiles.contains_key(name.as_str())
    {
        return Some(name);
    }
    mgr.default_profile.clone()
}

async fn effective_stations_from_cfg_state(
    state: &ProxyState,
    service_name: &str,
    cfg: &ProxyConfig,
) -> Vec<StationOption> {
    let overrides = state.get_station_meta_overrides(service_name).await;
    let state_overrides = state
        .get_station_runtime_state_overrides(service_name)
        .await;
    list_stations_from_cfg(cfg, service_name, overrides, state_overrides)
}

fn list_stations_from_cfg(
    cfg: &ProxyConfig,
    service_name: &str,
    meta_overrides: HashMap<String, (Option<bool>, Option<u8>)>,
    state_overrides: HashMap<String, RuntimeConfigState>,
) -> Vec<StationOption> {
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    build_station_options_from_mgr(mgr, &meta_overrides, &state_overrides)
}

fn list_profiles_from_cfg(
    cfg: &ProxyConfig,
    service_name: &str,
    default_name: Option<&str>,
) -> Vec<ControlProfileOption> {
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    build_profile_options_from_mgr(mgr, default_name)
}

fn is_addr_in_use(err: &anyhow::Error) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err.as_ref());
    while let Some(e) = cur {
        if let Some(io) = e.downcast_ref::<std::io::Error>()
            && io.kind() == std::io::ErrorKind::AddrInUse
        {
            return true;
        }
        cur = e.source();
    }
    false
}

fn suggest_next_port(
    rt: &tokio::runtime::Runtime,
    _service: ServiceKind,
    start: u16,
) -> Option<u16> {
    let fut = async move {
        for delta in 1u16..=50u16 {
            let port = start.saturating_add(delta);
            let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], port));
            if tokio::net::TcpListener::bind(addr).await.is_ok() {
                return Some(port);
            }
        }
        None
    };
    rt.block_on(fut)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex, OnceLock};

    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        routing::{get, post, put},
    };
    use codex_helper_core::dashboard_core::snapshot::DashboardSnapshot;
    use serde_json::Value;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[derive(Default)]
    struct ScopedEnv {
        saved: Vec<(String, Option<String>)>,
    }

    impl ScopedEnv {
        unsafe fn set(&mut self, key: &str, value: &str) {
            if !self.saved.iter().any(|(saved_key, _)| saved_key == key) {
                self.saved.push((key.to_string(), std::env::var(key).ok()));
            }
            unsafe {
                std::env::set_var(key, value);
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            for (key, value) in self.saved.iter().rev() {
                match value {
                    Some(value) => unsafe {
                        std::env::set_var(key, value);
                    },
                    None => unsafe {
                        std::env::remove_var(key);
                    },
                }
            }
        }
    }

    fn sample_station(name: &str) -> StationOption {
        StationOption {
            name: name.to_string(),
            alias: None,
            enabled: true,
            level: 1,
            configured_enabled: true,
            configured_level: 1,
            runtime_enabled_override: None,
            runtime_level_override: None,
            runtime_state: RuntimeConfigState::Normal,
            runtime_state_override: None,
            capabilities: Default::default(),
        }
    }

    fn sample_snapshot(stations: Vec<StationOption>) -> ApiV1Snapshot {
        ApiV1Snapshot {
            api_version: 1,
            service_name: "codex".to_string(),
            runtime_loaded_at_ms: Some(1),
            runtime_source_mtime_ms: Some(2),
            stations,
            configured_active_station: None,
            effective_active_station: None,
            default_profile: None,
            profiles: Vec::new(),
            snapshot: DashboardSnapshot {
                refreshed_at_ms: 1,
                active: Vec::new(),
                recent: Vec::new(),
                session_cards: Vec::new(),
                global_station_override: None,
                session_model_overrides: HashMap::new(),
                session_station_overrides: HashMap::new(),
                session_effort_overrides: HashMap::new(),
                session_service_tier_overrides: HashMap::new(),
                session_stats: HashMap::new(),
                station_health: HashMap::new(),
                health_checks: HashMap::new(),
                lb_view: HashMap::new(),
                usage_rollup: UsageRollupView::default(),
                stats_5m: WindowStats::default(),
                stats_1h: WindowStats::default(),
            },
        }
    }

    fn spawn_test_server(rt: &tokio::runtime::Runtime, app: Router) -> (String, JoinHandle<()>) {
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("bind test server");
            let addr = listener.local_addr().expect("test server addr");
            let handle = tokio::spawn(async move {
                axum::serve(listener, app).await.expect("serve test app");
            });
            (format!("http://{addr}"), handle)
        })
    }

    #[test]
    fn refresh_attached_prefers_station_snapshot_payload() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let caps = serde_json::json!({
            "api_version": 1,
            "service_name": "codex",
            "shared_capabilities": {
                "session_observability": true,
                "request_history": true
            },
            "host_local_capabilities": {
                "session_history": true,
                "transcript_read": true,
                "cwd_enrichment": true
            },
            "endpoints": [
                "/__codex_helper/api/v1/snapshot",
                "/__codex_helper/api/v1/stations",
                "/__codex_helper/api/v1/stations/runtime"
            ]
        });
        let snapshot = sample_snapshot(vec![sample_station("preferred-station")]);
        let app = Router::new()
            .route(
                "/__codex_helper/api/v1/capabilities",
                get({
                    let caps = caps.clone();
                    move || {
                        let caps = caps.clone();
                        async move { Json(caps) }
                    }
                }),
            )
            .route(
                "/__codex_helper/api/v1/snapshot",
                get({
                    let snapshot = snapshot.clone();
                    move || {
                        let snapshot = snapshot.clone();
                        async move { Json(snapshot) }
                    }
                }),
            );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4100, ServiceKind::Codex);
        controller.request_attach_with_admin_base(4100, Some(base_url));
        controller.refresh_attached_if_due(&rt, Duration::ZERO);

        let snapshot = controller.snapshot().expect("attached snapshot");
        assert_eq!(snapshot.stations.len(), 1);
        assert_eq!(snapshot.stations[0].name, "preferred-station");
        assert!(snapshot.shared_capabilities.session_observability);
        assert!(snapshot.shared_capabilities.request_history);
        assert!(snapshot.host_local_capabilities.session_history);
        assert!(snapshot.host_local_capabilities.transcript_read);
        assert!(snapshot.host_local_capabilities.cwd_enrichment);
        assert!(
            controller
                .attached()
                .expect("attached status")
                .supports_station_api
        );

        handle.abort();
    }

    #[test]
    fn refresh_attached_supports_partial_station_surface_with_canonical_effort_api() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let caps = serde_json::json!({
            "api_version": 1,
            "service_name": "codex",
            "shared_capabilities": {
                "session_observability": true,
                "request_history": false
            },
            "host_local_capabilities": {
                "session_history": false,
                "transcript_read": true,
                "cwd_enrichment": false
            },
            "remote_admin_access": {
                "loopback_without_token": true,
                "remote_requires_token": false,
                "remote_enabled": false,
                "token_header": crate::proxy::ADMIN_TOKEN_HEADER,
                "token_env_var": crate::proxy::ADMIN_TOKEN_ENV_VAR
            },
            "endpoints": [
                "/__codex_helper/api/v1/status/active",
                "/__codex_helper/api/v1/status/recent",
                "/__codex_helper/api/v1/status/session-stats",
                "/__codex_helper/api/v1/status/health-checks",
                "/__codex_helper/api/v1/status/station-health",
                "/__codex_helper/api/v1/runtime/status",
                "/__codex_helper/api/v1/stations",
                "/__codex_helper/api/v1/stations/runtime",
                "/__codex_helper/api/v1/overrides/global-station",
                "/__codex_helper/api/v1/overrides/session/station",
                "/__codex_helper/api/v1/overrides/session/effort",
                "/__codex_helper/api/v1/overrides/session/model"
            ]
        });
        let stations = vec![sample_station("station-partial")];
        let app = Router::new()
            .route(
                "/__codex_helper/api/v1/capabilities",
                get({
                    let caps = caps.clone();
                    move || {
                        let caps = caps.clone();
                        async move { Json(caps) }
                    }
                }),
            )
            .route(
                "/__codex_helper/api/v1/status/active",
                get(|| async { Json(Vec::<ActiveRequest>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/recent",
                get(|| async { Json(Vec::<FinishedRequest>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/session-stats",
                get(|| async { Json(HashMap::<String, SessionStats>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/health-checks",
                get(|| async { Json(HashMap::<String, HealthCheckStatus>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/station-health",
                get(|| async { Json(HashMap::<String, StationHealth>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/runtime/status",
                get(|| async {
                    Json(serde_json::json!({
                        "loaded_at_ms": 31,
                        "source_mtime_ms": 32,
                    }))
                }),
            )
            .route(
                "/__codex_helper/api/v1/stations",
                get({
                    let stations = stations.clone();
                    move || {
                        let stations = stations.clone();
                        async move { Json(stations) }
                    }
                }),
            )
            .route(
                "/__codex_helper/api/v1/overrides/global-station",
                get(|| async { Json(Some("station-partial".to_string())) }),
            )
            .route(
                "/__codex_helper/api/v1/overrides/session/station",
                get(|| async {
                    Json(HashMap::from([(
                        "sid-v1".to_string(),
                        "station-partial".to_string(),
                    )]))
                }),
            )
            .route(
                "/__codex_helper/api/v1/overrides/session/effort",
                get(|| async {
                    Json(HashMap::from([(
                        "sid-v1".to_string(),
                        "medium".to_string(),
                    )]))
                }),
            )
            .route(
                "/__codex_helper/api/v1/overrides/session/model",
                get(|| async {
                    Json(HashMap::from([(
                        "sid-v1".to_string(),
                        "gpt-5.4".to_string(),
                    )]))
                }),
            );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4201, ServiceKind::Codex);
        controller.request_attach_with_admin_base(4201, Some(base_url));
        controller.refresh_attached_if_due(&rt, Duration::ZERO);

        let snapshot = controller.snapshot().expect("partial station snapshot");
        assert!(snapshot.supports_v1);
        assert_eq!(snapshot.stations.len(), 1);
        assert_eq!(snapshot.stations[0].name, "station-partial");
        assert_eq!(
            snapshot.global_station_override.as_deref(),
            Some("station-partial")
        );
        assert_eq!(
            snapshot
                .session_station_overrides
                .get("sid-v1")
                .map(String::as_str),
            Some("station-partial")
        );
        assert_eq!(
            snapshot
                .session_effort_overrides
                .get("sid-v1")
                .map(String::as_str),
            Some("medium")
        );
        assert_eq!(
            snapshot
                .session_model_overrides
                .get("sid-v1")
                .map(String::as_str),
            Some("gpt-5.4")
        );
        assert!(snapshot.session_service_tier_overrides.is_empty());
        assert!(snapshot.shared_capabilities.session_observability);
        assert!(snapshot.host_local_capabilities.transcript_read);
        assert!(!snapshot.supports_default_profile_override);
        assert!(!snapshot.supports_session_override_reset);
        let attached = controller.attached().expect("partial attached status");
        assert_eq!(attached.api_version, Some(1));
        assert_eq!(attached.runtime_loaded_at_ms, Some(31));
        assert_eq!(attached.runtime_source_mtime_ms, Some(32));
        assert!(attached.supports_station_api);
        assert!(attached.supports_station_runtime_override);
        assert!(!attached.remote_admin_access.remote_enabled);
        assert!(!attached.remote_admin_access.remote_requires_token);

        handle.abort();
    }

    #[test]
    fn refresh_attached_prefers_typed_surface_capabilities_over_endpoint_strings() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let caps = serde_json::json!({
            "api_version": 1,
            "service_name": "codex",
            "surface_capabilities": {
                "status_active": true,
                "status_recent": true,
                "status_session_stats": true,
                "status_health_checks": true,
                "status_station_health": true,
                "runtime_status": true,
                "stations": true,
                "station_runtime": true,
                "global_station_override": true,
                "session_station_override": true,
                "session_reasoning_effort_override": true,
                "session_model_override": true
            },
            "shared_capabilities": {
                "session_observability": true,
                "request_history": false
            },
            "host_local_capabilities": {
                "session_history": false,
                "transcript_read": false,
                "cwd_enrichment": false
            },
            "endpoints": []
        });
        let stations = vec![sample_station("typed-surface")];
        let app = Router::new()
            .route(
                "/__codex_helper/api/v1/capabilities",
                get({
                    let caps = caps.clone();
                    move || {
                        let caps = caps.clone();
                        async move { Json(caps) }
                    }
                }),
            )
            .route(
                "/__codex_helper/api/v1/status/active",
                get(|| async { Json(Vec::<ActiveRequest>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/recent",
                get(|| async { Json(Vec::<FinishedRequest>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/session-stats",
                get(|| async { Json(HashMap::<String, SessionStats>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/health-checks",
                get(|| async { Json(HashMap::<String, HealthCheckStatus>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/station-health",
                get(|| async { Json(HashMap::<String, StationHealth>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/runtime/status",
                get(|| async {
                    Json(serde_json::json!({
                        "loaded_at_ms": 41,
                        "source_mtime_ms": 42,
                    }))
                }),
            )
            .route(
                "/__codex_helper/api/v1/stations",
                get({
                    let stations = stations.clone();
                    move || {
                        let stations = stations.clone();
                        async move { Json(stations) }
                    }
                }),
            )
            .route(
                "/__codex_helper/api/v1/overrides/global-station",
                get(|| async { Json(Some("typed-surface".to_string())) }),
            )
            .route(
                "/__codex_helper/api/v1/overrides/session/station",
                get(|| async {
                    Json(HashMap::from([(
                        "sid-typed".to_string(),
                        "typed-surface".to_string(),
                    )]))
                }),
            )
            .route(
                "/__codex_helper/api/v1/overrides/session/effort",
                get(|| async {
                    Json(HashMap::from([(
                        "sid-typed".to_string(),
                        "high".to_string(),
                    )]))
                }),
            )
            .route(
                "/__codex_helper/api/v1/overrides/session/model",
                get(|| async {
                    Json(HashMap::from([(
                        "sid-typed".to_string(),
                        "gpt-5.4-fast".to_string(),
                    )]))
                }),
            );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4202, ServiceKind::Codex);
        controller.request_attach_with_admin_base(4202, Some(base_url));
        controller.refresh_attached_if_due(&rt, Duration::ZERO);

        let snapshot = controller.snapshot().expect("typed surface snapshot");
        assert!(snapshot.supports_v1);
        assert_eq!(snapshot.stations.len(), 1);
        assert_eq!(snapshot.stations[0].name, "typed-surface");
        assert_eq!(
            snapshot
                .session_station_overrides
                .get("sid-typed")
                .map(String::as_str),
            Some("typed-surface")
        );
        assert_eq!(
            snapshot
                .session_effort_overrides
                .get("sid-typed")
                .map(String::as_str),
            Some("high")
        );
        assert_eq!(
            snapshot
                .session_model_overrides
                .get("sid-typed")
                .map(String::as_str),
            Some("gpt-5.4-fast")
        );
        let attached = controller.attached().expect("typed attached status");
        assert!(attached.supports_station_api);
        assert!(attached.supports_station_runtime_override);
        assert!(!attached.supports_default_profile_override);

        handle.abort();
    }

    #[test]
    fn refresh_attached_rejects_pre_v1_runtime_surface() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let app = Router::new()
            .route(
                "/__codex_helper/status/active",
                get(|| async { Json(Vec::<ActiveRequest>::new()) }),
            )
            .route(
                "/__codex_helper/status/recent",
                get(|| async { Json(Vec::<FinishedRequest>::new()) }),
            )
            .route(
                "/__codex_helper/config/runtime",
                get(|| async {
                    Json(serde_json::json!({
                        "loaded_at_ms": 51,
                        "source_mtime_ms": 52,
                    }))
                }),
            )
            .route(
                "/__codex_helper/override/session",
                get(|| async {
                    Json(HashMap::from([(
                        "sid-legacy".to_string(),
                        "low".to_string(),
                    )]))
                }),
            );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4202, ServiceKind::Codex);
        controller.request_attach_with_admin_base(4202, Some(base_url));
        controller.refresh_attached_if_due(&rt, Duration::ZERO);

        let snapshot = controller.snapshot().expect("attached snapshot");
        assert!(!snapshot.supports_v1);
        assert!(snapshot.stations.is_empty());
        assert!(snapshot.session_effort_overrides.is_empty());
        assert!(snapshot.last_error.is_some());
        let attached = controller.attached().expect("attached status");
        assert_eq!(attached.api_version, None);
        assert!(attached.last_error.is_some());
        assert_eq!(attached.runtime_loaded_at_ms, None);
        assert_eq!(attached.runtime_source_mtime_ms, None);
        assert!(!attached.supports_station_api);
        assert!(!attached.supports_station_runtime_override);

        handle.abort();
    }

    #[test]
    fn refresh_attached_prefers_aggregate_session_override_api() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let caps = serde_json::json!({
            "api_version": 1,
            "service_name": "codex",
            "endpoints": [
                "/__codex_helper/api/v1/status/active",
                "/__codex_helper/api/v1/status/recent",
                "/__codex_helper/api/v1/status/session-stats",
                "/__codex_helper/api/v1/status/health-checks",
                "/__codex_helper/api/v1/status/station-health",
                "/__codex_helper/api/v1/runtime/status",
                "/__codex_helper/api/v1/stations",
                "/__codex_helper/api/v1/overrides/global-station",
                "/__codex_helper/api/v1/overrides/session"
            ]
        });
        let stations = vec![sample_station("aggregate-only")];
        let app = Router::new()
            .route(
                "/__codex_helper/api/v1/capabilities",
                get({
                    let caps = caps.clone();
                    move || {
                        let caps = caps.clone();
                        async move { Json(caps) }
                    }
                }),
            )
            .route(
                "/__codex_helper/api/v1/status/active",
                get(|| async { Json(Vec::<ActiveRequest>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/recent",
                get(|| async { Json(Vec::<FinishedRequest>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/session-stats",
                get(|| async { Json(HashMap::<String, SessionStats>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/health-checks",
                get(|| async { Json(HashMap::<String, HealthCheckStatus>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/status/station-health",
                get(|| async { Json(HashMap::<String, StationHealth>::new()) }),
            )
            .route(
                "/__codex_helper/api/v1/runtime/status",
                get(|| async {
                    Json(serde_json::json!({
                        "loaded_at_ms": 11,
                        "source_mtime_ms": 22,
                    }))
                }),
            )
            .route(
                "/__codex_helper/api/v1/stations",
                get({
                    let stations = stations.clone();
                    move || {
                        let stations = stations.clone();
                        async move { Json(stations) }
                    }
                }),
            )
            .route(
                "/__codex_helper/api/v1/overrides/global-station",
                get(|| async { Json(Option::<String>::None) }),
            )
            .route(
                "/__codex_helper/api/v1/overrides/session",
                get(|| async {
                    Json(serde_json::json!({
                        "sessions": {
                            "sid-a": {
                                "reasoning_effort": "high",
                                "station_name": "aggregate-only",
                                "model": "gpt-5.4",
                                "service_tier": "priority"
                            }
                        }
                    }))
                }),
            );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4201, ServiceKind::Codex);
        controller.request_attach_with_admin_base(4201, Some(base_url));
        controller.refresh_attached_if_due(&rt, Duration::ZERO);

        let snapshot = controller.snapshot().expect("aggregate attached snapshot");
        assert_eq!(
            snapshot
                .session_station_overrides
                .get("sid-a")
                .map(String::as_str),
            Some("aggregate-only")
        );
        assert_eq!(
            snapshot
                .session_effort_overrides
                .get("sid-a")
                .map(String::as_str),
            Some("high")
        );
        assert_eq!(
            snapshot
                .session_model_overrides
                .get("sid-a")
                .map(String::as_str),
            Some("gpt-5.4")
        );
        assert_eq!(
            snapshot
                .session_service_tier_overrides
                .get("sid-a")
                .map(String::as_str),
            Some("priority")
        );

        handle.abort();
    }

    #[test]
    fn refresh_attached_sends_admin_token_when_configured() {
        let _env_lock = env_lock();
        let mut scoped = ScopedEnv::default();
        unsafe {
            scoped.set(crate::proxy::ADMIN_TOKEN_ENV_VAR, "gui-secret");
        }

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let observed_headers = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
        let caps = serde_json::json!({
            "api_version": 1,
            "service_name": "codex",
            "shared_capabilities": {
                "session_observability": true,
                "request_history": true
            },
            "host_local_capabilities": {
                "session_history": false,
                "transcript_read": false,
                "cwd_enrichment": false
            },
            "remote_admin_access": {
                "loopback_without_token": true,
                "remote_requires_token": true,
                "remote_enabled": true,
                "token_header": crate::proxy::ADMIN_TOKEN_HEADER,
                "token_env_var": crate::proxy::ADMIN_TOKEN_ENV_VAR
            },
            "endpoints": [
                "/__codex_helper/api/v1/snapshot"
            ]
        });
        let snapshot = sample_snapshot(vec![sample_station("alpha")]);
        let app = Router::new()
            .route(
                "/__codex_helper/api/v1/capabilities",
                get({
                    let caps = caps.clone();
                    let observed_headers = observed_headers.clone();
                    move |headers: HeaderMap| {
                        let caps = caps.clone();
                        let observed_headers = observed_headers.clone();
                        async move {
                            observed_headers.lock().expect("header lock").push(
                                headers
                                    .get(crate::proxy::ADMIN_TOKEN_HEADER)
                                    .and_then(|value| value.to_str().ok())
                                    .map(str::to_string),
                            );
                            Json(caps)
                        }
                    }
                }),
            )
            .route(
                "/__codex_helper/api/v1/snapshot",
                get({
                    let snapshot = snapshot.clone();
                    let observed_headers = observed_headers.clone();
                    move |headers: HeaderMap| {
                        let snapshot = snapshot.clone();
                        let observed_headers = observed_headers.clone();
                        async move {
                            observed_headers.lock().expect("header lock").push(
                                headers
                                    .get(crate::proxy::ADMIN_TOKEN_HEADER)
                                    .and_then(|value| value.to_str().ok())
                                    .map(str::to_string),
                            );
                            Json(snapshot)
                        }
                    }
                }),
            );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4250, ServiceKind::Codex);
        controller.request_attach_with_admin_base(4250, Some(base_url));
        controller.refresh_attached_if_due(&rt, Duration::ZERO);

        let observed_headers = observed_headers.lock().expect("header lock").clone();
        assert!(!observed_headers.is_empty());
        assert!(
            observed_headers
                .iter()
                .all(|value| value.as_deref() == Some("gui-secret"))
        );
        assert!(
            controller
                .attached()
                .expect("attached status")
                .remote_admin_access
                .remote_enabled
        );

        handle.abort();
    }

    #[test]
    fn read_control_trace_entries_prefers_attached_api_when_supported() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let app = Router::new().route(
            "/__codex_helper/api/v1/control-trace",
            get(|| async {
                Json(vec![ControlTraceLogEntry {
                    ts_ms: 300,
                    kind: "request_completed".to_string(),
                    service: Some("codex".to_string()),
                    request_id: Some(9),
                    event: Some("request_completed".to_string()),
                    detail: None,
                    payload: serde_json::json!({
                        "method": "POST",
                        "path": "/v1/responses"
                    }),
                }])
            }),
        );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4290, ServiceKind::Codex);
        let mut attached = AttachedStatus::new(4290);
        attached.admin_base_url = base_url.clone();
        attached.supports_control_trace_api = true;
        controller.mode = ProxyMode::Attached(attached);

        let result = controller
            .read_control_trace_entries(&rt, 80)
            .expect("read attached control trace");

        assert_eq!(
            result.source,
            ControlTraceDataSource::AttachedApi {
                admin_base_url: base_url
            }
        );
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].ts_ms, 300);
        assert_eq!(result.entries[0].kind, "request_completed");

        handle.abort();
    }

    #[test]
    fn read_control_trace_entries_falls_back_to_local_file_when_api_is_unavailable() {
        let _env_lock = env_lock();
        let mut scoped = ScopedEnv::default();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let trace_path = std::env::temp_dir()
            .join("codex-helper-gui-tests")
            .join(format!("control-trace-{unique}.jsonl"));
        std::fs::create_dir_all(trace_path.parent().expect("trace parent"))
            .expect("create trace parent");
        unsafe {
            scoped.set(
                "CODEX_HELPER_CONTROL_TRACE_PATH",
                trace_path.to_string_lossy().as_ref(),
            );
        }
        std::fs::write(
            &trace_path,
            [
                serde_json::json!({
                    "ts_ms": 100,
                    "kind": "retry_trace",
                    "service": "codex",
                    "request_id": 4,
                    "event": "attempt_select",
                    "payload": {
                        "event": "attempt_select",
                        "station_name": "right"
                    }
                })
                .to_string(),
                serde_json::json!({
                    "ts_ms": 200,
                    "kind": "request_completed",
                    "service": "codex",
                    "request_id": 4,
                    "event": "request_completed",
                    "payload": {
                        "method": "POST",
                        "path": "/v1/responses"
                    }
                })
                .to_string(),
            ]
            .join("\n"),
        )
        .expect("write local control trace");

        let mut controller = ProxyController::new(4291, ServiceKind::Codex);
        let mut attached = AttachedStatus::new(4291);
        attached.admin_base_url = "http://100.88.0.5:4101".to_string();
        attached.supports_control_trace_api = false;
        controller.mode = ProxyMode::Attached(attached);

        let result = controller
            .read_control_trace_entries(&rt, 80)
            .expect("fallback local control trace");

        assert_eq!(
            result.source,
            ControlTraceDataSource::AttachedFallbackLocal {
                admin_base_url: "http://100.88.0.5:4101".to_string(),
                path: trace_path.clone(),
            }
        );
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.entries[0].ts_ms, 200);
        assert_eq!(result.entries[0].kind, "request_completed");
    }

    #[test]
    fn attached_runtime_meta_uses_station_endpoints() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");

        let station_payload = Arc::new(Mutex::new(None::<Value>));
        let station_app = Router::new().route(
            "/__codex_helper/api/v1/stations/runtime",
            post({
                let station_payload = station_payload.clone();
                move |Json(payload): Json<Value>| {
                    let station_payload = station_payload.clone();
                    async move {
                        *station_payload.lock().expect("station payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
        let (station_base_url, station_handle) = spawn_test_server(&rt, station_app);

        let mut station_controller = ProxyController::new(4300, ServiceKind::Codex);
        let mut station_attached = AttachedStatus::new(4300);
        station_attached.admin_base_url = station_base_url;
        station_attached.supports_station_runtime_override = true;
        station_attached.supports_station_api = true;
        station_controller.mode = ProxyMode::Attached(station_attached);
        station_controller
            .set_runtime_station_meta(
                &rt,
                "alpha".to_string(),
                Some(Some(false)),
                Some(Some(7)),
                Some(Some(RuntimeConfigState::Draining)),
            )
            .expect("station runtime meta update");

        let station_payload = station_payload
            .lock()
            .expect("station payload lock")
            .clone()
            .expect("station payload");
        assert_eq!(
            station_payload.get("station_name"),
            Some(&Value::String("alpha".to_string()))
        );
        assert_eq!(
            station_payload.get("runtime_state"),
            Some(&Value::String("draining".to_string()))
        );
        station_handle.abort();
    }

    #[test]
    fn attached_probe_station_uses_station_probe_and_legacy_healthcheck_endpoints() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");

        let station_payload = Arc::new(Mutex::new(None::<Value>));
        let station_app = Router::new().route(
            "/__codex_helper/api/v1/stations/probe",
            post({
                let station_payload = station_payload.clone();
                move |Json(payload): Json<Value>| {
                    let station_payload = station_payload.clone();
                    async move {
                        *station_payload.lock().expect("station payload lock") = Some(payload);
                        StatusCode::OK
                    }
                }
            }),
        );
        let (station_base_url, station_handle) = spawn_test_server(&rt, station_app);

        let mut station_controller = ProxyController::new(4306, ServiceKind::Codex);
        let mut station_attached = AttachedStatus::new(4306);
        station_attached.admin_base_url = station_base_url;
        station_attached.api_version = Some(1);
        station_attached.supports_station_api = true;
        station_controller.mode = ProxyMode::Attached(station_attached);
        station_controller
            .probe_station(&rt, "alpha".to_string())
            .expect("station probe");

        let station_payload = station_payload
            .lock()
            .expect("station payload lock")
            .clone()
            .expect("station payload");
        assert_eq!(
            station_payload.get("station_name"),
            Some(&Value::String("alpha".to_string()))
        );
        station_handle.abort();

        let legacy_payload = Arc::new(Mutex::new(None::<Value>));
        let legacy_app = Router::new().route(
            "/__codex_helper/api/v1/healthcheck/start",
            post({
                let legacy_payload = legacy_payload.clone();
                move |Json(payload): Json<Value>| {
                    let legacy_payload = legacy_payload.clone();
                    async move {
                        *legacy_payload.lock().expect("legacy payload lock") = Some(payload);
                        StatusCode::OK
                    }
                }
            }),
        );
        let (legacy_base_url, legacy_handle) = spawn_test_server(&rt, legacy_app);

        let mut legacy_controller = ProxyController::new(4307, ServiceKind::Codex);
        let mut legacy_attached = AttachedStatus::new(4307);
        legacy_attached.admin_base_url = legacy_base_url;
        legacy_attached.api_version = Some(1);
        legacy_attached.supports_station_api = false;
        legacy_controller.mode = ProxyMode::Attached(legacy_attached);
        legacy_controller
            .probe_station(&rt, "beta".to_string())
            .expect("legacy probe fallback");

        let legacy_payload = legacy_payload
            .lock()
            .expect("legacy payload lock")
            .clone()
            .expect("legacy payload");
        assert_eq!(legacy_payload.get("all"), Some(&Value::Bool(false)));
        assert_eq!(
            legacy_payload
                .get("station_names")
                .and_then(|value| value.as_array())
                .and_then(|items| items.first())
                .and_then(|value| value.as_str()),
            Some("beta")
        );
        legacy_handle.abort();
    }

    #[test]
    fn attached_persisted_station_config_uses_v1_station_endpoints() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");

        let active_payload = Arc::new(Mutex::new(None::<Value>));
        let update_payload = Arc::new(Mutex::new(None::<Value>));
        let app = Router::new()
            .route(
                "/__codex_helper/api/v1/stations/config-active",
                post({
                    let active_payload = active_payload.clone();
                    move |Json(payload): Json<Value>| {
                        let active_payload = active_payload.clone();
                        async move {
                            *active_payload.lock().expect("active payload lock") = Some(payload);
                            StatusCode::NO_CONTENT
                        }
                    }
                }),
            )
            .route(
                "/__codex_helper/api/v1/stations/alpha",
                put({
                    let update_payload = update_payload.clone();
                    move |Json(payload): Json<Value>| {
                        let update_payload = update_payload.clone();
                        async move {
                            *update_payload.lock().expect("update payload lock") = Some(payload);
                            StatusCode::NO_CONTENT
                        }
                    }
                }),
            );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4302, ServiceKind::Codex);
        let mut attached = AttachedStatus::new(4302);
        attached.api_version = Some(1);
        attached.admin_base_url = base_url;
        attached.supports_persisted_station_config = true;
        controller.mode = ProxyMode::Attached(attached);

        controller
            .set_persisted_active_station(&rt, Some("alpha".to_string()))
            .expect("set persisted active station");
        controller
            .update_persisted_station(&rt, "alpha".to_string(), false, 7)
            .expect("update persisted station");

        let active_payload = active_payload
            .lock()
            .expect("active payload lock")
            .clone()
            .expect("active payload");
        assert_eq!(
            active_payload.get("station_name"),
            Some(&Value::String("alpha".to_string()))
        );

        let update_payload = update_payload
            .lock()
            .expect("update payload lock")
            .clone()
            .expect("update payload");
        assert_eq!(update_payload.get("enabled"), Some(&Value::Bool(false)));
        assert_eq!(update_payload.get("level"), Some(&Value::from(7)));

        handle.abort();
    }

    #[test]
    fn attached_persisted_retry_config_uses_v1_retry_endpoint() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");

        let observed_payload = Arc::new(Mutex::new(None::<Value>));
        let app = Router::new().route(
            "/__codex_helper/api/v1/retry/config",
            post({
                let observed_payload = observed_payload.clone();
                move |Json(payload): Json<Value>| {
                    let observed_payload = observed_payload.clone();
                    async move {
                        *observed_payload.lock().expect("retry payload lock") =
                            Some(payload.clone());
                        Json(serde_json::json!({
                            "configured": payload,
                            "resolved": {
                                "upstream": {
                                    "max_attempts": 2,
                                    "backoff_ms": 200,
                                    "backoff_max_ms": 2000,
                                    "jitter_ms": 100,
                                    "on_status": "429,500-599,524",
                                    "on_class": ["upstream_transport_error"],
                                    "strategy": "same_upstream"
                                },
                                "provider": {
                                    "max_attempts": 2,
                                    "backoff_ms": 0,
                                    "backoff_max_ms": 0,
                                    "jitter_ms": 0,
                                    "on_status": "401,403,404,408,429,500-599,524",
                                    "on_class": ["upstream_transport_error"],
                                    "strategy": "failover"
                                },
                                "allow_cross_station_before_first_output": true,
                                "never_on_status": "413,415,422",
                                "never_on_class": ["client_error_non_retryable"],
                                "cloudflare_challenge_cooldown_secs": 300,
                                "cloudflare_timeout_cooldown_secs": 12,
                                "transport_cooldown_secs": 45,
                                "cooldown_backoff_factor": 3,
                                "cooldown_backoff_max_secs": 180
                            }
                        }))
                    }
                }
            }),
        );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4303, ServiceKind::Codex);
        let mut attached = AttachedStatus::new(4303);
        attached.api_version = Some(1);
        attached.admin_base_url = base_url;
        attached.supports_retry_config_api = true;
        controller.mode = ProxyMode::Attached(attached);

        controller
            .set_persisted_retry_config(
                &rt,
                RetryConfig {
                    profile: Some(crate::config::RetryProfileName::CostPrimary),
                    transport_cooldown_secs: Some(45),
                    cloudflare_timeout_cooldown_secs: Some(12),
                    cooldown_backoff_factor: Some(3),
                    cooldown_backoff_max_secs: Some(180),
                    ..Default::default()
                },
            )
            .expect("set persisted retry config");

        let observed_payload = observed_payload
            .lock()
            .expect("retry payload lock")
            .clone()
            .expect("retry payload");
        assert_eq!(
            observed_payload.get("profile"),
            Some(&Value::String("cost-primary".to_string()))
        );
        assert_eq!(
            observed_payload.get("transport_cooldown_secs"),
            Some(&Value::from(45))
        );
        assert_eq!(
            observed_payload.get("cooldown_backoff_factor"),
            Some(&Value::from(3))
        );

        let snapshot = controller.snapshot().expect("snapshot");
        assert_eq!(
            snapshot
                .configured_retry
                .as_ref()
                .and_then(|retry| retry.profile),
            Some(crate::config::RetryProfileName::CostPrimary)
        );
        assert_eq!(
            snapshot
                .resolved_retry
                .as_ref()
                .map(|retry| retry.transport_cooldown_secs),
            Some(45)
        );
        assert_eq!(
            snapshot
                .resolved_retry
                .as_ref()
                .map(|retry| retry.cooldown_backoff_factor),
            Some(3)
        );
        assert_eq!(
            snapshot
                .resolved_retry
                .as_ref()
                .map(|retry| retry.allow_cross_station_before_first_output),
            Some(true)
        );

        handle.abort();
    }

    #[test]
    fn attached_persisted_station_spec_uses_v1_specs_endpoints() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");

        let observed_put_payload = Arc::new(Mutex::new(None::<Value>));
        let delete_hits = Arc::new(Mutex::new(0usize));
        let app = Router::new().route(
            "/__codex_helper/api/v1/stations/specs/alpha",
            put({
                let observed_put_payload = observed_put_payload.clone();
                move |Json(payload): Json<Value>| {
                    let observed_put_payload = observed_put_payload.clone();
                    async move {
                        *observed_put_payload
                            .lock()
                            .expect("station spec payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            })
            .delete({
                let delete_hits = delete_hits.clone();
                move || {
                    let delete_hits = delete_hits.clone();
                    async move {
                        *delete_hits.lock().expect("delete hits lock") += 1;
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4304, ServiceKind::Codex);
        let mut attached = AttachedStatus::new(4304);
        attached.api_version = Some(1);
        attached.admin_base_url = base_url;
        attached.supports_station_spec_api = true;
        controller.mode = ProxyMode::Attached(attached);

        controller
            .upsert_persisted_station_spec(
                &rt,
                "alpha".to_string(),
                PersistedStationSpec {
                    name: "alpha".to_string(),
                    alias: Some("Alpha".to_string()),
                    enabled: false,
                    level: 7,
                    members: vec![crate::config::GroupMemberRefV2 {
                        provider: "right".to_string(),
                        endpoint_names: vec!["hk".to_string()],
                        preferred: true,
                    }],
                },
            )
            .expect("upsert persisted station spec");
        controller
            .delete_persisted_station_spec(&rt, "alpha".to_string())
            .expect("delete persisted station spec");

        let observed_put_payload = observed_put_payload
            .lock()
            .expect("station spec payload lock")
            .clone()
            .expect("station spec payload");
        assert_eq!(
            observed_put_payload.get("alias"),
            Some(&Value::String("Alpha".to_string()))
        );
        assert_eq!(
            observed_put_payload.get("enabled"),
            Some(&Value::Bool(false))
        );
        assert_eq!(observed_put_payload.get("level"), Some(&Value::from(7)));
        assert_eq!(
            observed_put_payload["members"][0]
                .get("provider")
                .and_then(|value| value.as_str()),
            Some("right")
        );
        assert_eq!(*delete_hits.lock().expect("delete hits lock"), 1);

        handle.abort();
    }

    #[test]
    fn attached_session_override_reset_uses_v1_reset_endpoint() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");

        let observed_payload = Arc::new(Mutex::new(None::<Value>));
        let app = Router::new().route(
            "/__codex_helper/api/v1/overrides/session/reset",
            post({
                let observed_payload = observed_payload.clone();
                move |Json(payload): Json<Value>| {
                    let observed_payload = observed_payload.clone();
                    async move {
                        *observed_payload.lock().expect("reset payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4305, ServiceKind::Codex);
        let mut attached = AttachedStatus::new(4305);
        attached.api_version = Some(1);
        attached.admin_base_url = base_url;
        attached.supports_session_override_reset = true;
        controller.mode = ProxyMode::Attached(attached);

        controller
            .clear_session_manual_overrides(&rt, "sid-reset".to_string())
            .expect("reset session manual overrides");

        let observed_payload = observed_payload
            .lock()
            .expect("reset payload lock")
            .clone()
            .expect("reset payload");
        assert_eq!(
            observed_payload.get("session_id"),
            Some(&Value::String("sid-reset".to_string()))
        );

        handle.abort();
    }

    #[test]
    fn attached_session_effort_override_uses_v1_effort_endpoint() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");

        let observed_payload = Arc::new(Mutex::new(None::<Value>));
        let app = Router::new().route(
            "/__codex_helper/api/v1/overrides/session/effort",
            post({
                let observed_payload = observed_payload.clone();
                move |Json(payload): Json<Value>| {
                    let observed_payload = observed_payload.clone();
                    async move {
                        *observed_payload.lock().expect("effort payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4308, ServiceKind::Codex);
        let mut attached = AttachedStatus::new(4308);
        attached.api_version = Some(1);
        attached.admin_base_url = base_url;
        controller.mode = ProxyMode::Attached(attached);

        controller
            .apply_session_effort_override(&rt, "sid-effort".to_string(), Some("high".to_string()))
            .expect("set effort via canonical endpoint");

        let observed_payload = observed_payload
            .lock()
            .expect("effort payload lock")
            .clone()
            .expect("effort payload");
        assert_eq!(
            observed_payload.get("session_id"),
            Some(&Value::String("sid-effort".to_string()))
        );
        assert_eq!(
            observed_payload.get("effort"),
            Some(&Value::String("high".to_string()))
        );

        handle.abort();
    }

    #[test]
    fn attached_persisted_provider_spec_uses_v1_specs_endpoints() {
        let rt = tokio::runtime::Runtime::new().expect("runtime");

        let observed_put_payload = Arc::new(Mutex::new(None::<Value>));
        let delete_hits = Arc::new(Mutex::new(0usize));
        let app = Router::new().route(
            "/__codex_helper/api/v1/providers/specs/right",
            put({
                let observed_put_payload = observed_put_payload.clone();
                move |Json(payload): Json<Value>| {
                    let observed_put_payload = observed_put_payload.clone();
                    async move {
                        *observed_put_payload
                            .lock()
                            .expect("provider spec payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            })
            .delete({
                let delete_hits = delete_hits.clone();
                move || {
                    let delete_hits = delete_hits.clone();
                    async move {
                        *delete_hits.lock().expect("delete hits lock") += 1;
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
        let (base_url, handle) = spawn_test_server(&rt, app);

        let mut controller = ProxyController::new(4305, ServiceKind::Codex);
        let mut attached = AttachedStatus::new(4305);
        attached.api_version = Some(1);
        attached.admin_base_url = base_url;
        attached.supports_provider_spec_api = true;
        controller.mode = ProxyMode::Attached(attached);

        controller
            .upsert_persisted_provider_spec(
                &rt,
                "right".to_string(),
                PersistedProviderSpec {
                    name: "right".to_string(),
                    alias: Some("Right".to_string()),
                    enabled: false,
                    auth_token_env: Some("RIGHTCODE_API_KEY".to_string()),
                    api_key_env: Some("RIGHTCODE_HEADER_KEY".to_string()),
                    endpoints: vec![
                        crate::config::PersistedProviderEndpointSpec {
                            name: "hk".to_string(),
                            base_url: "https://right-hk.example.com/v1".to_string(),
                            enabled: true,
                            priority: 0,
                        },
                        crate::config::PersistedProviderEndpointSpec {
                            name: "us".to_string(),
                            base_url: "https://right-us.example.com/v1".to_string(),
                            enabled: false,
                            priority: 1,
                        },
                    ],
                },
            )
            .expect("upsert persisted provider spec");
        controller
            .delete_persisted_provider_spec(&rt, "right".to_string())
            .expect("delete persisted provider spec");

        let observed_put_payload = observed_put_payload
            .lock()
            .expect("provider spec payload lock")
            .clone()
            .expect("provider spec payload");
        assert_eq!(
            observed_put_payload.get("alias"),
            Some(&Value::String("Right".to_string()))
        );
        assert_eq!(
            observed_put_payload.get("enabled"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            observed_put_payload.get("auth_token_env"),
            Some(&Value::String("RIGHTCODE_API_KEY".to_string()))
        );
        assert_eq!(
            observed_put_payload.get("api_key_env"),
            Some(&Value::String("RIGHTCODE_HEADER_KEY".to_string()))
        );
        assert_eq!(
            observed_put_payload["endpoints"][0]
                .get("name")
                .and_then(|value| value.as_str()),
            Some("hk")
        );
        assert_eq!(*delete_hits.lock().expect("delete hits lock"), 1);

        handle.abort();
    }
}
