use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use reqwest::Client;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use std::collections::HashMap;

use crate::config::{
    ProxyConfig, ServiceKind, load_or_bootstrap_for_service, model_routing_warnings,
};
use crate::proxy::ProxyService;
use crate::state::{
    ActiveRequest, ConfigHealth, FinishedRequest, HealthCheckStatus, ProxyState, SessionStats,
};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GuiConfigOption {
    pub name: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub level: u8,
}

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
    pub port: u16,
    pub last_refresh: Option<Instant>,
    pub last_error: Option<String>,
    pub api_version: Option<u32>,
    pub service_name: Option<String>,
    pub active: Vec<ActiveRequest>,
    pub recent: Vec<FinishedRequest>,
    pub global_override: Option<String>,
    pub session_config_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub configs: Vec<GuiConfigOption>,
    pub config_health: HashMap<String, ConfigHealth>,
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub runtime_loaded_at_ms: Option<u64>,
    pub runtime_source_mtime_ms: Option<u64>,
}

impl AttachedStatus {
    fn new(port: u16) -> Self {
        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            port,
            last_refresh: None,
            last_error: None,
            api_version: None,
            service_name: None,
            active: Vec::new(),
            recent: Vec::new(),
            global_override: None,
            session_config_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            configs: Vec::new(),
            config_health: HashMap::new(),
            health_checks: HashMap::new(),
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
    pub global_override: Option<String>,
    pub session_config_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub configs: Vec<GuiConfigOption>,
    pub runtime_loaded_at_ms: Option<u64>,
    pub runtime_source_mtime_ms: Option<u64>,
    pub supports_v1: bool,
}

pub struct RunningProxy {
    pub service_name: &'static str,
    pub port: u16,
    pub state: Arc<ProxyState>,
    pub cfg: Arc<ProxyConfig>,
    pub last_refresh: Option<Instant>,
    pub last_error: Option<String>,
    pub active: Vec<ActiveRequest>,
    pub recent: Vec<FinishedRequest>,
    pub global_override: Option<String>,
    pub session_config_overrides: HashMap<String, String>,
    pub session_effort_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
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
}

struct PortInUseModal {
    port: u16,
    remember_choice: bool,
    chosen_new_port: u16,
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

    pub fn running(&self) -> Option<&RunningProxy> {
        match &self.mode {
            ProxyMode::Running(r) => Some(r),
            _ => None,
        }
    }

