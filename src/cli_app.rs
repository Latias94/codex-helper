use crate::cli_types::{
    Cli, CliError, CliResult, Command, DaemonCommand, NotifyCommand, RelayCommand, ServiceCommand,
    SwitchCommand,
};
use crate::codex_integration;
use crate::commands;
use crate::config::{
    LoadedConfig, RelayTargetConfig, ServiceKind, load_config, load_config_with_source,
    save_helper_config,
};
use crate::control_plane_client::{
    ControlPlaneClient, ControlPlaneEndpoint, configured_local_admin_token_env,
};
use crate::dashboard_core::{OperatorReadModel, OperatorReadStatus};
use crate::notify;
use crate::proxy::admin_loopback_addr_for_proxy_port;
use crate::runtime_host::{
    ProxyListenerBindError, ProxyRuntime, ProxyRuntimeOptions, RunningProxyRuntime,
    build_proxy_runtime_from_loaded_with_options,
};
use crate::runtime_manager::{
    RuntimeOwnerKind, RuntimeOwnerMarker, RuntimeOwnerMarkerGuard, clear_owner_marker,
    read_owner_marker_best_effort, write_owner_marker,
};
use crate::service_manager;
use crate::service_receipt::{
    ServicePlatformBackend, ServiceReceipt, ServiceReceiptError, read_service_receipt,
};
use crate::tui;
use anyhow::Context;
use codex_helper_core::relay_target::{
    ResolvedRelayTarget, default_proxy_port_for_service_kind, normalize_relay_target_name,
    relay_target_config_from_args, relay_target_names, resolve_relay_target,
};

use clap::Parser;
use codex_helper_core::codex_switch::{
    self, CodexSwitchIntent, CodexSwitchPhase, ValidatedCodexBaseUrl,
};
use codex_helper_core::credentials::CredentialSourceCapabilities;
use codex_helper_core::local_log_store::{LogRetention, RotatingLogWriter, repair_log};
use owo_colors::OwoColorize;
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::time::Duration;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

#[derive(Clone, Copy, Default)]
struct ServeRuntimeOptions {
    enable_tui: bool,
    resident: bool,
    supervisor_managed: bool,
    desktop_managed: bool,
    service_managed: bool,
}

impl ServeRuntimeOptions {
    fn owner_kind(self) -> Option<RuntimeOwnerKind> {
        if self.service_managed {
            Some(RuntimeOwnerKind::SystemService)
        } else if self.desktop_managed {
            Some(RuntimeOwnerKind::Desktop)
        } else if self.supervisor_managed {
            Some(RuntimeOwnerKind::Supervisor)
        } else if self.resident {
            Some(RuntimeOwnerKind::ManualCli)
        } else {
            None
        }
    }

    fn is_resident(self) -> bool {
        self.resident || self.supervisor_managed || self.desktop_managed || self.service_managed
    }
}

pub async fn run_cli() -> CliResult<()> {
    let cli = Cli::parse();
    if let Some(Command::Service { cmd }) = cli.command.as_ref() {
        service_manager::configure_service_command_environment(cmd)?;
    }
    let _log_guard = init_tracing(&cli);

    match cli.command.unwrap_or(Command::Serve {
        port: None,
        codex: false,
        claude: false,
        host: IpAddr::from([127, 0, 0, 1]),
        no_tui: false,
        resident: false,
        supervisor_managed: false,
        desktop_managed: false,
        service_managed: false,
    }) {
        Command::Default { codex, claude } => {
            handle_default_cmd(codex, claude).await?;
            return Ok(());
        }
        Command::Daemon { cmd } => {
            handle_daemon_cmd(cmd).await?;
            return Ok(());
        }
        Command::Service { cmd } => {
            service_manager::handle_service_command(cmd).await?;
            return Ok(());
        }
        Command::Tui {
            codex,
            claude,
            port,
        } => {
            let service_name = resolve_cli_service_name(codex, claude).await?;
            let port = port.unwrap_or_else(|| default_proxy_port_for_service(service_name));
            tui::run_local_attached_dashboard_with_admin_base_url(
                service_name,
                port,
                daemon_admin_base_url_for_proxy_port(port),
                configured_local_admin_token_env().map(str::to_string),
            )
            .await
            .map_err(|e| CliError::Other(e.to_string()))?;
            return Ok(());
        }
        Command::Relay { cmd } => {
            handle_relay_cmd(cmd).await?;
            return Ok(());
        }
        Command::Switch { cmd } => {
            match cmd {
                SwitchCommand::On { port, base_url } => do_switch_on(port, base_url)?,
                SwitchCommand::Off => do_switch_off()?,
                SwitchCommand::Status => do_switch_status()?,
            }
            return Ok(());
        }
        Command::Config { cmd } => {
            commands::config::handle_config_cmd(cmd).await?;
            return Ok(());
        }
        Command::Routing { cmd } => {
            commands::routing::handle_routing_cmd(cmd).await?;
            return Ok(());
        }
        Command::Provider { cmd } => {
            commands::provider::handle_provider_cmd(cmd).await?;
            return Ok(());
        }
        Command::Credential { cmd } => {
            commands::credential::handle_credential_cmd(cmd).await?;
            return Ok(());
        }
        Command::Codex { cmd } => {
            commands::codex::handle_codex_cmd(cmd).await?;
            return Ok(());
        }
        Command::Session { cmd } => {
            commands::session::handle_session_cmd(cmd).await?;
            return Ok(());
        }
        Command::Doctor { json } => {
            commands::doctor::handle_doctor_cmd(json).await?;
            return Ok(());
        }
        Command::Status { json } => {
            let (codex, claude) = tokio::join!(
                read_local_operator_model("codex", default_proxy_port_for_service("codex")),
                read_local_operator_model("claude", default_proxy_port_for_service("claude")),
            );
            commands::doctor::handle_status_cmd(json, &codex?, &claude?).await?;
            return Ok(());
        }
        Command::Usage {
            codex,
            claude,
            port,
            cmd,
        } => {
            let service_name = resolve_cli_service_name(codex, claude).await?;
            let port = port.unwrap_or_else(|| default_proxy_port_for_service(service_name));
            let client = local_control_plane_client(port)?;
            let model = client.refresh_operator_read_model(service_name, None).await;
            commands::usage::handle_usage_cmd(cmd, &client, model).await?;
            return Ok(());
        }
        Command::Pricing { cmd } => {
            commands::pricing::handle_pricing_cmd(cmd).await?;
            return Ok(());
        }
        Command::Notify { cmd } => {
            match cmd {
                NotifyCommand::Codex {
                    notification_json,
                    no_toast,
                    toast,
                } => notify::handle_codex_notify(notification_json, no_toast, toast).await?,
                NotifyCommand::FlushCodex => notify::handle_codex_flush().await?,
            }
            return Ok(());
        }
        Command::Serve {
            port,
            codex,
            claude,
            host,
            no_tui,
            resident,
            supervisor_managed,
            desktop_managed,
            service_managed,
        } => {
            if [supervisor_managed, desktop_managed, service_managed]
                .into_iter()
                .filter(|managed| *managed)
                .count()
                > 1
            {
                return Err(CliError::Other(
                    "--supervisor-managed, --desktop-managed, and --service-managed are mutually exclusive"
                        .to_string(),
                ));
            }
            let service_name = resolve_cli_service_name(codex, claude).await?;
            let port = port.unwrap_or_else(|| default_proxy_port_for_service(service_name));
            run_server(
                service_name,
                host,
                port,
                ServeRuntimeOptions {
                    enable_tui: !no_tui,
                    resident,
                    supervisor_managed,
                    desktop_managed,
                    service_managed,
                },
            )
            .await
            .map_err(|e| CliError::Other(e.to_string()))?;
        }
    }

    Ok(())
}

fn init_tracing(cli: &Cli) -> Option<WorkerGuard> {
    // Default to info logs unless the user sets RUST_LOG.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // When the built-in TUI is enabled, writing logs to the same terminal will cause flicker and
    // "bleeding" output. In that case, redirect tracing output to a file by default.
    let interactive_tui = match &cli.command {
        Some(Command::Serve {
            no_tui,
            resident,
            supervisor_managed,
            desktop_managed,
            service_managed,
            ..
        }) => {
            !*resident
                && !*supervisor_managed
                && !*desktop_managed
                && !*service_managed
                && !*no_tui
                && atty::is(atty::Stream::Stdin)
                && atty::is(atty::Stream::Stdout)
        }
        Some(Command::Tui { .. }) => {
            atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout)
        }
        Some(Command::Relay { cmd }) => {
            relay_command_opens_tui(cmd)
                && atty::is(atty::Stream::Stdin)
                && atty::is(atty::Stream::Stdout)
        }
        None => atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout),
        _ => false,
    };

    let service_entrypoint = matches!(
        &cli.command,
        Some(Command::Service {
            cmd: ServiceCommand::Run { .. } | ServiceCommand::TaskRun { .. }
        })
    );

    if interactive_tui || service_entrypoint {
        let log_dir = crate::config::proxy_home_dir().join("logs");
        let _ = std::fs::create_dir_all(&log_dir);

        let runtime_log_retention = LogRetention::from_env(
            "CODEX_HELPER_RUNTIME_LOG_MAX_BYTES",
            "CODEX_HELPER_RUNTIME_LOG_MAX_FILES",
            DEFAULT_RUNTIME_LOG_MAX_BYTES,
            DEFAULT_RUNTIME_LOG_MAX_FILES,
        );
        let runtime_log_path = log_dir.join(RUNTIME_LOG_FILE_NAME);
        repair_log(&runtime_log_path, runtime_log_retention);

        let file_appender = RotatingLogWriter::new(runtime_log_path, runtime_log_retention);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_ansi(false)
            .with_writer(non_blocking)
            .init();
        Some(guard)
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
        None
    }
}

