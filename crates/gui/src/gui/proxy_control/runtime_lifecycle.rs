use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use reqwest::Client;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::{
    ProxyConfig, ServiceKind, load_or_bootstrap_for_service, model_routing_warnings,
};
use crate::proxy::{
    ProxyService, admin_listener_router, admin_port_for_proxy_port, local_proxy_base_url,
    proxy_only_router_with_admin_base_url,
};
use crate::state::{ProxyState, UsageRollupView};

use super::running_refresh::{list_profiles_from_cfg, list_stations_from_cfg};
use super::{
    PortInUseAction, PortInUseModal, ProxyController, ProxyMode, RunningProxy, WindowStats,
};

fn is_addr_in_use(err: &anyhow::Error) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err.as_ref());
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && io.kind() == std::io::ErrorKind::AddrInUse
        {
            return true;
        }
        cur = err.source();
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

impl ProxyController {
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
            Err(err) => {
                if is_addr_in_use(&err) {
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
                            let _ = self.try_start(rt, service, suggested).map_err(|err| {
                                self.last_start_error = Some(err.to_string());
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
                    self.last_start_error = Some(err.to_string());
                }
            }
        }
    }

    pub fn confirm_port_in_use_attach(&mut self) {
        let Some(modal) = self.port_in_use_modal.as_ref() else {
            return;
        };
        self.request_attach(modal.port);
    }

    pub fn confirm_port_in_use_new_port(&mut self, rt: &tokio::runtime::Runtime) {
        let Some(modal) = self.port_in_use_modal.as_ref() else {
            return;
        };
        let port = modal.chosen_new_port;
        self.desired_port = port;
        self.port_in_use_modal = None;
        if let Err(err) = self.try_start(rt, self.desired_service, port) {
            self.last_start_error = Some(err.to_string());
        }
    }

    pub fn confirm_port_in_use_exit(&mut self) {
        self.port_in_use_modal = None;
        self.last_start_error = Some("port already in use; user chose exit".to_string());
        self.mode = ProxyMode::Stopped;
    }

    pub fn set_port_in_use_modal_remember(&mut self, value: bool) {
        if let Some(modal) = self.port_in_use_modal.as_mut() {
            modal.remember_choice = value;
        }
    }

    pub fn port_in_use_modal_remember(&self) -> bool {
        self.port_in_use_modal
            .as_ref()
            .map(|modal| modal.remember_choice)
            .unwrap_or(false)
    }

    pub fn set_port_in_use_modal_new_port(&mut self, port: u16) {
        if let Some(modal) = self.port_in_use_modal.as_mut() {
            modal.chosen_new_port = port;
        }
    }

    pub fn port_in_use_modal_suggested_port(&self) -> Option<u16> {
        self.port_in_use_modal
            .as_ref()
            .map(|modal| modal.chosen_new_port)
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
                for warning in warnings {
                    tracing::warn!("{}", warning);
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
                .map_err(|err| anyhow::anyhow!("serve error: {err}"))?;
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
            providers: Vec::new(),
            session_model_overrides: HashMap::new(),
            session_station_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_service_tier_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            stations,
            station_health: HashMap::new(),
            provider_balances: HashMap::new(),
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
