use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use futures_util::future::join_all;
use reqwest::Client;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use std::collections::HashMap;

use crate::config::{
    ProxyConfig, ServiceKind, load_or_bootstrap_for_service, model_routing_warnings,
};
use crate::dashboard_core::{
    ApiV1Snapshot, ConfigOption, ControlProfileOption, WindowStats, build_dashboard_snapshot,
};
use crate::proxy::{
    ProxyService, admin_listener_router, admin_port_for_proxy_port,
    local_admin_base_url_for_proxy_port, local_proxy_base_url,
    proxy_only_router_with_admin_base_url,
};
use crate::state::{
    ActiveRequest, ConfigHealth, FinishedRequest, HealthCheckStatus, LbConfigView, ProxyState,
    SessionIdentityCard, SessionStats, UsageRollupView,
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
    pub global_override: Option<String>,
    pub default_profile: Option<String>,
    pub profiles: Vec<ControlProfileOption>,
    pub session_model_overrides: HashMap<String, String>,
    pub session_config_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub configs: Vec<ConfigOption>,
    pub config_health: HashMap<String, ConfigHealth>,
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
    pub lb_view: HashMap<String, LbConfigView>,
    pub runtime_loaded_at_ms: Option<u64>,
    pub runtime_source_mtime_ms: Option<u64>,
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
            global_override: None,
            default_profile: None,
            profiles: Vec::new(),
            session_model_overrides: HashMap::new(),
            session_config_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_service_tier_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            configs: Vec::new(),
            config_health: HashMap::new(),
            health_checks: HashMap::new(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            lb_view: HashMap::new(),
            runtime_loaded_at_ms: None,
            runtime_source_mtime_ms: None,
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
    pub global_override: Option<String>,
    pub default_profile: Option<String>,
    pub profiles: Vec<ControlProfileOption>,
    pub session_model_overrides: HashMap<String, String>,
    pub session_config_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub configs: Vec<ConfigOption>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
    pub supports_v1: bool,
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
    pub global_override: Option<String>,
    pub default_profile: Option<String>,
    pub profiles: Vec<ControlProfileOption>,
    pub session_model_overrides: HashMap<String, String>,
    pub session_config_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub config_health: HashMap<String, ConfigHealth>,
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
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
    pub runtime_loaded_at_ms: Option<u64>,
    pub last_error: Option<String>,
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
                global_override: r.global_override.clone(),
                default_profile: r.default_profile.clone(),
                profiles: r.profiles.clone(),
                session_model_overrides: r.session_model_overrides.clone(),
                session_config_overrides: r.session_config_overrides.clone(),
                session_effort_overrides: r.session_effort_overrides.clone(),
                session_service_tier_overrides: r.session_service_tier_overrides.clone(),
                session_stats: r.session_stats.clone(),
                configs: list_configs_from_cfg(r.cfg.as_ref(), r.service_name),
                usage_rollup: r.usage_rollup.clone(),
                stats_5m: r.stats_5m.clone(),
                stats_1h: r.stats_1h.clone(),
                supports_v1: true,
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
                global_override: a.global_override.clone(),
                default_profile: a.default_profile.clone(),
                profiles: a.profiles.clone(),
                session_model_overrides: a.session_model_overrides.clone(),
                session_config_overrides: a.session_config_overrides.clone(),
                session_effort_overrides: a.session_effort_overrides.clone(),
                session_service_tier_overrides: a.session_service_tier_overrides.clone(),
                session_stats: a.session_stats.clone(),
                configs: a.configs.clone(),
                usage_rollup: a.usage_rollup.clone(),
                stats_5m: a.stats_5m.clone(),
                stats_1h: a.stats_1h.clone(),
                supports_v1: a.api_version == Some(1),
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
            attached.admin_base_url = admin_base_url;
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
        struct ApiCapabilities {
            api_version: u32,
            service_name: String,
            #[serde(default)]
            endpoints: Vec<String>,
        }

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
            Ok(client
                .get(url)
                .timeout(timeout)
                .send()
                .await?
                .error_for_status()?
                .json::<T>()
                .await?)
        }

        async fn scan_port(client: Client, port: u16) -> Option<DiscoveredProxy> {
            let base_url = local_proxy_base_url(port);
            let admin_base_url = local_admin_base_url_for_proxy_port(port);
            let timeout = Duration::from_millis(250);

            let caps = get_json::<ApiCapabilities>(
                &client,
                format!("{admin_base_url}/__codex_helper/api/v1/capabilities"),
                timeout,
            )
            .await;

            if let Ok(c) = caps {
                let runtime = get_json::<RuntimeConfigStatus>(
                    &client,
                    format!("{admin_base_url}/__codex_helper/api/v1/config/runtime"),
                    timeout,
                )
                .await
                .ok();

                return Some(DiscoveredProxy {
                    port,
                    base_url,
                    admin_base_url,
                    api_version: Some(c.api_version),
                    service_name: Some(c.service_name),
                    endpoints: c.endpoints,
                    runtime_loaded_at_ms: runtime.as_ref().map(|r| r.loaded_at_ms),
                    last_error: None,
                });
            }

            let caps = get_json::<ApiCapabilities>(
                &client,
                format!("{base_url}/__codex_helper/api/v1/capabilities"),
                timeout,
            )
            .await;

            if let Ok(c) = caps {
                let runtime = get_json::<RuntimeConfigStatus>(
                    &client,
                    format!("{base_url}/__codex_helper/api/v1/config/runtime"),
                    timeout,
                )
                .await
                .ok();

                return Some(DiscoveredProxy {
                    port,
                    base_url: base_url.clone(),
                    admin_base_url: base_url,
                    api_version: Some(c.api_version),
                    service_name: Some(c.service_name),
                    endpoints: c.endpoints,
                    runtime_loaded_at_ms: runtime.as_ref().map(|r| r.loaded_at_ms),
                    last_error: None,
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
                let caps = get_json::<ApiCapabilities>(
                    &client,
                    format!("{discovered_admin_base}/__codex_helper/api/v1/capabilities"),
                    timeout,
                )
                .await;

                if let Ok(c) = caps {
                    let runtime = get_json::<RuntimeConfigStatus>(
                        &client,
                        format!("{discovered_admin_base}/__codex_helper/api/v1/config/runtime"),
                        timeout,
                    )
                    .await
                    .ok();

                    return Some(DiscoveredProxy {
                        port,
                        base_url,
                        admin_base_url: discovered_admin_base,
                        api_version: Some(c.api_version),
                        service_name: Some(c.service_name),
                        endpoints: c.endpoints,
                        runtime_loaded_at_ms: runtime.as_ref().map(|r| r.loaded_at_ms),
                        last_error: None,
                    });
                }

                if let Ok(runtime) = get_json::<RuntimeConfigStatus>(
                    &client,
                    format!("{discovered_admin_base}/__codex_helper/config/runtime"),
                    timeout,
                )
                .await
                {
                    return Some(DiscoveredProxy {
                        port,
                        base_url,
                        admin_base_url: discovered_admin_base,
                        api_version: None,
                        service_name: None,
                        endpoints: Vec::new(),
                        runtime_loaded_at_ms: Some(runtime.loaded_at_ms),
                        last_error: None,
                    });
                }
            }

            let runtime = match get_json::<RuntimeConfigStatus>(
                &client,
                format!("{admin_base_url}/__codex_helper/config/runtime"),
                timeout,
            )
            .await
            {
                Ok(runtime) => Ok(runtime),
                Err(_) => {
                    get_json::<RuntimeConfigStatus>(
                        &client,
                        format!("{base_url}/__codex_helper/config/runtime"),
                        timeout,
                    )
                    .await
                }
            };
            match runtime {
                Ok(r) => Some(DiscoveredProxy {
                    port,
                    base_url,
                    admin_base_url,
                    api_version: None,
                    service_name: None,
                    endpoints: Vec::new(),
                    runtime_loaded_at_ms: Some(r.loaded_at_ms),
                    last_error: None,
                }),
                Err(_) => None,
            }
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
            struct ApiCapabilities {
                api_version: u32,
                service_name: String,
                #[serde(default)]
                endpoints: Vec<String>,
            }

            #[derive(Debug, serde::Deserialize)]
            struct RuntimeConfigStatus {
                loaded_at_ms: u64,
                #[serde(default)]
                source_mtime_ms: Option<u64>,
            }

            let req_timeout = Duration::from_millis(800);
            async fn get_json<T: serde::de::DeserializeOwned>(
                client: &Client,
                url: String,
                timeout: Duration,
            ) -> anyhow::Result<T> {
                Ok(client
                    .get(url)
                    .timeout(timeout)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<T>()
                    .await?)
            }

            #[derive(Default)]
            struct RefreshResult {
                management_base_url: String,
                api_version: Option<u32>,
                service_name: Option<String>,
                active: Vec<ActiveRequest>,
                recent: Vec<FinishedRequest>,
                session_cards: Vec<SessionIdentityCard>,
                global_override: Option<String>,
                default_profile: Option<String>,
                profiles: Vec<ControlProfileOption>,
                session_model: HashMap<String, String>,
                session_cfg: HashMap<String, String>,
                session_effort: HashMap<String, String>,
                session_service_tier: HashMap<String, String>,
                session_stats: HashMap<String, SessionStats>,
                configs: Vec<ConfigOption>,
                config_health: HashMap<String, ConfigHealth>,
                health_checks: HashMap<String, HealthCheckStatus>,
                usage_rollup: UsageRollupView,
                stats_5m: WindowStats,
                stats_1h: WindowStats,
                lb_view: HashMap<String, LbConfigView>,
                runtime_loaded_at_ms: Option<u64>,
                runtime_source_mtime_ms: Option<u64>,
            }

            async fn refresh_from_base(
                client: &Client,
                base: &str,
                req_timeout: Duration,
            ) -> anyhow::Result<RefreshResult> {
                let caps = get_json::<ApiCapabilities>(
                    client,
                    format!("{base}/__codex_helper/api/v1/capabilities"),
                    req_timeout,
                )
                .await;
                let supports_v1 = matches!(caps.as_ref(), Ok(c) if c.api_version == 1);

                if supports_v1 {
                    let caps = caps.expect("checked ok above");
                    let supports_snapshot = caps
                        .endpoints
                        .iter()
                        .any(|e| e == "/__codex_helper/api/v1/snapshot");

                    if supports_snapshot {
                        let api = get_json::<ApiV1Snapshot>(
                            client,
                            format!(
                                "{base}/__codex_helper/api/v1/snapshot?recent_limit=600&stats_days=21"
                            ),
                            req_timeout,
                        )
                        .await?;

                        return Ok(RefreshResult {
                            management_base_url: base.to_string(),
                            api_version: Some(api.api_version),
                            service_name: Some(api.service_name),
                            active: api.snapshot.active,
                            recent: api.snapshot.recent,
                            session_cards: api.snapshot.session_cards,
                            global_override: api.snapshot.global_override,
                            default_profile: api.default_profile,
                            profiles: api.profiles,
                            session_model: api.snapshot.session_model_overrides,
                            session_cfg: api.snapshot.session_config_overrides,
                            session_effort: api.snapshot.session_effort_overrides,
                            session_service_tier: api.snapshot.session_service_tier_overrides,
                            session_stats: api.snapshot.session_stats,
                            configs: api.configs,
                            config_health: api.snapshot.config_health,
                            health_checks: api.snapshot.health_checks,
                            usage_rollup: api.snapshot.usage_rollup,
                            stats_5m: api.snapshot.stats_5m,
                            stats_1h: api.snapshot.stats_1h,
                            lb_view: api.snapshot.lb_view,
                            runtime_loaded_at_ms: api.runtime_loaded_at_ms,
                            runtime_source_mtime_ms: api.runtime_source_mtime_ms,
                        });
                    }

                    let (
                        active,
                        recent,
                        runtime,
                        global_override,
                        session_cfg,
                        session_effort,
                        stats,
                        configs,
                        config_health,
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
                        get_json::<RuntimeConfigStatus>(
                            client,
                            format!("{base}/__codex_helper/api/v1/config/runtime"),
                            req_timeout,
                        ),
                        get_json::<Option<String>>(
                            client,
                            format!("{base}/__codex_helper/api/v1/overrides/global-config"),
                            req_timeout,
                        ),
                        get_json::<HashMap<String, String>>(
                            client,
                            format!("{base}/__codex_helper/api/v1/overrides/session/config"),
                            req_timeout,
                        ),
                        get_json::<HashMap<String, String>>(
                            client,
                            format!("{base}/__codex_helper/api/v1/overrides/session/effort"),
                            req_timeout,
                        ),
                        get_json::<HashMap<String, SessionStats>>(
                            client,
                            format!("{base}/__codex_helper/api/v1/status/session-stats"),
                            req_timeout,
                        ),
                        get_json::<Vec<ConfigOption>>(
                            client,
                            format!("{base}/__codex_helper/api/v1/configs"),
                            req_timeout,
                        ),
                        get_json::<HashMap<String, ConfigHealth>>(
                            client,
                            format!("{base}/__codex_helper/api/v1/status/config-health"),
                            req_timeout,
                        ),
                        get_json::<HashMap<String, HealthCheckStatus>>(
                            client,
                            format!("{base}/__codex_helper/api/v1/status/health-checks"),
                            req_timeout,
                        ),
                    )?;

                    let supports_session_model = caps
                        .endpoints
                        .iter()
                        .any(|e| e == "/__codex_helper/api/v1/overrides/session/model");
                    let supports_session_service_tier = caps
                        .endpoints
                        .iter()
                        .any(|e| e == "/__codex_helper/api/v1/overrides/session/service-tier");
                    let supports_profiles = caps
                        .endpoints
                        .iter()
                        .any(|e| e == "/__codex_helper/api/v1/profiles");

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
                            format!("{base}/__codex_helper/api/v1/overrides/session/service-tier"),
                            req_timeout,
                        )
                        .await
                        .ok()
                        .unwrap_or_default()
                    } else {
                        HashMap::new()
                    };

                    let (default_profile, profiles) = if supports_profiles {
                        #[derive(serde::Deserialize)]
                        struct ProfilesResponse {
                            default_profile: Option<String>,
                            #[serde(default)]
                            profiles: Vec<ControlProfileOption>,
                        }

                        let response = get_json::<ProfilesResponse>(
                            client,
                            format!("{base}/__codex_helper/api/v1/profiles"),
                            req_timeout,
                        )
                        .await
                        .ok();
                        match response {
                            Some(response) => (response.default_profile, response.profiles),
                            None => (None, Vec::new()),
                        }
                    } else {
                        (None, Vec::new())
                    };

                    return Ok(RefreshResult {
                        management_base_url: base.to_string(),
                        api_version: Some(caps.api_version),
                        service_name: Some(caps.service_name),
                        active,
                        recent,
                        session_cards: Vec::new(),
                        global_override,
                        default_profile,
                        profiles,
                        session_model,
                        session_cfg,
                        session_effort,
                        session_service_tier,
                        session_stats: stats,
                        configs,
                        config_health,
                        health_checks,
                        usage_rollup: UsageRollupView::default(),
                        stats_5m: WindowStats::default(),
                        stats_1h: WindowStats::default(),
                        lb_view: HashMap::new(),
                        runtime_loaded_at_ms: Some(runtime.loaded_at_ms),
                        runtime_source_mtime_ms: runtime.source_mtime_ms,
                    });
                }

                let (active, recent, runtime) = tokio::try_join!(
                    get_json::<Vec<ActiveRequest>>(
                        client,
                        format!("{base}/__codex_helper/status/active"),
                        req_timeout,
                    ),
                    get_json::<Vec<FinishedRequest>>(
                        client,
                        format!("{base}/__codex_helper/status/recent?limit=200"),
                        req_timeout,
                    ),
                    get_json::<RuntimeConfigStatus>(
                        client,
                        format!("{base}/__codex_helper/config/runtime"),
                        req_timeout,
                    ),
                )?;

                let session_effort = get_json::<HashMap<String, String>>(
                    client,
                    format!("{base}/__codex_helper/override/session"),
                    req_timeout,
                )
                .await
                .ok()
                .unwrap_or_default();

                Ok(RefreshResult {
                    management_base_url: base.to_string(),
                    api_version: None,
                    service_name: None,
                    active,
                    recent,
                    session_cards: Vec::new(),
                    global_override: None,
                    default_profile: None,
                    profiles: Vec::new(),
                    session_model: HashMap::new(),
                    session_cfg: HashMap::new(),
                    session_effort,
                    session_service_tier: HashMap::new(),
                    session_stats: HashMap::new(),
                    configs: Vec::new(),
                    config_health: HashMap::new(),
                    health_checks: HashMap::new(),
                    usage_rollup: UsageRollupView::default(),
                    stats_5m: WindowStats::default(),
                    stats_1h: WindowStats::default(),
                    lb_view: HashMap::new(),
                    runtime_loaded_at_ms: Some(runtime.loaded_at_ms),
                    runtime_source_mtime_ms: runtime.source_mtime_ms,
                })
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
                    att.global_override = result.global_override;
                    att.default_profile = result.default_profile;
                    att.profiles = result.profiles;
                    att.session_model_overrides = result.session_model;
                    att.session_config_overrides = result.session_cfg;
                    att.session_effort_overrides = result.session_effort;
                    att.session_service_tier_overrides = result.session_service_tier;
                    att.session_stats = result.session_stats;
                    att.configs = result.configs;
                    att.config_health = result.config_health;
                    att.health_checks = result.health_checks;
                    att.usage_rollup = result.usage_rollup;
                    att.stats_5m = result.stats_5m;
                    att.stats_1h = result.stats_1h;
                    att.lb_view = result.lb_view;
                    att.runtime_loaded_at_ms = result.runtime_loaded_at_ms;
                    att.runtime_source_mtime_ms = result.runtime_source_mtime_ms;
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
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let supports_v1 = att.api_version == Some(1);
                let fut = async move {
                    let url = if supports_v1 {
                        format!("{base}/__codex_helper/api/v1/overrides/session/effort")
                    } else {
                        format!("{base}/__codex_helper/override/session")
                    };
                    client
                        .post(url)
                        .timeout(Duration::from_millis(800))
                        .json(&serde_json::json!({
                            "session_id": session_id,
                            "effort": effort,
                        }))
                        .send()
                        .await?
                        .error_for_status()?;
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
                    client
                        .post(format!(
                            "{base}/__codex_helper/api/v1/overrides/session/model"
                        ))
                        .timeout(Duration::from_millis(800))
                        .json(&serde_json::json!({
                            "session_id": session_id,
                            "model": model,
                        }))
                        .send()
                        .await?
                        .error_for_status()?;
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
                    let profile = mgr
                        .profile(profile_name.as_str())
                        .with_context(|| format!("profile not found: {profile_name}"))?;
                    state
                        .set_session_binding(crate::state::SessionBinding {
                            session_id: session_id.clone(),
                            profile_name: Some(profile_name),
                            station_name: profile.station.clone(),
                            model: profile.model.clone(),
                            reasoning_effort: profile.reasoning_effort.clone(),
                            service_tier: profile.service_tier.clone(),
                            continuity_mode: crate::state::SessionContinuityMode::ManualProfile,
                            created_at_ms: now,
                            updated_at_ms: now,
                            last_seen_ms: now,
                        })
                        .await;
                    state.clear_session_config_override(&session_id).await;
                    state.clear_session_model_override(&session_id).await;
                    state.clear_session_effort_override(&session_id).await;
                    state.clear_session_service_tier_override(&session_id).await;

                    Ok::<(), anyhow::Error>(())
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
                    client
                        .post(format!(
                            "{base}/__codex_helper/api/v1/overrides/session/profile"
                        ))
                        .timeout(Duration::from_millis(1200))
                        .json(&serde_json::json!({
                            "session_id": session_id,
                            "profile_name": profile_name,
                        }))
                        .send()
                        .await?
                        .error_for_status()?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_session_config_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        session_id: String,
        config_name: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match config_name {
                        Some(name) => {
                            state
                                .set_session_config_override(session_id, name, now)
                                .await
                        }
                        None => state.clear_session_config_override(&session_id).await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support session config overrides (need api v1)");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    client
                        .post(format!(
                            "{base}/__codex_helper/api/v1/overrides/session/config"
                        ))
                        .timeout(Duration::from_millis(800))
                        .json(&serde_json::json!({
                            "session_id": session_id,
                            "config_name": config_name,
                        }))
                        .send()
                        .await?
                        .error_for_status()?;
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
                    client
                        .post(format!(
                            "{base}/__codex_helper/api/v1/overrides/session/service-tier"
                        ))
                        .timeout(Duration::from_millis(800))
                        .json(&serde_json::json!({
                            "session_id": session_id,
                            "service_tier": service_tier,
                        }))
                        .send()
                        .await?
                        .error_for_status()?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn apply_global_config_override(
        &mut self,
        rt: &tokio::runtime::Runtime,
        config_name: Option<String>,
    ) -> anyhow::Result<()> {
        match &mut self.mode {
            ProxyMode::Running(r) => {
                let state = r.state.clone();
                let now = now_ms();
                rt.block_on(async move {
                    match config_name {
                        Some(name) => state.set_global_config_override(name, now).await,
                        None => state.clear_global_config_override().await,
                    }
                });
                Ok(())
            }
            ProxyMode::Attached(att) => {
                if att.api_version != Some(1) {
                    bail!("attached proxy does not support global config override (need api v1)");
                }
                let base = att.admin_base_url.clone();
                let client = self.http_client.clone();
                let fut = async move {
                    client
                        .post(format!(
                            "{base}/__codex_helper/api/v1/overrides/global-config"
                        ))
                        .timeout(Duration::from_millis(800))
                        .json(&serde_json::json!({ "config_name": config_name }))
                        .send()
                        .await?
                        .error_for_status()?;
                    Ok::<(), anyhow::Error>(())
                };
                rt.block_on(fut)?;
                Ok(())
            }
            _ => bail!("proxy is not running/attached"),
        }
    }

    pub fn reload_runtime_config(&mut self, rt: &tokio::runtime::Runtime) -> anyhow::Result<()> {
        let (base, supports_v1) = match &self.mode {
            ProxyMode::Running(r) => (local_proxy_base_url(r.admin_port), true),
            ProxyMode::Attached(a) => (a.admin_base_url.clone(), a.api_version == Some(1)),
            _ => bail!("proxy is not running/attached"),
        };

        let client = self.http_client.clone();
        let fut = async move {
            let url = if supports_v1 {
                format!("{base}/__codex_helper/api/v1/config/reload")
            } else {
                format!("{base}/__codex_helper/config/reload")
            };
            client
                .post(url)
                .timeout(Duration::from_millis(800))
                .send()
                .await?
                .error_for_status()?;
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
        config_names: Vec<String>,
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
            client
                .post(format!("{base}/__codex_helper/api/v1/healthcheck/start"))
                .timeout(Duration::from_millis(800))
                .json(&serde_json::json!({ "all": all, "config_names": config_names }))
                .send()
                .await?
                .error_for_status()?;
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
        config_names: Vec<String>,
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
            client
                .post(format!("{base}/__codex_helper/api/v1/healthcheck/cancel"))
                .timeout(Duration::from_millis(800))
                .json(&serde_json::json!({ "all": all, "config_names": config_names }))
                .send()
                .await?
                .error_for_status()?;
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
            Ok::<_, anyhow::Error>(snapshot)
        };

        match rt.block_on(fut) {
            Ok(snap) => {
                r.last_error = None;
                r.default_profile = match r.service_name {
                    "claude" => r.cfg.claude.default_profile.clone(),
                    _ => r.cfg.codex.default_profile.clone(),
                };
                r.profiles = list_profiles_from_cfg(r.cfg.as_ref(), r.service_name);
                r.active = snap.active;
                r.recent = snap.recent;
                r.session_cards = snap.session_cards;
                r.global_override = snap.global_override;
                r.session_model_overrides = snap.session_model_overrides;
                r.session_config_overrides = snap.session_config_overrides;
                r.session_effort_overrides = snap.session_effort_overrides;
                r.session_service_tier_overrides = snap.session_service_tier_overrides;
                r.session_stats = snap.session_stats;
                r.config_health = snap.config_health;
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
                if cfg.codex.configs.is_empty() || cfg.codex.active_config().is_none() {
                    anyhow::bail!(
                        "No valid Codex upstream config; please configure ~/.codex-helper/config.toml (or config.json) first"
                    );
                }
            } else if cfg.claude.configs.is_empty() || cfg.claude.active_config().is_none() {
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
        let profiles = list_profiles_from_cfg(cfg.as_ref(), service_name);

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
            global_override: None,
            default_profile,
            profiles,
            session_model_overrides: HashMap::new(),
            session_config_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_service_tier_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            config_health: HashMap::new(),
            health_checks: HashMap::new(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
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

fn list_configs_from_cfg(cfg: &ProxyConfig, service_name: &str) -> Vec<ConfigOption> {
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    let mut out = mgr
        .configs
        .iter()
        .map(|(name, c)| ConfigOption {
            name: name.clone(),
            alias: c.alias.clone(),
            enabled: c.enabled,
            level: c.level.clamp(1, 10),
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
    out
}

fn list_profiles_from_cfg(cfg: &ProxyConfig, service_name: &str) -> Vec<ControlProfileOption> {
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    let default_name = mgr.default_profile.as_deref();
    let mut out = mgr
        .profiles
        .iter()
        .map(|(name, profile)| ControlProfileOption {
            name: name.clone(),
            station: profile.station.clone(),
            model: profile.model.clone(),
            reasoning_effort: profile.reasoning_effort.clone(),
            service_tier: profile.service_tier.clone(),
            is_default: default_name == Some(name.as_str()),
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
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