    pub fn running_mut(&mut self) -> Option<&mut RunningProxy> {
        match &mut self.mode {
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

    pub fn attached_mut(&mut self) -> Option<&mut AttachedStatus> {
        match &mut self.mode {
            ProxyMode::Attached(s) => Some(s),
            _ => None,
        }
    }

    pub fn snapshot(&self) -> Option<GuiRuntimeSnapshot> {
        match &self.mode {
            ProxyMode::Running(r) => Some(GuiRuntimeSnapshot {
                kind: ProxyModeKind::Running,
                base_url: Some(format!("http://127.0.0.1:{}", r.port)),
                port: Some(r.port),
                service_name: Some(r.service_name.to_string()),
                last_error: r.last_error.clone(),
                active: r.active.clone(),
                recent: r.recent.clone(),
                global_override: r.global_override.clone(),
                session_config_overrides: r.session_config_overrides.clone(),
                session_effort_overrides: r.session_effort_overrides.clone(),
                session_stats: r.session_stats.clone(),
                configs: list_configs_from_cfg(r.cfg.as_ref(), r.service_name),
                runtime_loaded_at_ms: None,
                runtime_source_mtime_ms: None,
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
                global_override: a.global_override.clone(),
                session_config_overrides: a.session_config_overrides.clone(),
                session_effort_overrides: a.session_effort_overrides.clone(),
                session_stats: a.session_stats.clone(),
                configs: a.configs.clone(),
                runtime_loaded_at_ms: a.runtime_loaded_at_ms,
                runtime_source_mtime_ms: a.runtime_source_mtime_ms,
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
        self.mode = ProxyMode::Attached(AttachedStatus::new(port));
        self.last_start_error = None;
        self.port_in_use_modal = None;
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
        let base = match &mut self.mode {
            ProxyMode::Attached(att) => {
                if let Some(t) = att.last_refresh
                    && t.elapsed() < refresh_every
                {
                    return;
                }
                att.last_refresh = Some(Instant::now());
                att.base_url.clone()
            }
            _ => return,
        };

        let client = self.http_client.clone();
        let fut = async move {
            #[derive(Debug, serde::Deserialize)]
            struct ApiCapabilities {
                api_version: u32,
                service_name: String,
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

            let caps = get_json::<ApiCapabilities>(
                &client,
                format!("{base}/__codex_helper/api/v1/capabilities"),
                req_timeout,
            )
            .await;
            let supports_v1 = matches!(caps.as_ref(), Ok(c) if c.api_version == 1);

            if supports_v1 {
                let caps = caps.expect("checked ok above");
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
                        &client,
                        format!("{base}/__codex_helper/api/v1/status/active"),
                        req_timeout,
                    ),
                    get_json::<Vec<FinishedRequest>>(
                        &client,
                        format!("{base}/__codex_helper/api/v1/status/recent?limit=200"),
                        req_timeout,
                    ),
                    get_json::<RuntimeConfigStatus>(
                        &client,
                        format!("{base}/__codex_helper/api/v1/config/runtime"),
                        req_timeout,
                    ),
                    get_json::<Option<String>>(
                        &client,
                        format!("{base}/__codex_helper/api/v1/overrides/global-config"),
                        req_timeout,
                    ),
                    get_json::<HashMap<String, String>>(
                        &client,
                        format!("{base}/__codex_helper/api/v1/overrides/session/config"),
                        req_timeout,
                    ),
                    get_json::<HashMap<String, String>>(
                        &client,
                        format!("{base}/__codex_helper/api/v1/overrides/session/effort"),
                        req_timeout,
                    ),
                    get_json::<HashMap<String, SessionStats>>(
                        &client,
                        format!("{base}/__codex_helper/api/v1/status/session-stats"),
                        req_timeout,
                    ),
                    get_json::<Vec<GuiConfigOption>>(
                        &client,
                        format!("{base}/__codex_helper/api/v1/configs"),
                        req_timeout,
                    ),
                    get_json::<HashMap<String, ConfigHealth>>(
                        &client,
                        format!("{base}/__codex_helper/api/v1/status/config-health"),
                        req_timeout,
                    ),
                    get_json::<HashMap<String, HealthCheckStatus>>(
                        &client,
                        format!("{base}/__codex_helper/api/v1/status/health-checks"),
                        req_timeout,
                    ),
                )?;

                return Ok::<_, anyhow::Error>((
                    Some(caps.api_version),
                    Some(caps.service_name),
                    active,
                    recent,
                    global_override,
                    session_cfg,
                    session_effort,
                    stats,
                    configs,
                    config_health,
                    health_checks,
                    Some(runtime.loaded_at_ms),
                    runtime.source_mtime_ms,
                ));
            }

            let (active, recent, runtime) = tokio::try_join!(
                get_json::<Vec<ActiveRequest>>(
                    &client,
                    format!("{base}/__codex_helper/status/active"),
                    req_timeout,
                ),
                get_json::<Vec<FinishedRequest>>(
                    &client,
                    format!("{base}/__codex_helper/status/recent?limit=200"),
                    req_timeout,
                ),
                get_json::<RuntimeConfigStatus>(
                    &client,
                    format!("{base}/__codex_helper/config/runtime"),
                    req_timeout,
                ),
            )?;

            let session_effort = get_json::<HashMap<String, String>>(
                &client,
                format!("{base}/__codex_helper/override/session"),
                req_timeout,
            )
            .await
            .ok()
            .unwrap_or_default();

            Ok::<_, anyhow::Error>((
                None,
                None,
                active,
                recent,
                None,
                HashMap::new(),
                session_effort,
                HashMap::new(),
                Vec::new(),
                HashMap::new(),
                HashMap::new(),
                Some(runtime.loaded_at_ms),
                runtime.source_mtime_ms,
            ))
        };

        match rt.block_on(fut) {
            Ok((
                api_version,
                service_name,
                active,
                recent,
                global_override,
                session_cfg,
                session_effort,
                stats,
                configs,
                config_health,
                health_checks,
                runtime_loaded_at_ms,
                runtime_source_mtime_ms,
            )) => {
                if let ProxyMode::Attached(att) = &mut self.mode {
                    att.last_error = None;
                    att.api_version = api_version;
                    att.service_name = service_name;
                    att.active = active;
                    att.recent = recent;
                    att.global_override = global_override;
                    att.session_config_overrides = session_cfg;
                    att.session_effort_overrides = session_effort;
                    att.session_stats = stats;
                    att.configs = configs;
                    att.config_health = config_health;
                    att.health_checks = health_checks;
                    att.runtime_loaded_at_ms = runtime_loaded_at_ms;
                    att.runtime_source_mtime_ms = runtime_source_mtime_ms;
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
                let base = att.base_url.clone();
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
                let base = att.base_url.clone();
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
                let base = att.base_url.clone();
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
            ProxyMode::Running(r) => (format!("http://127.0.0.1:{}", r.port), true),
            ProxyMode::Attached(a) => (a.base_url.clone(), a.api_version == Some(1)),
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
            ProxyMode::Running(r) => format!("http://127.0.0.1:{}", r.port),
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support health checks (need api v1)");
                }
                a.base_url.clone()
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
            ProxyMode::Running(r) => format!("http://127.0.0.1:{}", r.port),
            ProxyMode::Attached(a) => {
                if a.api_version != Some(1) {
                    bail!("attached proxy does not support health checks (need api v1)");
                }
                a.base_url.clone()
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
        let fut = async move {
            let (active, recent, global_override, session_cfg, session_effort, session_stats) = tokio::join!(
                state.list_active_requests(),
                state.list_recent_finished(200),
                state.get_global_config_override(),
                state.list_session_config_overrides(),
                state.list_session_effort_overrides(),
                state.list_session_stats(),
            );
            Ok::<
                (
                    Vec<ActiveRequest>,
                    Vec<FinishedRequest>,
                    Option<String>,
                    HashMap<String, String>,
                    HashMap<String, String>,
                    HashMap<String, SessionStats>,
                ),
                anyhow::Error,
            >((
                active,
                recent,
                global_override,
                session_cfg,
                session_effort,
                session_stats,
            ))
        };

        match rt.block_on(fut) {
            Ok((active, recent, global_override, session_cfg, session_effort, session_stats)) => {
                r.last_error = None;
                r.active = active;
                r.recent = recent;
                r.global_override = global_override;
                r.session_config_overrides = session_cfg;
                r.session_effort_overrides = session_effort;
                r.session_stats = session_stats;
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
            let app = crate::proxy::router(proxy);

            let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], port));
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .with_context(|| format!("bind {}", addr))?;

            let (shutdown_tx, shutdown_rx) = watch::channel(false);
            let server_shutdown = {
                let mut rx = shutdown_rx.clone();
                async move {
                    let _ = rx.changed().await;
                }
            };

            let handle = tokio::spawn(async move {
                axum::serve(listener, app.into_make_service())
                    .with_graceful_shutdown(server_shutdown)
                    .await
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

        self.mode = ProxyMode::Running(RunningProxy {
            service_name,
            port,
            state,
            cfg,
            last_refresh: None,
            last_error: None,
            active: Vec::new(),
            recent: Vec::new(),
            global_override: None,
            session_config_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_stats: HashMap::new(),
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

fn list_configs_from_cfg(cfg: &ProxyConfig, service_name: &str) -> Vec<GuiConfigOption> {
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    let mut out = mgr
        .configs
        .iter()
        .map(|(name, c)| GuiConfigOption {
            name: name.clone(),
            alias: c.alias.clone(),
            enabled: c.enabled,
            level: c.level.clamp(1, 10),
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
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
