mod check;
mod config;

use std::net::IpAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use codex_helper_core::config::load_config_with_source;
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
    /// Validate route credentials without opening runtime state, listeners, clients, or upstreams.
    #[arg(long)]
    check: bool,
    /// Emit the credential check as stable JSON. Requires --check.
    #[arg(long, requires = "check")]
    json: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    let file_config = load_server_config(cli.config.as_deref())?;
    let effective = EffectiveServerConfig::from_sources(
        file_config,
        ServerConfigOverrides {
            service: cli.service,
            host: cli.host,
            port: cli.port,
            admin_host: cli.admin_host,
            admin_port: cli.admin_port,
        },
    )?;
    let service_kind = effective.service_kind();
    let service_name = service_name_for_kind(service_kind);
    let admin_addr = effective.admin_addr();

    let loaded = load_config_with_source()
        .await
        .with_context(|| format!("load {service_name} proxy config"))?;
    if cli.check {
        let evaluation = check::evaluate(&loaded.source, service_kind)
            .with_context(|| format!("check {service_name} credential readiness"))?;
        let output = if cli.json {
            evaluation.render_json()?
        } else {
            evaluation.render_text()
        };
        println!("{output}");
        return Ok(if evaluation.succeeded() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
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

    let mut running_runtime = runtime.start();
    running_runtime.wait().await?;
    Ok(ExitCode::SUCCESS)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_requires_check_mode() {
        assert!(Cli::try_parse_from(["codex-helper-server", "--json"]).is_err());
        let cli = Cli::try_parse_from(["codex-helper-server", "--check", "--json"])
            .expect("parse check JSON flags");
        assert!(cli.check);
        assert!(cli.json);
    }
}