const RUNTIME_LOG_FILE_NAME: &str = "runtime.log";
const DEFAULT_RUNTIME_LOG_MAX_BYTES: u64 = 20 * 1024 * 1024;
const DEFAULT_RUNTIME_LOG_MAX_FILES: usize = 10;

async fn handle_daemon_cmd(cmd: DaemonCommand) -> CliResult<()> {
    match cmd {
        DaemonCommand::Status {
            codex,
            claude,
            port,
            json,
        } => {
            let service_name = resolve_cli_service_name(codex, claude).await?;
            let port = port.unwrap_or_else(|| default_proxy_port_for_service(service_name));
            print_daemon_status(service_name, port, json).await
        }
        DaemonCommand::Supervise {
            codex,
            claude,
            host,
            port,
            max_restarts,
        } => {
            let service_name = resolve_cli_service_name(codex, claude).await?;
            let port = port.unwrap_or_else(|| default_proxy_port_for_service(service_name));
            supervise_daemon(service_name, host, port, max_restarts).await
        }
    }
}

async fn resolve_cli_service_name(codex: bool, claude: bool) -> CliResult<&'static str> {
    if codex && claude {
        return Err(CliError::Other(
            "Please specify at most one of --codex / --claude".to_string(),
        ));
    }
    if claude {
        return Ok("claude");
    }
    if codex {
        return Ok("codex");
    }

    match load_config().await {
        Ok(cfg) => Ok(match cfg.default_service {
            Some(ServiceKind::Claude) => "claude",
            _ => "codex",
        }),
        Err(err) => {
            tracing::warn!(
                "Failed to load config for default service, falling back to Codex: {}",
                err
            );
            Ok("codex")
        }
    }
}

fn default_proxy_port_for_service(service_name: &str) -> u16 {
    if service_name == "claude" { 3210 } else { 3211 }
}

fn relay_command_opens_tui(cmd: &RelayCommand) -> bool {
    match cmd {
        RelayCommand::Use { no_tui, .. } => !*no_tui,
        RelayCommand::Target(args) => !args.iter().any(|arg| arg == "--no-tui"),
        _ => false,
    }
}

async fn handle_relay_cmd(cmd: RelayCommand) -> CliResult<()> {
    match cmd {
        RelayCommand::Add {
            name,
            proxy_url,
            admin_url,
            admin_token_env,
            codex,
            claude,
        } => {
            let service = relay_service_from_flags(codex, claude)?;
            add_relay_target(name, proxy_url, admin_url, admin_token_env, service).await
        }
        RelayCommand::List => list_relay_targets().await,
        RelayCommand::Status { target, json } => relay_status(target, json).await,
        RelayCommand::Off => {
            println!("Relay off no longer changes client configuration.");
            println!("Run `codex-helper switch off` explicitly to restore Codex.");
            Ok(())
        }
        RelayCommand::Use {
            target,
            no_tui,
            attach_only,
        } => use_relay_target(target, no_tui, attach_only).await,
        RelayCommand::Target(args) => {
            let (target, no_tui, attach_only) = parse_relay_target_shorthand(args)?;
            use_relay_target(target, no_tui, attach_only).await
        }
    }
}

fn relay_service_from_flags(codex: bool, claude: bool) -> CliResult<ServiceKind> {
    if codex && claude {
        return Err(CliError::Other(
            "Please specify at most one of --codex / --claude".to_string(),
        ));
    }
    if claude {
        Ok(ServiceKind::Claude)
    } else {
        Ok(ServiceKind::Codex)
    }
}

fn parse_relay_target_shorthand(args: Vec<String>) -> CliResult<(String, bool, bool)> {
    let Some(target) = args.first().cloned() else {
        return Err(CliError::Other(
            "relay target name is required; try `ch relay local` or `ch relay use nas`".to_string(),
        ));
    };
    let mut no_tui = false;
    let mut attach_only = false;
    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "--no-tui" => no_tui = true,
            "--attach-only" => attach_only = true,
            other => {
                return Err(CliError::Other(format!(
                    "unsupported relay target flag '{other}'; supported flags are --no-tui and --attach-only"
                )));
            }
        }
    }
    Ok((target, no_tui, attach_only))
}

async fn add_relay_target(
    name: String,
    proxy_url: String,
    admin_url: Option<String>,
    admin_token_env: Option<String>,
    service: ServiceKind,
) -> CliResult<()> {
    let target =
        relay_target_config_from_args(Some(service), proxy_url, admin_url, admin_token_env)
            .map_err(|err| CliError::Other(err.to_string()))?;

    save_named_relay_target(&name, target).await?;
    println!("Saved relay target '{name}'.");
    Ok(())
}

async fn save_named_relay_target(name: &str, target: RelayTargetConfig) -> CliResult<()> {
    let name = normalize_relay_target_name(name).map_err(|err| CliError::Other(err.to_string()))?;
    if name == "local" {
        return Err(CliError::Other(
            "relay target name cannot be the built-in 'local' target".to_string(),
        ));
    }
    let loaded = load_config_with_source()
        .await
        .map_err(|err| CliError::Configuration(err.to_string()))?;
    let mut source = loaded.source;
    source.relay_targets.insert(name, target);
    save_helper_config(&source)
        .await
        .map_err(|err| CliError::Configuration(err.to_string()))?;
    Ok(())
}

async fn list_relay_targets() -> CliResult<()> {
    let cfg = load_config()
        .await
        .map_err(|err| CliError::Configuration(err.to_string()))?;
    println!("{}", "codex-helper relay targets".bold());
    for name in relay_target_names(&cfg) {
        let target = resolve_relay_target(&cfg, &name)
            .map_err(|err| CliError::Configuration(err.to_string()))?;
        let service = service_name_for_kind(target.service);
        let admin = target.admin_url.as_deref().unwrap_or("<unavailable>");
        let marker = if target.is_local() {
            "built-in"
        } else {
            "configured"
        };
        println!(
            "  {name:<16} {service:<6} proxy={} admin={} ({marker})",
            target.proxy_url, admin
        );
    }
    Ok(())
}

async fn relay_status(target: Option<String>, json: bool) -> CliResult<()> {
    let cfg = load_config()
        .await
        .map_err(|err| CliError::Configuration(err.to_string()))?;
    if let Some(target_name) = target {
        let target = resolve_relay_target(&cfg, &target_name)
            .map_err(|err| CliError::Configuration(err.to_string()))?;
        return print_relay_target_status(target, json).await;
    }

    if json {
        let status =
            codex_switch::inspect().map_err(|err| CliError::CodexConfig(err.to_string()))?;
        let targets = relay_target_names(&cfg);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "targets": targets,
                "codex": {
                    "enabled": status.enabled,
                    "base_url": status.base_url,
                    "managed": status.managed,
                    "phase": status.phase.as_str(),
                    "recovery_reason": status.recovery_reason,
                }
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
        return Ok(());
    }

    println!("{}", "codex-helper relay status".bold());
    println!("targets: {}", relay_target_names(&cfg).join(", "));
    print_codex_switch_status()?;
    Ok(())
}

