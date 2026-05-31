mod config;

use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use codex_helper_core::config::load_or_bootstrap_for_service_with_v4_source;
use codex_helper_core::runtime_host::{
    build_proxy_runtime_from_loaded_with_options, service_name_for_kind,
};
use config::{EffectiveServerConfig, ServerConfigOverrides, ServerService, load_server_config};
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
    /// Admin API base URL advertised to proxy clients.
    #[arg(long)]
    advertised_admin_base_url: Option<String>,
    /// Enable host-local Codex session history enrichment for this server process.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    host_local_session_history: Option<bool>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let file_config = load_server_config(cli.config.as_deref())?;
    let effective = EffectiveServerConfig::from_sources(
        file_config,
        ServerConfigOverrides {
            service: cli.service,
            host: cli.host,
            port: cli.port,
            admin_host: cli.admin_host,
            admin_port: cli.admin_port,
            advertised_admin_base_url: cli.advertised_admin_base_url,
            host_local_session_history: cli.host_local_session_history,
        },
    )?;
    let service_kind = effective.service_kind();
    let service_name = service_name_for_kind(service_kind);
    let admin_addr = effective.admin_addr();

    let loaded = load_or_bootstrap_for_service_with_v4_source(service_kind)
        .await
        .with_context(|| format!("load {service_name} proxy config"))?;
    let runtime = build_proxy_runtime_from_loaded_with_options(
        service_name,
        effective.host,
        effective.port,
        effective.runtime_options(),
        loaded,
    )
    .await
    .with_context(|| {
        format!(
            "start {service_name} proxy on {}:{} with admin on {}",
            effective.host, effective.port, admin_addr
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
        effective.host,
        effective.port,
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
