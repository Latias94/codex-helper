use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::Router;
use reqwest::Client;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::{
    LoadedProxyConfig, ProxyConfig, ServiceKind, load_or_bootstrap_for_service_with_v4_source,
    model_routing_warnings,
};
use crate::lb::LbState;
use crate::proxy::{
    ProxyService, admin_listener_router, admin_loopback_addr_for_proxy_port, local_proxy_base_url,
    proxy_only_router_with_admin_base_url,
};
use crate::state::ProxyState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyLifetimeMode {
    Ephemeral,
    Resident,
}

impl ProxyLifetimeMode {
    pub fn is_resident(self) -> bool {
        matches!(self, Self::Resident)
    }
}

pub struct ProxyRuntime {
    pub service_name: &'static str,
    pub host: IpAddr,
    pub port: u16,
    pub admin_addr: SocketAddr,
    pub config: Arc<ProxyConfig>,
    pub proxy: ProxyService,
    pub state: Arc<ProxyState>,
    pub shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    listener: Option<tokio::net::TcpListener>,
    admin_listener: Option<tokio::net::TcpListener>,
    app: Option<Router>,
    admin_app: Option<Router>,
}

pub struct RunningProxyRuntime {
    pub service_name: &'static str,
    pub host: IpAddr,
    pub port: u16,
    pub admin_addr: SocketAddr,
    pub config: Arc<ProxyConfig>,
    pub proxy: ProxyService,
    pub state: Arc<ProxyState>,
    pub shutdown_tx: watch::Sender<bool>,
    pub server_handle: JoinHandle<Result<()>>,
}

impl ProxyRuntime {
    pub fn shutdown_receiver(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    pub fn start(mut self) -> RunningProxyRuntime {
        let listener = self
            .listener
            .take()
            .expect("proxy runtime listener should exist before start");
        let admin_listener = self
            .admin_listener
            .take()
            .expect("proxy runtime admin listener should exist before start");
        let app = self
            .app
            .take()
            .expect("proxy runtime app should exist before start");
        let admin_app = self
            .admin_app
            .take()
            .expect("proxy runtime admin app should exist before start");
        let shutdown_rx = self.shutdown_rx.clone();
        let server_handle =
            spawn_proxy_runtime_servers(listener, admin_listener, app, admin_app, shutdown_rx);

        RunningProxyRuntime {
            service_name: self.service_name,
            host: self.host,
            port: self.port,
            admin_addr: self.admin_addr,
            config: self.config,
            proxy: self.proxy,
            state: self.state,
            shutdown_tx: self.shutdown_tx,
            server_handle,
        }
    }
}

impl RunningProxyRuntime {
    pub async fn wait(self) -> Result<()> {
        self.server_handle
            .await
            .map_err(|error| anyhow::anyhow!("server task join error: {error}"))?
    }
}

pub async fn build_proxy_runtime(
    service_kind: ServiceKind,
    host: IpAddr,
    port: u16,
) -> Result<ProxyRuntime> {
    let service_name = service_name_for_kind(service_kind);
    let loaded = load_or_bootstrap_for_service_with_v4_source(service_kind).await?;
    build_proxy_runtime_from_loaded(service_name, host, port, loaded).await
}

pub async fn build_proxy_runtime_from_loaded(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    loaded: LoadedProxyConfig,
) -> Result<ProxyRuntime> {
    let addr: SocketAddr = SocketAddr::from((host, port));
    let admin_addr = admin_loopback_addr_for_proxy_port(port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let admin_listener = tokio::net::TcpListener::bind(admin_addr).await?;
    build_proxy_runtime_from_bound_listeners(
        service_name,
        host,
        port,
        loaded,
        listener,
        admin_listener,
    )
    .await
}

pub async fn build_proxy_runtime_from_bound_listeners(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    loaded: LoadedProxyConfig,
    listener: tokio::net::TcpListener,
    admin_listener: tokio::net::TcpListener,
) -> Result<ProxyRuntime> {
    let cfg = loaded.runtime;
    validate_service_has_upstream(service_name, &cfg)?;
    let v4_source = loaded.v4.map(Arc::new);

    let warnings = model_routing_warnings(&cfg, service_name);
    if !warnings.is_empty() {
        tracing::warn!("======== Model routing config warnings ========");
        for warning in warnings {
            tracing::warn!("{}", warning);
        }
        tracing::warn!("==============================================");
    }

    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        .build()?;

    let lb_states = Arc::new(Mutex::new(HashMap::<String, LbState>::new()));
    let admin_addr = admin_loopback_addr_for_proxy_port(port);
    let cfg = Arc::new(cfg);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let proxy = ProxyService::new_with_v4_source_and_shutdown(
        client,
        cfg.clone(),
        v4_source,
        service_name,
        lb_states,
        Some(shutdown_tx.clone()),
    );
    let state = proxy.state_handle();
    let app = proxy_only_router_with_admin_base_url(
        proxy.clone(),
        Some(local_proxy_base_url(admin_addr.port())),
    );
    let admin_app = admin_listener_router(proxy.clone());

    Ok(ProxyRuntime {
        service_name,
        host,
        port,
        admin_addr,
        config: cfg,
        proxy,
        state,
        shutdown_tx,
        shutdown_rx,
        listener: Some(listener),
        admin_listener: Some(admin_listener),
        app: Some(app),
        admin_app: Some(admin_app),
    })
}

pub fn service_name_for_kind(service_kind: ServiceKind) -> &'static str {
    match service_kind {
        ServiceKind::Codex => "codex",
        ServiceKind::Claude => "claude",
    }
}

pub fn service_kind_for_name(service_name: &str) -> ServiceKind {
    match service_name {
        "claude" => ServiceKind::Claude,
        _ => ServiceKind::Codex,
    }
}

pub fn validate_service_has_upstream(service_name: &str, cfg: &ProxyConfig) -> Result<()> {
    match service_name {
        "claude" => {
            if !cfg.claude.has_stations() || cfg.claude.active_station().is_none() {
                anyhow::bail!(
                    "未找到任何可用的 Claude 上游配置，请先确保 ~/.claude/settings.json 配置完整，\
或在 ~/.codex-helper/config.toml（或 config.json）的 `claude` 段下手动添加上游配置"
                );
            }
        }
        _ => {
            if !cfg.codex.has_stations() || cfg.codex.active_station().is_none() {
                anyhow::bail!(
                    "未找到任何可用的 Codex 上游配置，请先确保 ~/.codex/config.toml 与 ~/.codex/auth.json 配置完整，或手动编辑 ~/.codex-helper/config.toml（或 config.json）添加配置"
                );
            }
        }
    }
    Ok(())
}

fn spawn_proxy_runtime_servers(
    listener: tokio::net::TcpListener,
    admin_listener: tokio::net::TcpListener,
    app: Router,
    admin_app: Router,
    shutdown_rx: watch::Receiver<bool>,
) -> JoinHandle<Result<()>> {
    let proxy_server_shutdown = {
        let mut rx = shutdown_rx.clone();
        async move {
            let _ = rx.changed().await;
        }
    };
    let admin_server_shutdown = {
        let mut rx = shutdown_rx;
        async move {
            let _ = rx.changed().await;
        }
    };

    tokio::spawn(async move {
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
        )?;
        Ok(())
    })
}
