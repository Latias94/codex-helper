mod config;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use codex_helper_core::config::{ServiceKind, load_or_bootstrap_for_service_with_v4_source};
use codex_helper_core::host_local::{
    HostLocalSessionHistoryMode, set_host_local_session_history_mode,
};
use codex_helper_core::proxy::admin_port_for_proxy_port;
use codex_helper_core::runtime_host::{
    build_proxy_runtime_from_loaded_with_admin_addr, service_name_for_kind,
};
use config::{ServerService, load_server_config};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "codex-helper-server", version)]
#[command(about = "Container-oriented codex-helper central relay runtime", long_about = None)]
struct Cli {
    /// Optional server deployment config. CLI flags override matching file values.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Target upstream service exposed by this relay.
    #[arg(long, value_enum)]
    service: Option<ServerService>,
    /// Proxy listen host. Container deployments normally use 0.0.0.0.
    #[arg(long)]
    host: Option<IpAddr>,
    /// Proxy listen port. Defaults to 3211 for Codex and 3210 for Claude.
    #[arg(long)]
    port: Option<u16>,
    /// Admin listen host. Defaults to 127.0.0.1 unless explicitly exposed.
    #[arg(long)]
    admin_host: Option<IpAddr>,
    /// Admin listen port. Defaults to proxy port + 1000.
    #[arg(long)]
    admin_port: Option<u16>,
    /// Enable host-local Codex session history enrichment for this server process.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    host_local_session_history: Option<bool>,
}

impl From<ServerService> for ServiceKind {
    fn from(value: ServerService) -> Self {
        match value {
            ServerService::Codex => Self::Codex,
            ServerService::Claude => Self::Claude,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let file_config = load_server_config(cli.config.as_deref())?;
    let service = cli
        .service
        .or(file_config.service)
        .unwrap_or(ServerService::Codex);
    let service_kind = ServiceKind::from(service);
    let service_name = service_name_for_kind(service_kind);
    let host = cli
        .host
        .or(file_config.host)
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let port = cli
        .port
        .or(file_config.port)
        .unwrap_or_else(|| default_proxy_port_for_service(service_kind));
    let admin_host = cli
        .admin_host
        .or(file_config.admin_host)
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let admin_port = cli
        .admin_port
        .or(file_config.admin_port)
        .unwrap_or_else(|| admin_port_for_proxy_port(port));
    let admin_addr = SocketAddr::from((admin_host, admin_port));
    let host_local_session_history = cli
        .host_local_session_history
        .or(file_config.host_local_session_history)
        .unwrap_or(false);

    set_host_local_session_history_mode(if host_local_session_history {
        HostLocalSessionHistoryMode::Enabled
    } else {
        HostLocalSessionHistoryMode::Disabled
    });

    let loaded = load_or_bootstrap_for_service_with_v4_source(service_kind)
        .await
        .with_context(|| format!("load {service_name} proxy config"))?;
    let runtime = build_proxy_runtime_from_loaded_with_admin_addr(
        service_name,
        host,
        port,
        admin_addr,
        loaded,
    )
    .await
    .with_context(|| {
        format!(
            "start {service_name} proxy on {}:{} with admin on {}",
            host, port, admin_addr
        )
    })?;

    runtime.proxy.spawn_initial_balance_refresh();
    let shutdown_tx = runtime.shutdown_tx.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });

    tracing::info!(
        "codex-helper server listening on http://{}:{} (service: {})",
        host,
        port,
        service_name
    );
    tracing::info!(
        "codex-helper server admin API listening on http://{} (service: {})",
        admin_addr,
        service_name
    );

    runtime.start().wait().await
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

fn default_proxy_port_for_service(service_kind: ServiceKind) -> u16 {
    match service_kind {
        ServiceKind::Codex => 3211,
        ServiceKind::Claude => 3210,
    }
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) {
            (Ok(mut sigint), Ok(mut sigterm)) => {
                tokio::select! {
                    _ = sigint.recv() => {},
                    _ = sigterm.recv() => {},
                }
            }
            _ => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