async fn print_relay_target_status(target: ResolvedRelayTarget, json: bool) -> CliResult<()> {
    let Some(admin_url) = target.admin_url.clone() else {
        return Err(CliError::Other(format!(
            "relay target '{}' has no admin URL",
            target.name
        )));
    };
    let service_name = service_name_for_kind(target.service);
    let model =
        read_operator_model(&admin_url, target.admin_token_env.as_deref(), service_name).await?;
    if json {
        let payload = serde_json::json!({
            "target": target.name,
            "service": service_name,
            "proxy_url": target.proxy_url,
            "admin_url": admin_url,
            "reachable": operator_read_model_is_reachable(&model),
            "operator_read_model": model,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        );
        return Ok(());
    }

    println!("{}", format!("relay target '{}'", target.name).bold());
    println!("  service: {}", service_name);
    println!("  proxy:   {}", target.proxy_url);
    println!("  admin:   {}", admin_url);
    match model.status {
        OperatorReadStatus::Ready => {
            println!("  status:  {}", "[UP]".green());
        }
        OperatorReadStatus::Stale => {
            println!("  status:  {}", "[STALE]".yellow());
        }
        OperatorReadStatus::AuthRequired => {
            println!("  status:  {}", "[AUTH REQUIRED]".yellow());
        }
        OperatorReadStatus::Disconnected => {
            println!("  status:  {}", "[DOWN]".yellow());
        }
    }
    if let Some(data) = model.data.as_ref() {
        let routable_endpoints = data
            .summary
            .providers
            .iter()
            .map(|provider| provider.routable_endpoints)
            .sum::<usize>();
        let total_endpoints = data
            .summary
            .providers
            .iter()
            .map(|provider| provider.endpoints.len())
            .sum::<usize>();
        println!("  captured: {}", model.captured_at_ms);
        println!(
            "  active requests: {}, recent requests: {}, providers: {}, routable endpoints: {}/{}",
            data.summary.counts.active_requests,
            data.summary.counts.recent_requests,
            data.summary.counts.providers,
            routable_endpoints,
            total_endpoints
        );
    }
    Ok(())
}

async fn use_relay_target(target_name: String, no_tui: bool, attach_only: bool) -> CliResult<()> {
    let cfg = load_config()
        .await
        .map_err(|err| CliError::Configuration(err.to_string()))?;
    let target = resolve_relay_target(&cfg, &target_name)
        .map_err(|err| CliError::Configuration(err.to_string()))?;
    validate_relay_use_mode(&target, no_tui, attach_only)?;
    print_relay_client_config_hint(&target);
    if target.is_local() && !attach_only {
        let service_name = service_name_for_kind(target.service);
        let port = relay_proxy_port(&target)
            .unwrap_or_else(|| default_proxy_port_for_service(service_name));
        return run_server(
            service_name,
            IpAddr::from([127, 0, 0, 1]),
            port,
            ServeRuntimeOptions {
                enable_tui: !no_tui,
                ..ServeRuntimeOptions::default()
            },
        )
        .await
        .map_err(|err| CliError::Other(err.to_string()));
    }

    if !no_tui {
        attach_tui_to_relay_target(&target).await?;
    }
    Ok(())
}

fn validate_relay_use_mode(
    target: &ResolvedRelayTarget,
    no_tui: bool,
    attach_only: bool,
) -> CliResult<()> {
    if no_tui && attach_only {
        return Err(CliError::Other(
            "`--no-tui` and `--attach-only` together have no effect".to_string(),
        ));
    }
    if no_tui && !target.is_local() {
        return Err(CliError::Other(
            "`--no-tui` can only start the built-in local relay target; omit it to attach the read-only TUI to a remote target"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod relay_use_mode_tests {
    use super::*;

    fn relay_target(built_in_local: bool) -> ResolvedRelayTarget {
        ResolvedRelayTarget {
            name: if built_in_local { "local" } else { "nas" }.to_string(),
            service: ServiceKind::Codex,
            proxy_url: "http://127.0.0.1:3211".to_string(),
            admin_url: Some("http://127.0.0.1:4211".to_string()),
            admin_token_env: None,
            built_in_local,
        }
    }

    #[test]
    fn remote_relay_target_rejects_no_tui_instead_of_exiting_successfully() {
        let error = validate_relay_use_mode(&relay_target(false), true, false)
            .expect_err("remote --no-tui must not become a successful no-op");
        assert!(error.to_string().contains("built-in local relay target"));
    }

    #[test]
    fn local_relay_target_allows_no_tui_startup() {
        validate_relay_use_mode(&relay_target(true), true, false)
            .expect("local --no-tui should start without the console");
    }

    #[test]
    fn relay_target_rejects_no_tui_with_attach_only() {
        let error = validate_relay_use_mode(&relay_target(true), true, true)
            .expect_err("no-tui attach-only has no observable behavior");
        assert!(error.to_string().contains("have no effect"));
    }
}

fn print_relay_client_config_hint(target: &ResolvedRelayTarget) {
    println!(
        "Relay target '{}' is read-only for client configuration.",
        target.name
    );
    if target.service == ServiceKind::Codex {
        match ValidatedCodexBaseUrl::parse(target.proxy_url.as_str()) {
            Ok(base_url) => {
                println!(
                    "Run `codex-helper switch on --base-url <URL>` explicitly to update Codex."
                );
                println!("  URL: {}", base_url.as_str());
            }
            Err(error) => {
                println!("This relay URL is incompatible with the explicit Codex switch: {error}")
            }
        }
    }
}

async fn attach_tui_to_relay_target(target: &ResolvedRelayTarget) -> CliResult<()> {
    let Some(admin_url) = target.admin_url.clone() else {
        return Err(CliError::Other(format!(
            "relay target '{}' has no admin URL",
            target.name
        )));
    };
    let service_name = service_name_for_kind(target.service);
    let proxy_port =
        relay_proxy_port(target).unwrap_or_else(|| default_proxy_port_for_service(service_name));
    let result = if target.is_local() {
        tui::run_local_attached_dashboard_with_admin_base_url(
            service_name,
            proxy_port,
            admin_url,
            target.admin_token_env.clone(),
        )
        .await
    } else {
        tui::run_attached_dashboard_with_admin_base_url(
            service_name,
            proxy_port,
            admin_url,
            target.admin_token_env.clone(),
        )
        .await
    };
    result.map_err(|err| CliError::Other(err.to_string()))
}

fn service_name_for_kind(service: ServiceKind) -> &'static str {
    match service {
        ServiceKind::Codex => "codex",
        ServiceKind::Claude => "claude",
    }
}

fn relay_proxy_port(target: &ResolvedRelayTarget) -> Option<u16> {
    reqwest::Url::parse(&target.proxy_url)
        .ok()
        .and_then(|url| url.port_or_known_default())
}

fn daemon_admin_base_url_for_proxy_port(port: u16) -> String {
    format!("http://{}", admin_loopback_addr_for_proxy_port(port))
}

#[derive(Debug, thiserror::Error)]
enum ServiceRefreshTargetError {
    #[error(transparent)]
    Receipt(#[from] ServiceReceiptError),
    #[error("service receipt targets a different service")]
    ServiceMismatch,
    #[error("service receipt targets a different platform backend")]
    PlatformMismatch,
    #[error("service receipt admin authority is invalid: {0}")]
    AdminAuthority(#[from] anyhow::Error),
}

struct VerifiedServiceRefreshTarget {
    receipt: ServiceReceipt,
    endpoint: ControlPlaneEndpoint,
}

impl VerifiedServiceRefreshTarget {
    fn local_operator_client(
        &self,
    ) -> Result<crate::control_plane_client::LocalOperatorClient, ServiceRefreshTargetError> {
        crate::control_plane_client::LocalOperatorClient::from_helper_home(
            self.endpoint.clone(),
            self.receipt.helper_home(),
        )
        .map_err(ServiceRefreshTargetError::AdminAuthority)
    }
}

#[cfg(test)]
fn resolve_service_refresh_target(
    helper_home: &Path,
    expected_service: ServiceKind,
) -> Result<VerifiedServiceRefreshTarget, ServiceRefreshTargetError> {
    resolve_service_refresh_target_inner(helper_home, Some(expected_service))
}

fn resolve_service_refresh_target_inner(
    helper_home: &Path,
    expected_service: Option<ServiceKind>,
) -> Result<VerifiedServiceRefreshTarget, ServiceRefreshTargetError> {
    let receipt = read_service_receipt(helper_home)?;
    verify_service_refresh_target(receipt, expected_service)
}

fn verify_service_refresh_target(
    receipt: ServiceReceipt,
    expected_service: Option<ServiceKind>,
) -> Result<VerifiedServiceRefreshTarget, ServiceRefreshTargetError> {
    if expected_service.is_some_and(|expected| receipt.service() != expected) {
        return Err(ServiceRefreshTargetError::ServiceMismatch);
    }
    if ServicePlatformBackend::current() != Some(receipt.platform_backend()) {
        return Err(ServiceRefreshTargetError::PlatformMismatch);
    }
    let endpoint = ControlPlaneEndpoint::new(
        receipt.admin_base_url(),
        configured_local_admin_token_env().map(str::to_string),
    )
    .map_err(ServiceRefreshTargetError::AdminAuthority)?;
    Ok(VerifiedServiceRefreshTarget { receipt, endpoint })
}

pub(crate) async fn refresh_resident_credential(
    credential_name: codex_helper_core::credentials::CredentialName,
    action: codex_helper_core::service_target::LocalCredentialRefreshAction,
) -> anyhow::Result<codex_helper_core::service_target::LocalCredentialRefreshResponse> {
    let helper_home = crate::config::proxy_home_dir();
    let target = resolve_service_refresh_target_inner(&helper_home, None)?;
    let service = target.receipt.service();
    let request = codex_helper_core::service_target::LocalCredentialRefreshRequest {
        service,
        install_generation: target.receipt.install_generation().clone(),
        credential_name,
        action,
    };
    Ok(target
        .local_operator_client()?
        .refresh_native_credential(&request)
        .await?)
}

pub(crate) async fn read_resident_operator_model() -> anyhow::Result<OperatorReadModel> {
    Ok(read_resident_service_runtime().await?.operator)
}

pub(crate) async fn read_resident_service_runtime()
-> anyhow::Result<codex_helper_core::service_target::LocalServiceRuntimeReadResponse> {
    let helper_home = crate::config::proxy_home_dir();
    let target = resolve_service_refresh_target_inner(&helper_home, None)?;
    read_service_runtime_from_target(target).await
}

pub(crate) async fn read_service_runtime_for_receipt(
    receipt: ServiceReceipt,
) -> anyhow::Result<codex_helper_core::service_target::LocalServiceRuntimeReadResponse> {
    let target = verify_service_refresh_target(receipt, None)?;
    read_service_runtime_from_target(target).await
}

async fn read_service_runtime_from_target(
    target: VerifiedServiceRefreshTarget,
) -> anyhow::Result<codex_helper_core::service_target::LocalServiceRuntimeReadResponse> {
    let request = codex_helper_core::service_target::LocalServiceRuntimeReadRequest {
        service: target.receipt.service(),
        install_generation: target.receipt.install_generation().clone(),
    };
    let response = target
        .local_operator_client()?
        .read_service_runtime(&request)
        .await?;
    anyhow::ensure!(
        response.identity.helper_home == target.receipt.helper_home(),
        "service receipt and resident runtime identify different helper homes"
    );
    anyhow::ensure!(
        response.identity.client_home == target.receipt.client_home(),
        "service receipt and resident runtime identify different client homes"
    );
    Ok(response)
}

async fn read_operator_model(
    admin_url: &str,
    admin_token_env: Option<&str>,
    service_name: &str,
) -> CliResult<OperatorReadModel> {
    let client = control_plane_client(admin_url, admin_token_env)?;
    Ok(client.refresh_operator_read_model(service_name, None).await)
}

fn control_plane_client(
    admin_url: &str,
    admin_token_env: Option<&str>,
) -> CliResult<ControlPlaneClient> {
    let endpoint =
        ControlPlaneEndpoint::new(admin_url.to_string(), admin_token_env.map(str::to_string))
            .map_err(|err| CliError::Other(err.to_string()))?;
    ControlPlaneClient::new(endpoint).map_err(|err| CliError::Other(err.to_string()))
}

fn local_control_plane_client(port: u16) -> CliResult<ControlPlaneClient> {
    control_plane_client(
        &daemon_admin_base_url_for_proxy_port(port),
        configured_local_admin_token_env(),
    )
}

async fn read_local_operator_model(service_name: &str, port: u16) -> CliResult<OperatorReadModel> {
    let client = local_control_plane_client(port)?;
    Ok(client.refresh_operator_read_model(service_name, None).await)
}

fn operator_read_model_is_reachable(model: &OperatorReadModel) -> bool {
    model.status != OperatorReadStatus::Disconnected
}

async fn print_daemon_status(service_name: &'static str, port: u16, json: bool) -> CliResult<()> {
    let owner_marker = read_owner_marker_best_effort(service_name, port);
    let model = read_local_operator_model(service_name, port).await?;

    if json {
        let payload = serde_json::json!({
            "service_name": service_name,
            "port": port,
            "running": operator_read_model_is_reachable(&model),
            "owner": owner_marker,
            "operator_read_model": model,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
        );
        return Ok(());
    }

    let admin_addr = admin_loopback_addr_for_proxy_port(port);

    println!("{}", "codex-helper daemon status".bold());
    match model.status {
        OperatorReadStatus::Ready => println!(
            "{} {}:{} (admin: http://{})",
            "[UP]".green(),
            model.service_name,
            port,
            admin_addr
        ),
        OperatorReadStatus::Stale => println!(
            "{} {}:{} (admin: http://{})",
            "[STALE]".yellow(),
            model.service_name,
            port,
            admin_addr
        ),
        OperatorReadStatus::AuthRequired => println!(
            "{} {}:{} (admin: http://{})",
            "[AUTH REQUIRED]".yellow(),
            service_name,
            port,
            admin_addr
        ),
        OperatorReadStatus::Disconnected => println!(
            "{} {}:{} (admin: http://{})",
            "[DOWN]".yellow(),
            service_name,
            port,
            admin_addr
        ),
    }
    if let Some(owner) = owner_marker.as_ref() {
        println!(
            "  owner: {} (mode: {}, pid: {})",
            owner.owner, owner.lifecycle_mode, owner.pid
        );
    } else {
        println!("  owner: <unknown/manual older runtime>");
    }
    if let Some(data) = model.data.as_ref() {
        let routable_endpoints = data
            .summary
            .providers
            .iter()
            .map(|provider| provider.routable_endpoints)
            .sum::<usize>();
        let total_endpoints = data
            .summary
            .providers
            .iter()
            .map(|provider| provider.endpoints.len())
            .sum::<usize>();
        println!(
            "  default profile: {}",
            data.summary
                .runtime
                .default_profile
                .as_deref()
                .unwrap_or("<none>")
        );
        println!(
            "  active requests: {}, recent requests: {}, providers: {}, routable endpoints: {}/{}",
            data.summary.counts.active_requests,
            data.summary.counts.recent_requests,
            data.summary.counts.providers,
            routable_endpoints,
            total_endpoints
        );
    } else if model.status == OperatorReadStatus::Disconnected {
        println!(
            "  高级模式可用 `codex-helper serve --{} --resident` 显式启动常驻代理。",
            service_name
        );
    }
    Ok(())
}

#[cfg(test)]
mod local_admin_token_contract_tests {
    #[test]
    fn compose_healthcheck_expands_the_configured_admin_token_inside_the_container() {
        let compose = include_str!("../deploy/compose/codex-helper.yml");
        let header = format!(
            r#"-H \"{}: $${}\""#,
            crate::proxy::ADMIN_TOKEN_HEADER,
            crate::proxy::ADMIN_TOKEN_ENV_VAR
        );

        assert!(
            compose.contains(&header),
            "missing healthcheck header: {header}"
        );
        assert!(
            compose.contains("/__codex_helper/api/v1/operator/read-model"),
            "healthcheck must use the canonical operator read-model"
        );
    }
}

async fn supervise_daemon(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    max_restarts: u32,
) -> CliResult<()> {
    let exe = std::env::current_exe()
        .map_err(|err| CliError::Other(format!("failed to locate current executable: {err}")))?;
    let crash_marker_path = supervisor_crash_marker_path(service_name, port);
    let owner_marker = RuntimeOwnerMarker::new(RuntimeOwnerKind::Supervisor, service_name, port)
        .with_note("supervisor is managing a resident proxy child");
    if let Err(err) = write_owner_marker(&owner_marker) {
        tracing::warn!("failed to write supervisor owner marker: {err}");
    }
    let _owner_marker_guard = RuntimeOwnerMarkerGuard::new(service_name, port, true);
    let mut restart_count = 0u32;
    println!(
        "{} supervising {} resident proxy on http://{}:{}",
        "[OK]".green(),
        service_name,
        host,
        port
    );

    loop {
        let mut args = vec![
            "serve".to_string(),
            format!("--{service_name}"),
            "--host".to_string(),
            host.to_string(),
            "--port".to_string(),
            port.to_string(),
            "--resident".to_string(),
            "--supervisor-managed".to_string(),
        ];
        if service_name != "codex" && service_name != "claude" {
            args.retain(|arg| arg != &format!("--{service_name}"));
        }

        let mut child = tokio::process::Command::new(&exe)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .map_err(|err| {
                CliError::Other(format!(
                    "failed to spawn resident proxy child {:?} {:?}: {err}",
                    exe, args
                ))
            })?;

        tokio::select! {
            status = child.wait() => {
                let status = status.map_err(|err| CliError::Other(format!("resident proxy child wait failed: {err}")))?;
                if status.success() {
                    clear_supervisor_crash_marker(&crash_marker_path);
                    if let Err(err) = clear_owner_marker(service_name, port) {
                        tracing::warn!("failed to clear supervisor owner marker: {err}");
                    }
                    println!(
                        "{} {} resident proxy exited cleanly; supervisor is stopping",
                        "[OK]".green(),
                        service_name
                    );
                    return Ok(());
                }

                restart_count = restart_count.saturating_add(1);
                record_supervisor_crash_marker(
                    &crash_marker_path,
                    service_name,
                    host,
                    port,
                    restart_count,
                    max_restarts,
                    &status.to_string(),
                );
                if restart_count > max_restarts {
                    return Err(CliError::Other(format!(
                        "{} resident proxy crashed too many times ({restart_count}/{max_restarts}); last status: {status}",
                        service_name
                    )));
                }

                let delay_secs = supervisor_restart_delay_secs(restart_count);
                println!(
                    "{} {} resident proxy exited with {status}; restart {restart_count}/{max_restarts} in {delay_secs}s",
                    "[WARN]".yellow(),
                    service_name
                );
                tokio::time::sleep(Duration::from_secs(delay_secs)).await;
            }
            _ = wait_for_shutdown_signal() => {
                println!(
                    "{} stopping supervisor and resident proxy child",
                    "[INFO]".cyan()
                );
                let _ = child.start_kill();
                let _ = child.wait().await;
                if let Err(err) = clear_owner_marker(service_name, port) {
                    tracing::warn!("failed to clear supervisor owner marker: {err}");
                }
                return Ok(());
            }
        }
    }
}

fn supervisor_crash_marker_path(service_name: &str, port: u16) -> PathBuf {
    crate::config::proxy_home_dir()
        .join("run")
        .join(format!("{service_name}-{port}.supervisor-crash.json"))
}

fn clear_supervisor_crash_marker(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => tracing::warn!(
            "failed to clear supervisor crash marker {:?}: {}",
            path,
            err
        ),
    }
}

fn record_supervisor_crash_marker(
    path: &Path,
    service_name: &str,
    host: IpAddr,
    port: u16,
    restart_count: u32,
    max_restarts: u32,
    status: &str,
) {
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(
            "failed to create supervisor marker dir {:?}: {}",
            parent,
            err
        );
        return;
    }

    let payload = serde_json::json!({
        "service_name": service_name,
        "host": host.to_string(),
        "port": port,
        "restart_count": restart_count,
        "max_restarts": max_restarts,
        "status": status,
        "recorded_at_ms": current_epoch_ms(),
        "hint": "resident proxy child exited unexpectedly; supervisor will restart until max_restarts is exceeded"
    });

    match serde_json::to_string_pretty(&payload) {
        Ok(text) => {
            if let Err(err) = std::fs::write(path, text) {
                tracing::warn!(
                    "failed to write supervisor crash marker {:?}: {}",
                    path,
                    err
                );
            }
        }
        Err(err) => tracing::warn!("failed to serialize supervisor crash marker: {}", err),
    }
}

fn current_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn supervisor_restart_delay_secs(restart_count: u32) -> u64 {
    1u64 << restart_count.saturating_sub(1).min(5)
}

async fn load_serve_config() -> anyhow::Result<LoadedConfig> {
    load_config_with_source().await
}

fn resolve_serve_tui_language(loaded: &LoadedConfig) -> tui::Language {
    if let Ok(language) = std::env::var("CODEX_HELPER_TUI_LANG") {
        return tui::resolve_language_preference(Some(&language));
    }
    if let Some(language) = loaded.source.ui.language.as_deref() {
        if !language.trim().eq_ignore_ascii_case("auto") && tui::parse_language(language).is_none()
        {
            tracing::warn!(
                "Invalid ui.language '{}', falling back to system locale",
                language
            );
        }
        return tui::resolve_language_preference(Some(language));
    }
    tui::detect_system_language()
}

fn codex_startup_readiness_for_existing_switch(
    port: u16,
) -> codex_integration::CodexStartupReadiness {
    use codex_integration::{
        CodexStartupReadinessIssue, CodexStartupReadinessIssueKind, CodexStartupReadinessSeverity,
    };

    let issue = match codex_switch::inspect() {
        Err(error) => Some(CodexStartupReadinessIssue {
            kind: CodexStartupReadinessIssueKind::DiagnosticError,
            severity: CodexStartupReadinessSeverity::Warning,
            title: "Codex switch status is unavailable".to_string(),
            detail: error.to_string(),
            action: "Inspect the helper-owned switch journal before changing Codex config."
                .to_string(),
        }),
        Ok(status) => match status.phase {
            CodexSwitchPhase::Off => Some(CodexStartupReadinessIssue {
                kind: CodexStartupReadinessIssueKind::SwitchDisabled,
                severity: CodexStartupReadinessSeverity::Warning,
                title: "Codex is not switched to this proxy".to_string(),
                detail: "No applied helper-owned Codex switch was found.".to_string(),
                action: format!(
                    "Run `codex-helper switch on --port {port}` explicitly before starting a Codex client."
                ),
            }),
            CodexSwitchPhase::RecoveryRequired => Some(CodexStartupReadinessIssue {
                kind: CodexStartupReadinessIssueKind::DiagnosticError,
                severity: CodexStartupReadinessSeverity::Warning,
                title: "Codex switch requires recovery".to_string(),
                detail: status.recovery_reason.unwrap_or_else(|| {
                    "The config no longer matches the switch journal.".to_string()
                }),
                action: "Do not overwrite Codex config; reconcile the recorded fingerprints first."
                    .to_string(),
            }),
            CodexSwitchPhase::Prepared => Some(CodexStartupReadinessIssue {
                kind: CodexStartupReadinessIssueKind::DiagnosticError,
                severity: CodexStartupReadinessSeverity::Warning,
                title: "Codex switch operation is incomplete".to_string(),
                detail: "A prepared helper-owned switch operation still needs recovery."
                    .to_string(),
                action: format!(
                    "Retry `codex-helper switch on --port {port}` or run `codex-helper switch off`."
                ),
            }),
            CodexSwitchPhase::Applied if !status.enabled => Some(CodexStartupReadinessIssue {
                kind: CodexStartupReadinessIssueKind::DiagnosticError,
                severity: CodexStartupReadinessSeverity::Warning,
                title: "Codex switch state is inconsistent".to_string(),
                detail: "The journal is applied but Codex does not select the helper stanza."
                    .to_string(),
                action:
                    "Run `codex-helper switch status` and reconcile the config before continuing."
                        .to_string(),
            }),
            CodexSwitchPhase::Applied => {
                let expected = ValidatedCodexBaseUrl::local(port);
                (status.base_url.as_deref() != Some(expected.as_str())).then(|| {
                    CodexStartupReadinessIssue {
                        kind: CodexStartupReadinessIssueKind::SwitchPortMismatch,
                        severity: CodexStartupReadinessSeverity::Warning,
                        title: "Codex points to a different helper endpoint".to_string(),
                        detail: format!(
                            "Codex uses {}, but this proxy is starting at {}.",
                            status.base_url.as_deref().unwrap_or("<unset>"),
                            expected.as_str()
                        ),
                        action: format!(
                            "Run `codex-helper switch off`, then `codex-helper switch on --port {port}`, or serve at the configured endpoint."
                        ),
                    }
                })
            }
        },
    };

    codex_integration::CodexStartupReadiness {
        issues: issue.into_iter().collect(),
    }
}

async fn run_server(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    options: ServeRuntimeOptions,
) -> anyhow::Result<()> {
    let owner_kind = options.owner_kind();
    let interactive = !options.is_resident()
        && options.enable_tui
        && atty::is(atty::Stream::Stdin)
        && atty::is(atty::Stream::Stdout);
    let _owner_marker_guard =
        RuntimeOwnerMarkerGuard::new(service_name, port, owner_kind.is_some());

    if let Some(owner_kind) = owner_kind {
        let marker = RuntimeOwnerMarker::new(owner_kind, service_name, port);
        if let Err(err) = write_owner_marker(&marker) {
            tracing::warn!("failed to write runtime owner marker: {err}");
        }
    }
    let loaded = load_serve_config().await?;
    let tui_lang = resolve_serve_tui_language(&loaded);

    let runtime =
        build_local_proxy_runtime(service_name, host, port, options.service_managed, loaded)
            .await?;
    let addr: SocketAddr = SocketAddr::from((host, port));
    let admin_addr = runtime.admin_addr;
    let cfg = runtime.config.clone();
    let proxy = runtime.proxy.clone();
    let state = runtime.state.clone();
    let shutdown_tx = runtime.shutdown_tx.clone();
    let shutdown_rx = runtime.shutdown_receiver();

    tracing::info!(
        "codex-helper proxy listening on http://{} (service: {})",
        addr,
        service_name
    );
    tracing::info!(
        "codex-helper admin API listening on http://{} (service: {})",
        admin_addr,
        service_name
    );

    {
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            wait_for_shutdown_signal().await;
            let _ = shutdown_tx.send(true);
        });
    }

    let mut running_runtime = runtime.start();

    let result = if interactive {
        let startup_readiness =
            (service_name == "codex").then(|| codex_startup_readiness_for_existing_switch(port));

        let mut tui_handle = tokio::spawn(tui::run_dashboard(
            proxy.clone(),
            state,
            cfg.clone(),
            service_name,
            port,
            admin_addr.port(),
            startup_readiness,
            tui_lang,
            shutdown_tx.clone(),
            shutdown_rx.clone(),
        ));

        tokio::select! {
            server_res = running_runtime.wait() => {
                let _ = shutdown_tx.send(true);
                let _ = tui_handle.await;
                server_res?;
                Ok::<(), anyhow::Error>(())
            }
            tui_res = &mut tui_handle => {
                match tui_res {
                    Ok(Ok(())) => {
                        // The dashboard requested a shutdown (or exited because shutdown was already triggered).
                        let _ = shutdown_tx.send(true);
                        await_server_shutdown_with_timeout(&mut running_runtime).await?;
                        Ok::<(), anyhow::Error>(())
                    }
                    Ok(Err(err)) => {
                        // If the dashboard fails (e.g. terminal issues), keep running without it.
                        tracing::warn!("TUI dashboard failed; continuing without TUI: {}", err);
                        await_server_shutdown(&mut running_runtime).await?;
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(join_err) => {
                        tracing::warn!("TUI task join error; continuing without TUI: {}", join_err);
                        await_server_shutdown(&mut running_runtime).await?;
                        Ok::<(), anyhow::Error>(())
                    }
                }
            }
        }
    } else {
        await_server_shutdown(&mut running_runtime).await?;
        Ok(())
    };

    result?;

    Ok(())
}

pub(crate) async fn run_service_managed_server(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
) -> CliResult<()> {
    run_server(
        service_name,
        host,
        port,
        ServeRuntimeOptions {
            enable_tui: false,
            service_managed: true,
            ..ServeRuntimeOptions::default()
        },
    )
    .await
    .map_err(|error| CliError::Other(error.to_string()))
}

async fn build_local_proxy_runtime(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    service_managed: bool,
    loaded: LoadedConfig,
) -> anyhow::Result<ProxyRuntime> {
    let admin_addr = admin_loopback_addr_for_proxy_port(port);
    let service_install_generation = if service_managed {
        codex_helper_core::service_target::ServiceInstallGeneration::from_process_env()
            .context("read service install generation from process environment")?
    } else {
        None
    };
    if !host.is_loopback() {
        tracing::warn!(
            "Binding to non-loopback address {}. This may expose your proxy to other machines. Consider using 127.0.0.1 + SSH port forwarding instead.",
            host
        );
        tracing::warn!(
            "The /__codex_helper admin API stays on loopback only at http://{}.",
            admin_addr
        );
    }
    let service_runtime_identity = service_install_generation.as_ref().map(|generation| {
        codex_helper_core::service_target::ServiceRuntimeIdentity {
            service: codex_helper_core::runtime_host::service_kind_for_name(service_name),
            helper_home: crate::config::proxy_home_dir(),
            client_home: if service_name == "claude" {
                crate::config::claude_home()
            } else {
                crate::config::codex_home()
            },
            install_generation: generation.clone(),
        }
    });
    build_proxy_runtime_from_loaded_with_options(
        service_name,
        host,
        port,
        ProxyRuntimeOptions::for_proxy_port(port)
            .with_admin_addr(admin_addr)
            .with_credential_sources(CredentialSourceCapabilities::platform_native())
            .with_service_runtime_identity(service_runtime_identity),
        loaded,
    )
    .await
    .map_err(|error| local_runtime_startup_error(error, service_name))
}

async fn await_server_shutdown(runtime: &mut RunningProxyRuntime) -> anyhow::Result<()> {
    runtime.wait().await
}

async fn await_server_shutdown_with_timeout(
    runtime: &mut RunningProxyRuntime,
) -> anyhow::Result<()> {
    let timeout = std::time::Duration::from_secs(2);
    tokio::select! {
        joined = runtime.wait() => joined,
        _ = tokio::time::sleep(timeout) => {
            tracing::warn!(
                "server graceful shutdown exceeded {}ms; aborting remaining server tasks",
                timeout.as_millis()
            );
            runtime.abort_and_wait().await
        }
    }
}

fn local_runtime_startup_error(error: anyhow::Error, service_name: &'static str) -> anyhow::Error {
    let bind_help = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ProxyListenerBindError>())
        .map(|bind_error| {
            listener_bind_help(bind_error.addr(), service_name, bind_error.source_error())
        });
    match bind_help {
        Some(help) => error.context(help),
        None => error,
    }
}

fn listener_bind_help(addr: SocketAddr, service_name: &str, err: &std::io::Error) -> String {
    let port = addr.port();
    let example_port = port.saturating_add(1);

    let service_flag = match service_name {
        "codex" => Some("--codex"),
        "claude" => Some("--claude"),
        _ => None,
    };

    let mut example_cmd = vec!["codex-helper", "serve"];
    if let Some(flag) = service_flag {
        example_cmd.push(flag);
    }
    example_cmd.push("--port");
    let example_port_s = example_port.to_string();
    example_cmd.push(&example_port_s);
    let example_cmd = example_cmd.join(" ");

    let os_code = err.raw_os_error();
    let kind = err.kind();
    let is_addr_in_use = kind == ErrorKind::AddrInUse || os_code == Some(10048);
    let is_windows_10013 = os_code == Some(10013);
    let is_permission_denied = kind == ErrorKind::PermissionDenied || is_windows_10013;

    if is_addr_in_use {
        let mut lines = vec![format!(
            "无法监听 http://{addr}（service: {service_name}）：端口 {port} 可能已被占用。"
        )];
        if let Some(hint) = listener_bind_port_owner_hint(port) {
            lines.push(hint);
        }
        lines.push(format!(
            "- 关闭占用该端口的进程，或改用其它端口，例如：`{example_cmd}`"
        ));
        return lines.join("\n");
    }

    if is_permission_denied {
        let mut lines = vec![format!(
            "无法监听 http://{addr}（service: {service_name}）：没有权限绑定到端口 {port}。"
        )];
        if is_windows_10013 {
            lines.push(
                "提示：Windows 的 (os error 10013) 既可能是权限/安全软件拦截，也可能是该端口被其它进程以“排他方式”占用（有时不会报 10048）。"
                    .to_string(),
            );
        }
        if let Some(hint) = listener_bind_port_owner_hint(port) {
            lines.push(hint);
        }
        lines.push(format!(
            "- 可先确认是否有残留 `codex-helper`/其它进程占用该端口，然后重试或换端口，例如：`{example_cmd}`"
        ));
        lines.push("- 如仍失败，可尝试以管理员身份运行，或检查防火墙/安全软件策略。".to_string());
        return lines.join("\n");
    }

    format!(
        "无法监听 http://{addr}（service: {service_name}）。\n\
- 可尝试换端口，例如：`{example_cmd}`"
    )
}

#[cfg(test)]
fn listener_bind_port_owner_hint(_port: u16) -> Option<String> {
    None
}

#[cfg(not(test))]
fn listener_bind_port_owner_hint(port: u16) -> Option<String> {
    port_owner_hint(port)
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
struct PortOwner {
    pid: u32,
    name: Option<String>,
}

#[cfg_attr(test, allow(dead_code))]
fn port_owner_hint(port: u16) -> Option<String> {
    let owners = port_owners(port);
    if owners.is_empty() {
        return None;
    }
    let desc = owners
        .into_iter()
        .take(5)
        .map(|o| match o.name {
            Some(name) if !name.trim().is_empty() => format!("PID {} ({})", o.pid, name),
            _ => format!("PID {}", o.pid),
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("- 占用该端口的进程（尽力推断）：{desc}"))
}

#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
fn port_owners(port: u16) -> Vec<PortOwner> {
    let out = run_cmd_stdout("netstat", &["-ano", "-p", "tcp"]).unwrap_or_default();
    let pids = parse_windows_netstat_listening_pids(&out, port);
    pids.into_iter()
        .map(|pid| PortOwner {
            pid,
            name: windows_tasklist_image_name(pid),
        })
        .collect()
}

#[cfg(unix)]
#[cfg_attr(test, allow(dead_code))]
fn port_owners(port: u16) -> Vec<PortOwner> {
    #[cfg(target_os = "linux")]
    {
        if let Some(out) = run_cmd_stdout("ss", &["-ltnp"]) {
            let owners = parse_linux_ss_listening_owners(&out, port);
            if !owners.is_empty() {
                return owners;
            }
        }
    }

    if let Some(out) = run_cmd_stdout_owned(
        "lsof",
        &[
            "-nP".to_string(),
            format!("-iTCP:{port}"),
            "-sTCP:LISTEN".to_string(),
        ],
    ) {
        let owners = parse_unix_lsof_owners(&out);
        if !owners.is_empty() {
            return owners;
        }
    }

    Vec::new()
}

#[cfg(not(any(windows, unix)))]
fn port_owners(_port: u16) -> Vec<PortOwner> {
    Vec::new()
}

#[cfg(any(windows, target_os = "linux"))]
#[cfg_attr(test, allow(dead_code))]
fn run_cmd_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = ProcessCommand::new(program).args(args).output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return None;
    }
    Some(stdout)
}

#[cfg_attr(test, allow(dead_code))]
fn run_cmd_stdout_owned(program: &str, args: &[String]) -> Option<String> {
    let output = ProcessCommand::new(program).args(args).output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return None;
    }
    Some(stdout)
}

#[cfg(windows)]
fn parse_windows_netstat_listening_pids(output: &str, port: u16) -> Vec<u32> {
    let port_suffix = format!(":{port}");
    let mut pids = Vec::<u32>::new();
    for line in output.lines() {
        let line = line.trim();
        if !line.starts_with("TCP") {
            continue;
        }
        let cols = line.split_whitespace().collect::<Vec<_>>();
        if cols.len() < 5 {
            continue;
        }
        let local = cols[1];
        let state = cols[3];
        let pid_s = cols[4];
        if !state.eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        if !local.ends_with(&port_suffix) {
            continue;
        }
        let Ok(pid) = pid_s.parse::<u32>() else {
            continue;
        };
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
}

#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
fn windows_tasklist_image_name(pid: u32) -> Option<String> {
    let filter = format!("PID eq {pid}");
    let out = run_cmd_stdout_owned(
        "tasklist",
        &[
            "/FI".to_string(),
            filter,
            "/FO".to_string(),
            "CSV".to_string(),
            "/NH".to_string(),
        ],
    )?;

    let line = out.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    // When there is no such PID, `tasklist` prints an informational message (localized) without CSV quotes.
    if !line.starts_with('"') {
        return None;
    }

    // Example (CSV): "Image Name","PID","Session Name","Session#","Mem Usage"
    let mut parts = line.split("\",\"");
    let image = parts.next()?.trim().trim_matches('"').to_string();
    if image.is_empty() { None } else { Some(image) }
}

#[cfg(target_os = "linux")]
fn parse_linux_ss_listening_owners(output: &str, port: u16) -> Vec<PortOwner> {
    let port_marker = format!(":{port}");
    let mut owners = Vec::<PortOwner>::new();
    for line in output.lines() {
        if !line.contains("LISTEN") {
            continue;
        }
        if !line.contains(&port_marker) {
            continue;
        }
        let mut rest = line;
        while let Some(pid_pos) = rest.find("pid=") {
            let after = &rest[pid_pos + 4..];
            let pid_len = after.chars().take_while(|c| c.is_ascii_digit()).count();
            if pid_len == 0 {
                rest = after;
                continue;
            }
            let Ok(pid) = after[..pid_len].parse::<u32>() else {
                rest = &after[pid_len..];
                continue;
            };

            let name = rest[..pid_pos].rfind("((").and_then(|start| {
                let sub = &rest[start..pid_pos];
                let q1 = sub.find('"')?;
                let after_q1 = &sub[q1 + 1..];
                let q2 = after_q1.find('"')?;
                Some(after_q1[..q2].to_string())
            });

            if !owners.iter().any(|o| o.pid == pid) {
                owners.push(PortOwner { pid, name });
            }
            rest = &after[pid_len..];
        }
    }
    owners
}

#[cfg(unix)]
fn parse_unix_lsof_owners(output: &str) -> Vec<PortOwner> {
    let mut owners = Vec::<PortOwner>::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("COMMAND") {
            continue;
        }
        let cols = line.split_whitespace().collect::<Vec<_>>();
        if cols.len() < 2 {
            continue;
        }
        let name = cols[0].to_string();
        let Ok(pid) = cols[1].parse::<u32>() else {
            continue;
        };
        if !owners.iter().any(|o| o.pid == pid) {
            owners.push(PortOwner {
                pid,
                name: Some(name),
            });
        }
    }
    owners
}

fn do_switch_on(port: Option<u16>, base_url: Option<String>) -> CliResult<()> {
    let validated_base_url = match base_url {
        Some(base_url) => ValidatedCodexBaseUrl::parse(base_url),
        None => Ok(ValidatedCodexBaseUrl::local(port.unwrap_or_else(|| {
            default_proxy_port_for_service_kind(ServiceKind::Codex)
        }))),
    }
    .map_err(|error| CliError::CodexConfig(error.to_string()))?;
    let outcome = codex_switch::apply(CodexSwitchIntent::On { validated_base_url })
        .map_err(|error| CliError::CodexConfig(error.to_string()))?;
    println!(
        "Codex switch: {} ({})",
        outcome.change.as_str(),
        outcome.status.phase.as_str()
    );
    if let Some(base_url) = outcome.status.base_url.as_deref() {
        println!("  base_url: {base_url}");
    }
    println!("  config: {:?}", outcome.status.config_path);
    println!("  state:  {:?}", outcome.status.state_path);
    Ok(())
}

fn do_switch_off() -> CliResult<()> {
    let outcome = codex_switch::apply(CodexSwitchIntent::Off)
        .map_err(|error| CliError::CodexConfig(error.to_string()))?;
    println!(
        "Codex switch: {} ({})",
        outcome.change.as_str(),
        outcome.status.phase.as_str()
    );
    Ok(())
}

fn do_switch_status() -> CliResult<()> {
    print_codex_switch_status()
}

async fn handle_default_cmd(codex: bool, claude: bool) -> CliResult<()> {
    if codex && claude {
        return Err(CliError::Other(
            "Please specify at most one of --codex / --claude".to_string(),
        ));
    }

    let loaded = load_config_with_source()
        .await
        .map_err(|e| CliError::Configuration(e.to_string()))?;

    if codex || claude {
        let mut source = loaded.source;
        source.default_service = Some(if claude {
            ServiceKind::Claude
        } else {
            ServiceKind::Codex
        });
        save_helper_config(&source)
            .await
            .map_err(|e| CliError::Configuration(e.to_string()))?;

        let name = if claude { "Claude" } else { "Codex" };
        println!("Default target service has been set to {}.", name);
    } else {
        let name = match loaded.source.default_service {
            Some(ServiceKind::Claude) => "Claude",
            _ => "Codex",
        };
        println!("Current default target service: {}.", name);
    }

    Ok(())
}

fn print_codex_switch_status() -> CliResult<()> {
    let status =
        codex_switch::inspect().map_err(|error| CliError::CodexConfig(error.to_string()))?;
    println!("{}", "Codex switch status".bold());
    println!("  phase:   {}", status.phase.as_str());
    println!("  enabled: {}", status.enabled);
    println!("  managed: {}", status.managed);
    println!("  config:  {:?}", status.config_path);
    println!("  state:   {:?}", status.state_path);
    println!(
        "  base_url: {}",
        status.base_url.as_deref().unwrap_or("<unset>")
    );
    if let Some(reason) = status.recovery_reason.as_deref() {
        println!("  recovery: {}", reason.yellow());
    }
    Ok(())
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
                // Fallback: at least handle Ctrl+C.
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
mod listener_bind_help_tests {
    use super::*;

    #[test]
    fn bind_help_mentions_addr_and_service() {
        let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], 3211));
        let err = std::io::Error::new(ErrorKind::AddrInUse, "in use");
        let msg = listener_bind_help(addr, "codex", &err);
        assert!(msg.contains("http://127.0.0.1:3211"));
        assert!(msg.contains("service: codex"));
    }

    #[test]
    fn bind_help_for_addr_in_use_is_friendly() {
        let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], 3211));
        let err = std::io::Error::new(ErrorKind::AddrInUse, "in use");
        let msg = listener_bind_help(addr, "codex", &err);
        assert!(msg.contains("端口 3211"));
        assert!(msg.contains("可能已被占用"));
        assert!(msg.contains("codex-helper serve"));
    }

    #[test]
    fn bind_help_for_permission_denied_is_friendly() {
        let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], 3211));
        let err = std::io::Error::new(ErrorKind::PermissionDenied, "permission denied");
        let msg = listener_bind_help(addr, "codex", &err);
        assert!(msg.contains("没有权限绑定"));
        assert!(msg.contains("管理员"));
    }

    #[test]
    #[cfg(windows)]
    fn bind_help_for_windows_10013_mentions_10013() {
        let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], 3211));
        let err = std::io::Error::from_raw_os_error(10013);
        let msg = listener_bind_help(addr, "codex", &err);
        assert!(msg.contains("10013"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_netstat_parser_extracts_listening_pid() {
        let sample = "\
Active Connections\n\
\n\
  Proto  Local Address          Foreign Address        State           PID\n\
  TCP    127.0.0.1:3211         0.0.0.0:0              LISTENING       4242\n\
  TCP    127.0.0.1:3212         0.0.0.0:0              LISTENING       1111\n\
  TCP    127.0.0.1:3211         0.0.0.0:0              LISTENING       4242\n\
";
        let pids = parse_windows_netstat_listening_pids(sample, 3211);
        assert_eq!(pids, vec![4242]);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_ss_parser_extracts_pid_and_name() {
        let sample = "\
State   Recv-Q  Send-Q   Local Address:Port   Peer Address:Port Process\n\
LISTEN  0       4096     127.0.0.1:3211       0.0.0.0:*     users:((\"codex-helper\",pid=1234,fd=3))\n\
";
        let owners = parse_linux_ss_listening_owners(sample, 3211);
        assert_eq!(
            owners,
            vec![PortOwner {
                pid: 1234,
                name: Some("codex-helper".to_string())
            }]
        );
    }

    #[test]
    #[cfg(unix)]
    fn unix_lsof_parser_extracts_pid_and_name() {
        let sample = "\
COMMAND   PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME\n\
node    7777 user   23u  IPv4 0x0      0t0     TCP 127.0.0.1:3211 (LISTEN)\n\
";
        let owners = parse_unix_lsof_owners(sample);
        assert_eq!(
            owners,
            vec![PortOwner {
                pid: 7777,
                name: Some("node".to_string())
            }]
        );
    }
}

#[cfg(test)]
mod service_refresh_target_tests {
    use super::*;
    use crate::service_receipt::{ServiceReceiptTransaction, service_receipt_path};
    use codex_helper_core::service_target::ServiceInstallGeneration;

    struct TestHome(PathBuf);

    impl TestHome {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "codex-helper-service-target-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(path.join("client")).expect("create target test home");
            Self(path)
        }

        fn install_receipt(
            &self,
            service: ServiceKind,
            admin_base_url: &str,
            platform: ServicePlatformBackend,
        ) -> ServiceReceipt {
            let receipt = ServiceReceipt::new(
                service,
                self.0.clone(),
                self.0.join("client"),
                admin_base_url,
                platform,
                ServiceInstallGeneration::generate(),
            )
            .expect("build service receipt");
            let mut transaction =
                ServiceReceiptTransaction::begin(self.0.clone()).expect("begin receipt write");
            transaction
                .replace(&receipt)
                .expect("write service receipt");
            receipt
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn foreign_platform() -> ServicePlatformBackend {
        match ServicePlatformBackend::current().expect("supported test platform") {
            ServicePlatformBackend::WindowsScheduledTask => {
                ServicePlatformBackend::MacosLaunchAgent
            }
            ServicePlatformBackend::MacosLaunchAgent | ServicePlatformBackend::LinuxSystemdUser => {
                ServicePlatformBackend::WindowsScheduledTask
            }
        }
    }

    #[test]
    fn receipt_discovery_uses_exact_default_or_custom_admin_authority() {
        let default_home = TestHome::new();
        let custom_home = TestHome::new();
        default_home.install_receipt(
            ServiceKind::Codex,
            "http://127.0.0.1:4211",
            ServicePlatformBackend::current().expect("supported platform"),
        );
        let custom_receipt = custom_home.install_receipt(
            ServiceKind::Claude,
            "http://127.0.0.1:5310",
            ServicePlatformBackend::current().expect("supported platform"),
        );

        let default = resolve_service_refresh_target(&default_home.0, ServiceKind::Codex)
            .expect("resolve default target");
        let custom = resolve_service_refresh_target(&custom_home.0, ServiceKind::Claude)
            .expect("resolve custom target");

        assert_eq!(default.endpoint.admin_base_url(), "http://127.0.0.1:4211");
        assert_eq!(custom.endpoint.admin_base_url(), "http://127.0.0.1:5310");
        assert_eq!(
            custom.receipt.install_generation(),
            custom_receipt.install_generation()
        );
        assert_eq!(custom.receipt.client_home(), custom_home.0.join("client"));
    }

    #[test]
    fn receipt_discovery_rejects_absent_legacy_mismatched_and_foreign_targets_without_fallback() {
        let selected = TestHome::new();
        let other = TestHome::new();
        other.install_receipt(
            ServiceKind::Codex,
            "http://127.0.0.1:6551",
            ServicePlatformBackend::current().expect("supported platform"),
        );

        assert!(matches!(
            resolve_service_refresh_target(&selected.0, ServiceKind::Codex),
            Err(ServiceRefreshTargetError::Receipt(
                ServiceReceiptError::Missing
            ))
        ));

        std::fs::write(
            service_receipt_path(&selected.0),
            br#"{"schema_version":0,"admin_base_url":"http://127.0.0.1:6551"}"#,
        )
        .expect("write legacy receipt");
        assert!(matches!(
            resolve_service_refresh_target(&selected.0, ServiceKind::Codex),
            Err(ServiceRefreshTargetError::Receipt(
                ServiceReceiptError::LegacySchema { .. }
            ))
        ));

        std::fs::remove_file(service_receipt_path(&selected.0)).expect("remove legacy receipt");
        selected.install_receipt(
            ServiceKind::Claude,
            "http://127.0.0.1:4310",
            ServicePlatformBackend::current().expect("supported platform"),
        );
        assert!(matches!(
            resolve_service_refresh_target(&selected.0, ServiceKind::Codex),
            Err(ServiceRefreshTargetError::ServiceMismatch)
        ));

        let foreign = TestHome::new();
        let foreign_receipt = ServiceReceipt::new(
            ServiceKind::Codex,
            foreign.0.clone(),
            foreign.0.join("client"),
            "http://127.0.0.1:4999",
            ServicePlatformBackend::current().expect("supported platform"),
            ServiceInstallGeneration::generate(),
        )
        .expect("build foreign receipt");
        std::fs::write(
            service_receipt_path(&selected.0),
            serde_json::to_vec(&foreign_receipt).expect("serialize foreign receipt"),
        )
        .expect("write foreign receipt");
        assert!(matches!(
            resolve_service_refresh_target(&selected.0, ServiceKind::Codex),
            Err(ServiceRefreshTargetError::Receipt(
                ServiceReceiptError::ForeignHelperHome
            ))
        ));
    }

    #[test]
    fn receipt_discovery_rejects_platform_mismatch() {
        let home = TestHome::new();
        home.install_receipt(
            ServiceKind::Codex,
            "http://127.0.0.1:4211",
            foreign_platform(),
        );

        assert!(matches!(
            resolve_service_refresh_target(&home.0, ServiceKind::Codex),
            Err(ServiceRefreshTargetError::PlatformMismatch)
        ));
    }
}

#[cfg(test)]
mod supervisor_tests {
    use super::*;

    #[test]
    fn restart_delay_grows_with_restarts_and_caps_out() {
        assert_eq!(supervisor_restart_delay_secs(1), 1);
        assert_eq!(supervisor_restart_delay_secs(2), 2);
        assert_eq!(supervisor_restart_delay_secs(3), 4);
        assert_eq!(supervisor_restart_delay_secs(6), 32);
        assert_eq!(supervisor_restart_delay_secs(9), 32);
    }

    #[test]
    fn owner_marker_makes_supervisor_runtime_visible_to_status() {
        let run_dir = std::env::temp_dir().join(format!(
            "codex-helper-supervisor-owner-{}-{}",
            std::process::id(),
            crate::logging::now_ms()
        ));
        let marker =
            RuntimeOwnerMarker::new_with_pid(RuntimeOwnerKind::Supervisor, "codex", 3211, 123, 456)
                .with_note("test supervisor owner");

        let path = crate::runtime_manager::write_owner_marker_to(&run_dir, &marker)
            .expect("write supervisor owner marker");
        assert_eq!(path, run_dir.join("codex-3211.owner.json"));

        let loaded = crate::runtime_manager::read_owner_marker_from(&run_dir, "codex", 3211)
            .expect("read supervisor owner marker")
            .expect("marker should exist");
        assert_eq!(loaded.owner, RuntimeOwnerKind::Supervisor);
        assert_eq!(
            loaded.lifecycle_mode,
            crate::runtime_manager::ProxyLifecycleMode::ResidentDaemon
        );
        assert_eq!(loaded.note.as_deref(), Some("test supervisor owner"));

        crate::runtime_manager::clear_owner_marker_from(&run_dir, "codex", 3211)
            .expect("clear supervisor owner marker");
    }
}

#[cfg(test)]
mod serve_startup_tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    struct ScopedEnv {
        saved: Vec<(String, Option<String>)>,
    }

    impl ScopedEnv {
        fn new() -> Self {
            Self { saved: Vec::new() }
        }

        unsafe fn set_path(&mut self, key: &str, value: &Path) {
            self.saved.push((key.to_string(), std::env::var(key).ok()));
            unsafe { std::env::set_var(key, value) };
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            for (key, old) in self.saved.drain(..).rev() {
                unsafe {
                    match old {
                        Some(value) => std::env::set_var(&key, value),
                        None => std::env::remove_var(&key),
                    }
                }
            }
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        match LOCK.get_or_init(|| Mutex::new(())).lock() {
            Ok(guard) => guard,
            Err(error) => error.into_inner(),
        }
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create test directory");
        }
        std::fs::write(path, contents).expect("write test file");
    }

    fn loaded_test_config(source: crate::config::HelperConfig) -> LoadedConfig {
        LoadedConfig { source }
    }

    fn loaded_runtime_test_config() -> LoadedConfig {
        let provider_id = "test".to_string();
        loaded_test_config(crate::config::HelperConfig {
            codex: crate::config::ServiceRouteConfig {
                providers: std::collections::BTreeMap::from([(
                    provider_id.clone(),
                    crate::config::ProviderConfig {
                        base_url: Some("http://127.0.0.1:9/v1".to_string()),
                        ..crate::config::ProviderConfig::default()
                    },
                )]),
                routing: Some(crate::config::RouteGraphConfig::ordered_failover(vec![
                    provider_id,
                ])),
                ..crate::config::ServiceRouteConfig::default()
            },
            ..crate::config::HelperConfig::default()
        })
    }

    fn empty_loaded_test_config() -> LoadedConfig {
        loaded_test_config(crate::config::HelperConfig::default())
    }

    fn reserve_admin_for_free_proxy_port() -> (u16, std::net::TcpListener) {
        for _ in 0..100 {
            let proxy = std::net::TcpListener::bind("127.0.0.1:0")
                .expect("reserve candidate proxy address");
            let proxy_port = proxy.local_addr().expect("candidate proxy address").port();
            let admin_addr = admin_loopback_addr_for_proxy_port(proxy_port);
            if let Ok(admin) = std::net::TcpListener::bind(admin_addr) {
                drop(proxy);
                return (proxy_port, admin);
            }
        }
        panic!("reserve a free proxy port with an occupiable admin port");
    }

    #[test]
    fn serve_startup_preparation_is_read_only() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-serve-startup-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let codex_home = root.join("codex");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        std::fs::create_dir_all(&codex_home).expect("create Codex home");

        let mut env = ScopedEnv::new();
        unsafe {
            env.set_path("CODEX_HELPER_HOME", &helper_home);
            env.set_path("CODEX_HOME", &codex_home);
        }

        let codex_config_path = codex_home.join("config.toml");
        let codex_auth_path = codex_home.join("auth.json");
        let models_cache_path = codex_home.join("models_cache.json");
        let codex_config = r#"model_provider = "external"

[model_providers.external]
name = "external"
base_url = "https://external.example.com/v1"
env_key = "EXTERNAL_API_KEY"
"#;
        let codex_auth = r#"{"EXTERNAL_API_KEY":"test-only"}"#;
        let models_cache = r#"{"models":[{"slug":"gpt-5.4","service_tiers":[]}]}"#;
        write_file(&codex_config_path, codex_config);
        write_file(&codex_auth_path, codex_auth);
        write_file(&models_cache_path, models_cache);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let loaded = runtime
            .block_on(load_serve_config())
            .expect("load serve config");
        let readiness = codex_startup_readiness_for_existing_switch(3211);

        assert!(loaded.source.codex.providers.is_empty());
        let switch_disabled = readiness
            .issues
            .iter()
            .find(|issue| {
                issue.kind == codex_integration::CodexStartupReadinessIssueKind::SwitchDisabled
            })
            .expect("external provider should require an explicit switch");
        assert!(switch_disabled.action.contains("switch on"));
        assert!(switch_disabled.action.contains("explicitly"));
        assert!(!switch_disabled.action.contains("restart codex-helper"));
        assert!(!helper_home.join("config.toml").exists());
        assert!(!helper_home.join("config.json").exists());
        assert!(!helper_home.join("state/codex-switch.json").exists());
        assert!(!codex_home.join("codex-helper-switch-state.json").exists());
        assert_eq!(
            std::fs::read_to_string(&codex_config_path).expect("read Codex config"),
            codex_config
        );
        assert_eq!(
            std::fs::read_to_string(&codex_auth_path).expect("read Codex auth"),
            codex_auth
        );
        assert_eq!(
            std::fs::read_to_string(&models_cache_path).expect("read models cache"),
            models_cache
        );

        let helper_config_path = helper_home.join("config.toml");
        let helper_config = "version = 6\n\n[notify]\nenabled = false\n";
        write_file(&helper_config_path, helper_config);
        let loaded = runtime
            .block_on(load_serve_config())
            .expect("load existing helper config");
        let _ = resolve_serve_tui_language(&loaded);
        assert_eq!(
            std::fs::read_to_string(&helper_config_path).expect("read helper config"),
            helper_config
        );

        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_runtime_store_failure_happens_before_listener_bind() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-local-runtime-store-order-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let state_dir = helper_home.join("state");
        std::fs::create_dir_all(&state_dir).expect("create helper state directory");
        std::fs::write(state_dir.join("state.sqlite"), b"not a sqlite database")
            .expect("write corrupt runtime store");

        let mut env = ScopedEnv::new();
        unsafe {
            env.set_path("CODEX_HELPER_HOME", &helper_home);
        }

        let occupied_proxy =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserve occupied proxy address");
        let proxy_addr = occupied_proxy.local_addr().expect("occupied proxy address");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let result = runtime.block_on(build_local_proxy_runtime(
            "codex",
            proxy_addr.ip(),
            proxy_addr.port(),
            false,
            empty_loaded_test_config(),
        ));
        let error = match result {
            Ok(_) => panic!("corrupt runtime store must prevent local startup"),
            Err(error) => error,
        };

        assert!(error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<codex_helper_core::runtime_store::RuntimeStoreError>(),
                Some(codex_helper_core::runtime_store::RuntimeStoreError::CorruptDatabase { .. })
            )
        }));

        drop(occupied_proxy);
        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn local_runtime_bind_errors_report_exact_proxy_and_admin_addresses() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-local-runtime-bind-error-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let mut env = ScopedEnv::new();
        unsafe {
            env.set_path("CODEX_HELPER_HOME", &helper_home);
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        runtime.block_on(async {
            let occupied_proxy =
                std::net::TcpListener::bind("127.0.0.1:0").expect("reserve occupied proxy address");
            let proxy_addr = occupied_proxy.local_addr().expect("occupied proxy address");
            let proxy_result = build_local_proxy_runtime(
                "codex",
                proxy_addr.ip(),
                proxy_addr.port(),
                false,
                loaded_runtime_test_config(),
            )
            .await;
            let proxy_error = match proxy_result {
                Ok(_) => panic!("occupied proxy address must fail"),
                Err(error) => error,
            };
            let proxy_bind_error = proxy_error
                .chain()
                .find_map(|cause| cause.downcast_ref::<ProxyListenerBindError>())
                .expect("preserve proxy bind error");
            assert_eq!(
                proxy_bind_error.kind(),
                crate::runtime_host::ProxyListenerKind::Proxy
            );
            assert_eq!(proxy_bind_error.addr(), proxy_addr);
            assert!(proxy_error.to_string().contains(&proxy_addr.to_string()));
            drop(occupied_proxy);

            let (proxy_port, occupied_admin) = reserve_admin_for_free_proxy_port();
            let admin_addr = occupied_admin.local_addr().expect("occupied admin address");
            let admin_result = build_local_proxy_runtime(
                "codex",
                IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                proxy_port,
                false,
                loaded_runtime_test_config(),
            )
            .await;
            let admin_error = match admin_result {
                Ok(_) => panic!("occupied admin address must fail"),
                Err(error) => error,
            };
            let admin_bind_error = admin_error
                .chain()
                .find_map(|cause| cause.downcast_ref::<ProxyListenerBindError>())
                .expect("preserve admin bind error");
            assert_eq!(
                admin_bind_error.kind(),
                crate::runtime_host::ProxyListenerKind::Admin
            );
            assert_eq!(admin_bind_error.addr(), admin_addr);
            assert!(admin_error.to_string().contains(&admin_addr.to_string()));
            drop(occupied_admin);
        });

        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }
}
