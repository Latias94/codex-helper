use crate::cli_types::{
    Cli, CliError, CliResult, Command, DaemonCommand, NotifyCommand, RelayCommand, ServiceCommand,
    SwitchCommand, UsageCommand, UsageSource, reject_legacy_switch_mode,
};
use crate::codex_integration;
use crate::commands;
#[cfg(test)]
use crate::config::save_helper_config;
use crate::config::{
    CodexClientPatchConfig, CodexClientPatchOverrides, HelperConfig, LoadedConfig,
    RelayTargetConfig, ServiceKind, load_config, load_config_with_source, mutate_helper_config,
};
use crate::control_plane_client::{
    ControlPlaneClient, ControlPlaneEndpoint, ControlPlaneError, configured_local_admin_token_env,
    normalize_base_url,
};
use crate::dashboard_core::{OperatorReadModel, OperatorReadStatus};
use crate::notify;
use crate::proxy::admin_loopback_addr_for_proxy_port;
use crate::runtime_host::{
    ProxyListenerBindError, ProxyRuntime, ProxyRuntimeOptions, RunningProxyRuntime,
    build_proxy_runtime_from_loaded_with_options,
};
use crate::runtime_manager::{
    ProxyLifecycleMode, RuntimeOwnerKind, RuntimeOwnerLease, RuntimeOwnerMarker, read_owner_marker,
    read_owner_marker_best_effort,
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
use codex_helper_core::credentials::{CredentialAggregateReadiness, CredentialSourceCapabilities};
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
    auto_manage_codex_switch: bool,
}

impl ServeRuntimeOptions {
    fn owner_kind(self) -> Option<RuntimeOwnerKind> {
        if self.service_managed {
            Some(RuntimeOwnerKind::SystemService)
        } else if self.desktop_managed {
            Some(RuntimeOwnerKind::Desktop)
        } else if self.supervisor_managed {
            None
        } else {
            Some(RuntimeOwnerKind::ManualCli)
        }
    }

    fn owner_marker(self, service_name: &str, port: u16) -> Option<RuntimeOwnerMarker> {
        let marker = RuntimeOwnerMarker::new(self.owner_kind()?, service_name, port);
        Some(if self.is_resident() {
            marker
        } else {
            marker.with_lifecycle_mode(ProxyLifecycleMode::EphemeralConsole)
        })
    }

    fn is_resident(self) -> bool {
        self.resident || self.supervisor_managed || self.desktop_managed || self.service_managed
    }

    fn should_auto_manage_codex_switch(self, service_name: &str) -> bool {
        self.auto_manage_codex_switch
            && service_name == "codex"
            && !self.supervisor_managed
            && !self.desktop_managed
            && !self.service_managed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CliEntrypoint {
    CodexHelper,
    Ch,
}

impl CliEntrypoint {
    const fn auto_manages_codex_client(self) -> bool {
        matches!(self, Self::Ch)
    }
}

pub async fn run_cli() -> CliResult<()> {
    run_codex_cli(CliEntrypoint::CodexHelper).await
}

pub async fn run_ch_cli() -> CliResult<()> {
    run_codex_cli(CliEntrypoint::Ch).await
}

async fn run_codex_cli(entrypoint: CliEntrypoint) -> CliResult<()> {
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
            handle_daemon_cmd(cmd, entrypoint.auto_manages_codex_client()).await?;
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
            handle_relay_cmd(cmd, entrypoint.auto_manages_codex_client()).await?;
            return Ok(());
        }
        Command::Switch { cmd } => {
            match cmd {
                SwitchCommand::On {
                    port,
                    base_url,
                    preset,
                    legacy_mode,
                    responses_websocket,
                    compaction,
                    translate_models,
                    hosted_image_generation,
                } => {
                    reject_legacy_switch_mode(legacy_mode)?;
                    do_switch_on(
                        port,
                        base_url,
                        CodexSwitchClientPatchSelection {
                            overrides: CodexClientPatchOverrides {
                                preset: preset.map(Into::into),
                                responses_websocket,
                                compaction: compaction.map(Into::into),
                                translate_models,
                                hosted_image_generation: hosted_image_generation.map(Into::into),
                            },
                        },
                    )
                    .await?;
                }
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
            source,
            cmd,
        } => {
            let service_name = resolve_cli_service_name(codex, claude).await?;
            let port = port.unwrap_or_else(|| default_proxy_port_for_service(service_name));
            let client = local_control_plane_client(port)?;
            let model = if source == UsageSource::Store || matches!(cmd, UsageCommand::Quota { .. })
            {
                None
            } else {
                Some(client.refresh_operator_read_model(service_name, None).await)
            };
            commands::usage::handle_usage_cmd(cmd, source, service_name, &client, model).await?;
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
                    auto_manage_codex_switch: entrypoint.auto_manages_codex_client(),
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
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(std::io::stderr)
            .init();
        None
    }
}

const RUNTIME_LOG_FILE_NAME: &str = "runtime.log";
const DEFAULT_RUNTIME_LOG_MAX_BYTES: u64 = 20 * 1024 * 1024;
const DEFAULT_RUNTIME_LOG_MAX_FILES: usize = 10;

async fn handle_daemon_cmd(cmd: DaemonCommand, auto_manage_codex_switch: bool) -> CliResult<()> {
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
            supervise_daemon(
                service_name,
                host,
                port,
                max_restarts,
                auto_manage_codex_switch,
            )
            .await
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

async fn handle_relay_cmd(cmd: RelayCommand, auto_manage_codex_switch: bool) -> CliResult<()> {
    match cmd {
        RelayCommand::Add {
            name,
            proxy_url,
            admin_url,
            admin_token_env,
            codex,
            claude,
            preset,
            legacy_mode,
            responses_websocket,
            compaction,
            translate_models,
            hosted_image_generation,
        } => {
            reject_legacy_switch_mode(legacy_mode)?;
            let service = relay_service_from_flags(codex, claude)?;
            add_relay_target(
                name,
                proxy_url,
                admin_url,
                admin_token_env,
                service,
                CodexClientPatchOverrides {
                    preset: preset.map(Into::into),
                    responses_websocket,
                    compaction: compaction.map(Into::into),
                    translate_models,
                    hosted_image_generation: hosted_image_generation.map(Into::into),
                },
            )
            .await
        }
        RelayCommand::List => list_relay_targets().await,
        RelayCommand::Status { target, json } => relay_status(target, json).await,
        RelayCommand::Off => handle_relay_off(auto_manage_codex_switch),
        RelayCommand::Use {
            target,
            no_tui,
            attach_only,
        } => use_relay_target(target, no_tui, attach_only, auto_manage_codex_switch).await,
        RelayCommand::Target(args) => {
            let (target, no_tui, attach_only) = parse_relay_target_shorthand(args)?;
            use_relay_target(target, no_tui, attach_only, auto_manage_codex_switch).await
        }
    }
}

fn handle_relay_off(auto_manage_codex_switch: bool) -> CliResult<()> {
    if !auto_manage_codex_switch {
        return Err(CliError::Other(
            "`codex-helper relay off` does not modify Codex client configuration; run `codex-helper switch off` explicitly"
                .to_string(),
        ));
    }
    do_switch_off()
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
    client_patch: CodexClientPatchOverrides,
) -> CliResult<()> {
    if service == ServiceKind::Claude && !client_patch.is_empty() {
        return Err(CliError::Other(
            "relay target client patch options are only supported for Codex".to_string(),
        ));
    }
    let target = relay_target_config_from_args(
        Some(service),
        proxy_url,
        admin_url,
        admin_token_env,
        Some(client_patch),
    )
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
    mutate_helper_config(move |source| {
        if let Some(overrides) = target.client_patch {
            source
                .codex
                .client_patch
                .unwrap_or_default()
                .with_field_overrides(overrides)
                .validate()
                .map_err(|error| anyhow::anyhow!("invalid relay target client patch: {error}"))?;
        }
        source.relay_targets.insert(name, target);
        Ok(())
    })
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
        println!("{}", relay_target_list_line(&cfg, &name));
    }
    Ok(())
}

fn relay_target_list_line(cfg: &HelperConfig, name: &str) -> String {
    match resolve_relay_target(cfg, name) {
        Ok(target) => {
            let service = service_name_for_kind(target.service);
            let admin = target.admin_url.as_deref().unwrap_or("<unavailable>");
            let marker = if target.is_local() {
                "built-in"
            } else {
                "configured"
            };
            format!(
                "  {name:<16} {service:<6} proxy={} admin={} ({marker})",
                target.proxy_url, admin
            )
        }
        Err(_) => {
            let Some(target) = cfg.relay_targets.get(name) else {
                return format!(
                    "  {name:<16} {:<6} proxy=<invalid> admin=<unavailable> (migration-required)",
                    "unknown"
                );
            };
            let service = service_name_for_kind(target.service.unwrap_or(ServiceKind::Codex));
            let proxy =
                normalize_base_url(&target.proxy_url).unwrap_or_else(|| "<invalid>".to_string());
            let admin = target
                .admin_url
                .as_deref()
                .and_then(normalize_base_url)
                .unwrap_or_else(|| "<unavailable>".to_string());
            format!("  {name:<16} {service:<6} proxy={proxy} admin={admin} (migration-required)")
        }
    }
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
                    "client_patch": status.client_patch,
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

async fn use_relay_target(
    target_name: String,
    no_tui: bool,
    attach_only: bool,
    auto_manage_codex_switch: bool,
) -> CliResult<()> {
    let cfg = load_config()
        .await
        .map_err(|err| CliError::Configuration(err.to_string()))?;
    let target = resolve_relay_target(&cfg, &target_name)
        .map_err(|err| CliError::Configuration(err.to_string()))?;
    validate_relay_use_mode(&target, no_tui, attach_only, auto_manage_codex_switch)?;
    let manage_codex_client =
        relay_should_manage_codex_client(&target, attach_only, auto_manage_codex_switch);
    if target.is_local() && !attach_only {
        let service_name = service_name_for_kind(target.service);
        let port = relay_proxy_port(&target)
            .unwrap_or_else(|| default_proxy_port_for_service(service_name));
        if manage_codex_client {
            println!(
                "Starting relay target '{}' on 127.0.0.1:{}; Codex will switch after the runtime is ready.",
                target.name, port
            );
        } else {
            println!(
                "Starting relay target '{}' on 127.0.0.1:{}; client configuration will remain unchanged.",
                target.name, port
            );
        }
        return run_server(
            service_name,
            IpAddr::from([127, 0, 0, 1]),
            port,
            ServeRuntimeOptions {
                enable_tui: !no_tui,
                auto_manage_codex_switch: manage_codex_client,
                ..ServeRuntimeOptions::default()
            },
        )
        .await
        .map_err(|err| CliError::Other(err.to_string()));
    }

    if manage_codex_client {
        let configured = resolve_relay_codex_client_patch(
            cfg.codex.client_patch.unwrap_or_default(),
            target.client_patch,
            CodexSwitchClientPatchSelection::default(),
        );
        let validated_base_url = ValidatedCodexBaseUrl::parse(&target.proxy_url)
            .map_err(|error| CliError::CodexConfig(error.to_string()))?;
        preflight_configured_relay_switch_target(&target).await?;
        apply_codex_switch(
            validated_base_url,
            CodexSwitchClientPatchSelection::default(),
            configured,
        )?;
    } else {
        print_relay_client_config_hint(&target, attach_only);
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
    auto_manage_codex_switch: bool,
) -> CliResult<()> {
    if no_tui && attach_only {
        return Err(CliError::Other(
            "`--no-tui` and `--attach-only` together have no effect".to_string(),
        ));
    }
    if no_tui
        && !target.is_local()
        && !relay_should_manage_codex_client(target, attach_only, auto_manage_codex_switch)
    {
        return Err(CliError::Other(
            "`--no-tui` has no client action for this relay entrypoint; use `ch relay` to manage Codex or omit it to attach the read-only TUI"
                .to_string(),
        ));
    }
    Ok(())
}

fn relay_should_manage_codex_client(
    target: &ResolvedRelayTarget,
    attach_only: bool,
    auto_manage_codex_switch: bool,
) -> bool {
    auto_manage_codex_switch && !attach_only && target.service == ServiceKind::Codex
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
            client_patch: None,
            built_in_local,
        }
    }

    #[test]
    fn remote_codex_relay_client_management_follows_entrypoint() {
        let target = relay_target(false);
        assert!(relay_should_manage_codex_client(&target, false, true));
        assert!(!relay_should_manage_codex_client(&target, false, false));
        assert!(!relay_should_manage_codex_client(&target, true, true));
        validate_relay_use_mode(&target, true, false, true)
            .expect("ch remote Codex --no-tui performs a client switch");

        let error = validate_relay_use_mode(&target, true, false, false)
            .expect_err("ordinary codex-helper relay has no implicit client action");
        assert!(error.to_string().contains("ch relay"), "{error}");
    }

    #[test]
    fn remote_claude_relay_target_rejects_no_tui_without_a_client_action() {
        let mut target = relay_target(false);
        target.service = ServiceKind::Claude;

        let error = validate_relay_use_mode(&target, true, false, true)
            .expect_err("remote Claude --no-tui has no client action");
        assert!(error.to_string().contains("no client action"), "{error}");
    }

    #[test]
    fn local_relay_client_management_follows_entrypoint() {
        let target = relay_target(true);
        validate_relay_use_mode(&target, true, false, false)
            .expect("local --no-tui should start without the console");
        assert!(relay_should_manage_codex_client(&target, false, true));
        assert!(!relay_should_manage_codex_client(&target, false, false));
    }

    #[test]
    fn relay_target_rejects_no_tui_with_attach_only() {
        let error = validate_relay_use_mode(&relay_target(true), true, true, true)
            .expect_err("no-tui attach-only has no observable behavior");
        assert!(error.to_string().contains("have no effect"));
    }

    #[test]
    fn relay_off_parses_but_only_ch_may_apply_its_implicit_switch_action() {
        let parsed =
            Cli::try_parse_from(["codex-helper", "relay", "off"]).expect("parse relay off command");
        assert!(matches!(
            parsed.command,
            Some(Command::Relay {
                cmd: RelayCommand::Off
            })
        ));

        let error = handle_relay_off(false)
            .expect_err("ordinary codex-helper relay off must preserve Codex client files");
        assert!(error.to_string().contains("codex-helper switch off"));
    }
}

fn print_relay_client_config_hint(target: &ResolvedRelayTarget, attach_only: bool) {
    println!(
        "Relay target '{}' is attached without changing client configuration{}.",
        target.name,
        if attach_only { " (--attach-only)" } else { "" }
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

const SUPERVISOR_OWNER_ID_ENV: &str = "CODEX_HELPER_INTERNAL_SUPERVISOR_OWNER_ID";
const SUPERVISOR_CHILD_GENERATION_ENV: &str = "CODEX_HELPER_INTERNAL_SUPERVISOR_CHILD_GENERATION";
const SUPERVISOR_READY_VERSION: u32 = 1;
const SUPERVISOR_READY_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct SupervisorChildReady {
    version: u32,
    supervisor_owner_id: String,
    child_generation: String,
    child_pid: u32,
    service_name: String,
    proxy_port: u16,
    admin_port: u16,
}

struct SupervisorReadyExpectation<'a> {
    path: &'a Path,
    owner_id: &'a str,
    child_generation: &'a str,
    child_pid: u32,
    service_name: &'a str,
    proxy_port: u16,
}

enum SupervisorReadiness {
    Ready,
    Exited(std::process::ExitStatus),
    Shutdown,
}

fn supervisor_ready_matches(
    ready: &SupervisorChildReady,
    expected: &SupervisorReadyExpectation<'_>,
) -> bool {
    ready.version == SUPERVISOR_READY_VERSION
        && ready.supervisor_owner_id == expected.owner_id
        && ready.child_generation == expected.child_generation
        && ready.child_pid == expected.child_pid
        && ready.service_name == expected.service_name
        && ready.proxy_port == expected.proxy_port
        && ready.admin_port == admin_loopback_addr_for_proxy_port(expected.proxy_port).port()
}

fn supervisor_child_ready_path(
    service_name: &str,
    proxy_port: u16,
    child_generation: &str,
) -> PathBuf {
    crate::runtime_manager::runtime_run_dir().join(format!(
        "{service_name}-{proxy_port}-{child_generation}.supervisor-ready.json"
    ))
}

fn remove_file_if_present(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => tracing::warn!("failed to remove supervisor file {:?}: {error}", path),
    }
}

fn publish_supervisor_child_ready(
    service_name: &str,
    proxy_port: u16,
    admin_port: u16,
) -> anyhow::Result<()> {
    let (owner_id, child_generation) = match (
        std::env::var(SUPERVISOR_OWNER_ID_ENV).ok(),
        std::env::var(SUPERVISOR_CHILD_GENERATION_ENV).ok(),
    ) {
        (Some(owner_id), Some(child_generation)) => (owner_id, child_generation),
        (None, None) => return Ok(()),
        _ => anyhow::bail!("supervisor child readiness environment is incomplete"),
    };
    uuid::Uuid::parse_str(&owner_id).context("validate supervisor owner generation")?;
    uuid::Uuid::parse_str(&child_generation).context("validate supervisor child generation")?;
    let path = supervisor_child_ready_path(service_name, proxy_port, &child_generation);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create supervisor readiness dir {:?}", parent))?;
    }
    let ready = SupervisorChildReady {
        version: SUPERVISOR_READY_VERSION,
        supervisor_owner_id: owner_id,
        child_generation,
        child_pid: std::process::id(),
        service_name: service_name.to_string(),
        proxy_port,
        admin_port,
    };
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    std::fs::write(&temp, serde_json::to_vec(&ready)?)
        .with_context(|| format!("write supervisor readiness temp file {:?}", temp))?;
    if let Err(error) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("publish supervisor readiness {:?}", path));
    }
    Ok(())
}

async fn wait_for_supervisor_parent_eof() {
    use tokio::io::AsyncReadExt as _;

    let mut stdin = tokio::io::stdin();
    let mut buffer = [0u8; 1];
    loop {
        match stdin.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

async fn wait_for_supervised_child_readiness(
    child: &mut tokio::process::Child,
    shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
    expected: SupervisorReadyExpectation<'_>,
) -> CliResult<SupervisorReadiness> {
    let deadline = tokio::time::Instant::now() + SUPERVISOR_READY_TIMEOUT;
    loop {
        if *shutdown_rx.borrow() {
            return Ok(SupervisorReadiness::Shutdown);
        }
        if let Some(status) = child.try_wait().map_err(|error| {
            CliError::Other(format!("inspect resident proxy child status: {error}"))
        })? {
            return Ok(SupervisorReadiness::Exited(status));
        }

        match std::fs::read_to_string(expected.path) {
            Ok(text) => {
                let ready =
                    serde_json::from_str::<SupervisorChildReady>(&text).map_err(|error| {
                        CliError::Other(format!(
                            "parse supervised child readiness {:?}: {error}",
                            expected.path
                        ))
                    })?;
                if !supervisor_ready_matches(&ready, &expected) {
                    return Err(CliError::Other(
                        "supervised child readiness identity did not match the spawned child"
                            .to_string(),
                    ));
                }
                if let Ok(model) =
                    read_local_operator_model(expected.service_name, expected.proxy_port).await
                    && model.service_name == expected.service_name
                    && model.status == OperatorReadStatus::Ready
                {
                    return Ok(SupervisorReadiness::Ready);
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(CliError::Other(format!(
                    "read supervised child readiness {:?}: {error}",
                    expected.path
                )));
            }
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::Other(format!(
                "supervised child did not become ready within {} seconds",
                SUPERVISOR_READY_TIMEOUT.as_secs()
            )));
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            changed = shutdown_rx.changed() => {
                let _ = changed;
                return Ok(SupervisorReadiness::Shutdown);
            }
        }
    }
}

async fn stop_supervised_child(child: &mut tokio::process::Child) {
    drop(child.stdin.take());
    match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => tracing::warn!("failed to wait for supervised child: {error}"),
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
}

async fn ensure_ch_codex_route(port: u16) -> anyhow::Result<()> {
    if let codex_helper_core::codex_onboarding::CodexOnboardingOutcome::Imported {
        provider_id,
        config_path,
    } = codex_helper_core::codex_onboarding::ensure_default_codex_route(port)
        .await
        .context("prepare the first ch provider route")?
    {
        tracing::info!(
            provider = %provider_id,
            config = %config_path.display(),
            "imported the active Codex provider for first ch startup"
        );
    }
    Ok(())
}

async fn supervise_daemon(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    max_restarts: u32,
    auto_manage_codex_switch: bool,
) -> CliResult<()> {
    let exe = std::env::current_exe()
        .map_err(|err| CliError::Other(format!("failed to locate current executable: {err}")))?;
    if auto_manage_codex_switch && service_name == "codex" {
        ensure_ch_codex_route(port)
            .await
            .map_err(|error| CliError::Configuration(format!("{error:#}")))?;
    }
    let crash_marker_path = supervisor_crash_marker_path(service_name, port);
    let owner_marker = RuntimeOwnerMarker::new(RuntimeOwnerKind::Supervisor, service_name, port)
        .with_note("supervisor is managing a resident proxy child");
    let owner_lease = RuntimeOwnerLease::acquire(&owner_marker)
        .map_err(|error| CliError::Other(format!("acquire supervisor ownership: {error}")))?;
    let client_patch = if auto_manage_codex_switch && service_name == "codex" {
        Some(
            load_config()
                .await
                .map_err(|error| CliError::Configuration(error.to_string()))?
                .codex
                .client_patch
                .unwrap_or_default(),
        )
    } else {
        None
    };
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let shutdown_task = tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });
    let mut restart_count = 0u32;
    let mut switch_resolved = false;
    let mut switch_guard = None;
    println!(
        "{} supervising {} resident proxy on http://{}:{}",
        "[OK]".green(),
        service_name,
        host,
        port
    );

    let run_result: CliResult<()> = async {
        loop {
            let child_generation = uuid::Uuid::new_v4().to_string();
            let ready_path = supervisor_child_ready_path(service_name, port, &child_generation);
            remove_file_if_present(&ready_path);
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

            let mut command = tokio::process::Command::new(&exe);
            command
                .args(&args)
                .env(SUPERVISOR_OWNER_ID_ENV, owner_lease.instance_id())
                .env(SUPERVISOR_CHILD_GENERATION_ENV, &child_generation)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .kill_on_drop(true);
            let mut child = command.spawn().map_err(|err| {
                CliError::Other(format!(
                    "failed to spawn resident proxy child {:?} {:?}: {err}",
                    exe, args
                ))
            })?;
            let child_pid = child.id().ok_or_else(|| {
                CliError::Other("resident proxy child did not expose a process ID".to_string())
            })?;

            let readiness = wait_for_supervised_child_readiness(
                &mut child,
                &mut shutdown_rx,
                SupervisorReadyExpectation {
                    path: &ready_path,
                    owner_id: owner_lease.instance_id(),
                    child_generation: &child_generation,
                    child_pid,
                    service_name,
                    proxy_port: port,
                },
            )
            .await;
            remove_file_if_present(&ready_path);

            let failure = match readiness {
                Ok(SupervisorReadiness::Ready) => {
                    if !switch_resolved && let Some(client_patch) = client_patch {
                        if let Err(error) = preflight_local_admin_switch_target(port).await {
                            stop_supervised_child(&mut child).await;
                            return Err(error);
                        }
                        let outcome = match codex_switch::acquire_ephemeral_local_codex(
                            &owner_lease,
                            client_patch,
                        ) {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                stop_supervised_child(&mut child).await;
                                return Err(CliError::CodexConfig(format!(
                                    "supervised proxy became ready but Codex auto-switch failed: {error}"
                                )));
                            }
                        };
                        switch_resolved = true;
                        switch_guard = outcome.restore_lease.map(|restore_lease| {
                            ForegroundCodexSwitchGuard {
                                restore_lease: Some(restore_lease),
                            }
                        });
                    }

                    tokio::select! {
                        status = child.wait() => {
                            let status = status.map_err(|error| CliError::Other(
                                format!("resident proxy child wait failed: {error}")
                            ))?;
                            if status.success() {
                                clear_supervisor_crash_marker(&crash_marker_path);
                                println!(
                                    "{} {} resident proxy exited cleanly; supervisor is stopping",
                                    "[OK]".green(),
                                    service_name
                                );
                                return Ok(());
                            }
                            Some(status.to_string())
                        }
                        changed = shutdown_rx.changed() => {
                            let _ = changed;
                            println!(
                                "{} stopping supervisor and resident proxy child",
                                "[INFO]".cyan()
                            );
                            stop_supervised_child(&mut child).await;
                            return Ok(());
                        }
                    }
                }
                Ok(SupervisorReadiness::Exited(status)) => Some(format!(
                    "child exited before readiness with {status}"
                )),
                Ok(SupervisorReadiness::Shutdown) => {
                    stop_supervised_child(&mut child).await;
                    return Ok(());
                }
                Err(error) => {
                    stop_supervised_child(&mut child).await;
                    Some(format!("readiness failed: {error}"))
                }
            };

            let failure = failure.expect("non-terminal supervisor cycle has a failure");
            restart_count = restart_count.saturating_add(1);
            record_supervisor_crash_marker(
                &crash_marker_path,
                service_name,
                host,
                port,
                restart_count,
                max_restarts,
                &failure,
            );
            if restart_count > max_restarts {
                return Err(CliError::Other(format!(
                    "{} resident proxy failed too many times ({restart_count}/{max_restarts}); last failure: {failure}",
                    service_name
                )));
            }

            let delay_secs = supervisor_restart_delay_secs(restart_count);
            println!(
                "{} {} resident proxy failed ({failure}); restart {restart_count}/{max_restarts} in {delay_secs}s",
                "[WARN]".yellow(),
                service_name
            );
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(delay_secs)) => {}
                changed = shutdown_rx.changed() => {
                    let _ = changed;
                    return Ok(());
                }
            }
        }
    }
    .await;
    shutdown_task.abort();

    let restore_result = match switch_guard.as_mut() {
        Some(guard) => guard
            .finalize()
            .map_err(|error| CliError::CodexConfig(error.to_string())),
        None => Ok(()),
    };
    match (run_result, restore_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(run_error), Err(restore_error)) => Err(CliError::Other(format!(
            "{run_error}; Codex client restore also failed: {restore_error}"
        ))),
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

const CH_SERVICE_ATTACH_TIMEOUT: Duration = Duration::from_secs(15);
const CH_SERVICE_ATTACH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CH_SERVICE_ATTACH_PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const CH_SERVICE_ATTACH_TCP_TIMEOUT: Duration = Duration::from_millis(200);

async fn tcp_endpoint_is_reachable(addr: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(
            CH_SERVICE_ATTACH_TCP_TIMEOUT,
            tokio::net::TcpStream::connect(addr),
        )
        .await,
        Ok(Ok(_))
    )
}

fn validate_ch_service_attach_receipt(
    receipt: &ServiceReceipt,
    service_name: &str,
    port: u16,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        receipt.service() == ServiceKind::Codex && service_name == "codex",
        "the native service receipt does not identify the local Codex service"
    );
    anyhow::ensure!(
        ServicePlatformBackend::current() == Some(receipt.platform_backend()),
        "the native service receipt belongs to a different platform backend"
    );
    anyhow::ensure!(
        receipt.admin_base_url() == daemon_admin_base_url_for_proxy_port(port),
        "the native service receipt targets a different admin endpoint"
    );

    anyhow::ensure!(
        ch_attach_paths_identify_same_location(
            receipt.client_home(),
            crate::config::codex_home().as_path(),
        )?,
        "the running native service uses a different Codex home"
    );
    Ok(())
}

fn ch_attach_paths_identify_same_location(left: &Path, right: &Path) -> anyhow::Result<bool> {
    let left = resolve_ch_attach_path_identity(left)
        .with_context(|| format!("resolve path identity for {}", left.display()))?;
    let right = resolve_ch_attach_path_identity(right)
        .with_context(|| format!("resolve path identity for {}", right.display()))?;

    #[cfg(windows)]
    {
        Ok(left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy()))
    }
    #[cfg(not(windows))]
    {
        Ok(left == right)
    }
}

fn resolve_ch_attach_path_identity(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .with_context(|| format!("resolve current directory for {}", path.display()))?
            .join(path)
    };
    let mut existing = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let name = existing.file_name().with_context(|| {
                    format!(
                        "no existing path ancestor is available for {}",
                        absolute.display()
                    )
                })?;
                missing.push(name.to_os_string());
                existing = existing.parent().with_context(|| {
                    format!(
                        "no existing path ancestor is available for {}",
                        absolute.display()
                    )
                })?;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect path identity for {}", existing.display()));
            }
        }
    }

    let mut resolved = std::fs::canonicalize(existing)
        .with_context(|| format!("canonicalize path identity for {}", existing.display()))?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn read_ch_service_owner_marker_advisory(
    service_name: &str,
    port: u16,
) -> Option<RuntimeOwnerMarker> {
    match read_owner_marker(service_name, port) {
        Ok(marker) => marker,
        Err(error) => {
            tracing::warn!(
                service = service_name,
                proxy_port = port,
                "ignoring unreadable runtime owner marker while verifying the signed native service identity: {error}"
            );
            None
        }
    }
}

fn validate_ch_service_owner_marker(
    marker: &RuntimeOwnerMarker,
    service_name: &str,
    port: u16,
) -> anyhow::Result<()> {
    let admin_addr = admin_loopback_addr_for_proxy_port(port);
    anyhow::ensure!(
        marker.owner == RuntimeOwnerKind::SystemService
            && marker.lifecycle_mode == ProxyLifecycleMode::ResidentDaemon
            && marker.service_name == service_name
            && marker.proxy_port == port
            && marker.admin_port == admin_addr.port()
            && !marker.instance_id.is_empty()
            && marker.pid != 0,
        "refusing to attach ch because the runtime owner marker does not match the expected native service"
    );
    Ok(())
}

fn validate_ch_service_runtime_identity(
    runtime: &codex_helper_core::service_target::LocalServiceRuntimeReadResponse,
    receipt: &ServiceReceipt,
    service_name: &str,
    proxy_addr: SocketAddr,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        runtime.identity.service == receipt.service()
            && &runtime.identity.install_generation == receipt.install_generation()
            && runtime.operator.service_name == service_name
            && ch_attach_paths_identify_same_location(
                runtime.identity.helper_home.as_path(),
                receipt.helper_home(),
            )?
            && ch_attach_paths_identify_same_location(
                runtime.identity.client_home.as_path(),
                receipt.client_home(),
            )?
            && receipt.admin_base_url() == daemon_admin_base_url_for_proxy_port(proxy_addr.port()),
        "the native service receipt and signed runtime identity do not match"
    );
    Ok(())
}

async fn wait_for_ch_service_runtime(
    receipt: ServiceReceipt,
    proxy_addr: SocketAddr,
) -> anyhow::Result<codex_helper_core::service_target::LocalServiceRuntimeReadResponse> {
    let deadline = tokio::time::Instant::now() + CH_SERVICE_ATTACH_TIMEOUT;

    loop {
        let proxy_reachable = tcp_endpoint_is_reachable(proxy_addr).await;
        let last_error = match tokio::time::timeout(
            CH_SERVICE_ATTACH_PROBE_TIMEOUT,
            read_service_runtime_for_receipt(receipt.clone()),
        )
        .await
        {
            Ok(Ok(runtime)) if proxy_reachable => return Ok(runtime),
            Ok(Ok(_)) => format!("the native service proxy at {proxy_addr} is not reachable"),
            Ok(Err(error)) => error.to_string(),
            Err(_) => format!(
                "the native service identity probe exceeded {}ms",
                CH_SERVICE_ATTACH_PROBE_TIMEOUT.as_millis()
            ),
        };

        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "refusing to attach ch because the native service could not be verified within {}ms: {last_error}",
                CH_SERVICE_ATTACH_TIMEOUT.as_millis()
            );
        }
        tokio::time::sleep(CH_SERVICE_ATTACH_POLL_INTERVAL).await;
    }
}

async fn try_attach_ch_to_running_service(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    options: ServeRuntimeOptions,
    interactive: bool,
) -> anyhow::Result<bool> {
    try_attach_ch_to_running_service_with_platform_state(
        service_name,
        host,
        port,
        options,
        interactive,
        service_manager::current_service_runtime_state,
    )
    .await
}

fn service_state_can_own_starting_endpoint(state: service_manager::ServiceRuntimeState) -> bool {
    matches!(
        state,
        service_manager::ServiceRuntimeState::Running
            | service_manager::ServiceRuntimeState::Starting
    )
}

async fn try_attach_ch_to_running_service_with_platform_state<F>(
    service_name: &'static str,
    host: IpAddr,
    port: u16,
    options: ServeRuntimeOptions,
    interactive: bool,
    platform_state: F,
) -> anyhow::Result<bool>
where
    F: FnOnce() -> CliResult<service_manager::ServiceRuntimeState>,
{
    if !options.should_auto_manage_codex_switch(service_name)
        || host != IpAddr::from([127, 0, 0, 1])
    {
        return Ok(false);
    }

    let owner_marker = read_ch_service_owner_marker_advisory(service_name, port);
    if let Some(owner_marker) = owner_marker.as_ref() {
        validate_ch_service_owner_marker(owner_marker, service_name, port)?;
    }

    let proxy_addr = SocketAddr::from((host, port));
    let admin_addr = admin_loopback_addr_for_proxy_port(port);
    let (proxy_reachable, admin_reachable) = tokio::join!(
        tcp_endpoint_is_reachable(proxy_addr),
        tcp_endpoint_is_reachable(admin_addr),
    );

    let helper_home = crate::config::proxy_home_dir();
    let receipt = match read_service_receipt(&helper_home) {
        Ok(receipt) => receipt,
        Err(ServiceReceiptError::Missing) => return Ok(false),
        Err(error) => {
            return Err(error).context("read the native service receipt before attaching ch");
        }
    };
    validate_ch_service_attach_receipt(&receipt, service_name, port)?;
    if !proxy_reachable && !admin_reachable {
        let state = platform_state().map_err(|error| {
            anyhow::anyhow!(
                "inspect the native service state before deciding whether ch may start a foreground runtime: {error}"
            )
        })?;
        anyhow::ensure!(
            state != service_manager::ServiceRuntimeState::Unknown,
            "refusing to start a foreground ch runtime because the native service state could not be determined"
        );
        if !service_state_can_own_starting_endpoint(state) {
            return Ok(false);
        }
    }
    let runtime = wait_for_ch_service_runtime(receipt.clone(), proxy_addr).await?;
    validate_ch_service_runtime_identity(&runtime, &receipt, service_name, proxy_addr)?;
    let current_owner_marker = read_ch_service_owner_marker_advisory(service_name, port);
    if let Some(current_owner_marker) = current_owner_marker.as_ref() {
        validate_ch_service_owner_marker(current_owner_marker, service_name, port)?;
    }
    if let (Some(owner_marker), Some(current_owner_marker)) =
        (owner_marker.as_ref(), current_owner_marker.as_ref())
    {
        anyhow::ensure!(
            current_owner_marker == owner_marker,
            "the native service runtime owner changed while ch was attaching"
        );
    }
    let current_receipt = read_service_receipt(&helper_home)
        .context("re-read the native service receipt before attaching ch")?;
    anyhow::ensure!(
        current_receipt == receipt,
        "the native service receipt changed while ch was attaching"
    );
    let current_runtime = wait_for_ch_service_runtime(current_receipt, proxy_addr).await?;
    validate_ch_service_runtime_identity(&current_runtime, &receipt, service_name, proxy_addr)?;
    anyhow::ensure!(
        current_runtime.identity == runtime.identity,
        "the native service runtime identity changed while ch was attaching"
    );
    ensure_owned_runtime_switch_readiness(current_runtime.credential_readiness)
        .context("verify native service credentials before switching Codex")?;
    let configured = load_config()
        .await
        .context("load the Codex client patch for the verified native service")?
        .codex
        .client_patch
        .unwrap_or_default();
    apply_codex_switch(
        ValidatedCodexBaseUrl::local(port),
        CodexSwitchClientPatchSelection::default(),
        configured,
    )
    .context("automatically switch Codex to the verified native service")?;

    println!(
        "ch attached to the verified native Codex service at http://{proxy_addr}; the Codex switch remains applied while the service is installed."
    );
    if interactive {
        tui::run_local_attached_dashboard_with_admin_base_url(
            service_name,
            port,
            receipt.admin_base_url().to_string(),
            configured_local_admin_token_env().map(str::to_string),
        )
        .await
        .context("run the local dashboard attached to the native service")?;
    }
    Ok(true)
}

struct ForegroundCodexSwitchGuard {
    restore_lease: Option<codex_switch::CodexSwitchRestoreLease>,
}

impl ForegroundCodexSwitchGuard {
    fn finalize(&mut self) -> Result<(), codex_switch::CodexSwitchError> {
        let Some(restore_lease) = self.restore_lease.take() else {
            return Ok(());
        };
        log_foreground_codex_restore(codex_switch::restore_if_owned_with_retry(&restore_lease))
    }
}

impl Drop for ForegroundCodexSwitchGuard {
    fn drop(&mut self) {
        let Some(restore_lease) = self.restore_lease.take() else {
            return;
        };
        if let Err(error) =
            log_foreground_codex_restore(codex_switch::restore_if_owned(&restore_lease))
        {
            tracing::warn!(
                "ch could not restore the Codex client switch from its drop fallback: {error}"
            );
        }
    }
}

fn log_foreground_codex_restore(
    result: Result<Option<codex_switch::CodexSwitchOutcome>, codex_switch::CodexSwitchError>,
) -> Result<(), codex_switch::CodexSwitchError> {
    match result {
        Ok(Some(outcome)) => {
            tracing::info!(
                change = outcome.change.as_str(),
                phase = outcome.status.phase.as_str(),
                "ch restored the Codex client switch after the foreground proxy stopped"
            );
            Ok(())
        }
        Ok(None) => {
            tracing::info!(
                "ch left the Codex client switch unchanged because its generation is no longer owned by this foreground proxy"
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn prepare_ch_codex_switch(
    service_name: &str,
    options: ServeRuntimeOptions,
    runtime_owner: Option<&RuntimeOwnerLease>,
    client_patch: CodexClientPatchConfig,
) -> anyhow::Result<Option<ForegroundCodexSwitchGuard>> {
    if !options.should_auto_manage_codex_switch(service_name) {
        return Ok(None);
    }

    let runtime_owner =
        runtime_owner.context("the foreground ch proxy is missing its runtime ownership lease")?;
    let outcome = if options.is_resident() {
        codex_switch::apply_with_client_patch(
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(runtime_owner.proxy_port()),
            },
            client_patch,
        )
    } else {
        codex_switch::acquire_ephemeral_local_codex(runtime_owner, client_patch)
    }
    .context("automatically switch Codex to the local ch proxy")?;
    tracing::info!(
        change = outcome.change.as_str(),
        phase = outcome.status.phase.as_str(),
        base_url = outcome.status.base_url.as_deref().unwrap_or("<unset>"),
        "ch switched Codex to the local proxy"
    );

    if options.is_resident() {
        tracing::info!("ch resident mode leaves the Codex client switch applied");
        Ok(None)
    } else {
        Ok(outcome
            .restore_lease
            .map(|restore_lease| ForegroundCodexSwitchGuard {
                restore_lease: Some(restore_lease),
            }))
    }
}

async fn resolve_serve_tui_language(loaded: &LoadedConfig) -> tui::Language {
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
    let detected = tui::detect_system_language();
    if let Err(error) = mutate_helper_config(|config| {
        if config.ui.language.is_none() {
            config.ui.language = Some("auto".to_string());
        }
        Ok(())
    })
    .await
    {
        tracing::warn!("Failed to persist ui.language to config: {error}");
    }
    detected
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
    let interactive = !options.is_resident()
        && options.enable_tui
        && atty::is(atty::Stream::Stdin)
        && atty::is(atty::Stream::Stdout);
    if try_attach_ch_to_running_service(service_name, host, port, options, interactive).await? {
        return Ok(());
    }
    if options.should_auto_manage_codex_switch(service_name) {
        ensure_ch_codex_route(port).await?;
    }
    let loaded = load_serve_config().await?;
    let tui_lang = resolve_serve_tui_language(&loaded).await;
    let client_patch = loaded.source.codex.client_patch.unwrap_or_default();

    let runtime =
        build_local_proxy_runtime(service_name, host, port, options.service_managed, loaded)
            .await?;
    let owner_lease = options
        .owner_marker(service_name, port)
        .map(|marker| RuntimeOwnerLease::acquire(&marker).context("acquire runtime ownership"))
        .transpose()?;
    if options.should_auto_manage_codex_switch(service_name) {
        anyhow::ensure!(
            owner_lease.is_some(),
            "the foreground ch proxy is missing its runtime ownership lease"
        );
        preflight_owned_local_runtime_switch(&runtime.proxy).await?;
    }
    let mut codex_switch_guard =
        prepare_ch_codex_switch(service_name, options, owner_lease.as_ref(), client_patch)?;
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
    if options.supervisor_managed {
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            wait_for_supervisor_parent_eof().await;
            let _ = shutdown_tx.send(true);
        });
    }

    let mut running_runtime = runtime.start();
    if options.supervisor_managed {
        publish_supervisor_child_ready(service_name, port, admin_addr.port())?;
    }

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
                server_res
            }
            tui_res = &mut tui_handle => {
                match tui_res {
                    Ok(Ok(())) => {
                        // The dashboard requested a shutdown (or exited because shutdown was already triggered).
                        let _ = shutdown_tx.send(true);
                        await_server_shutdown_with_timeout(&mut running_runtime).await
                    }
                    Ok(Err(err)) => {
                        // If the dashboard fails (e.g. terminal issues), keep running without it.
                        tracing::warn!("TUI dashboard failed; continuing without TUI: {}", err);
                        await_server_shutdown(&mut running_runtime).await
                    }
                    Err(join_err) => {
                        tracing::warn!("TUI task join error; continuing without TUI: {}", join_err);
                        await_server_shutdown(&mut running_runtime).await
                    }
                }
            }
        }
    } else {
        await_server_shutdown(&mut running_runtime).await
    };

    let restore_result = match codex_switch_guard.as_mut() {
        Some(guard) => guard
            .finalize()
            .context("restore Codex client configuration after foreground proxy shutdown"),
        None => Ok(()),
    };
    match (result, restore_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(runtime_error), Ok(())) => Err(runtime_error),
        (Ok(()), Err(restore_error)) => Err(restore_error),
        (Err(runtime_error), Err(restore_error)) => Err(anyhow::anyhow!(
            "proxy runtime failed: {runtime_error:#}; Codex client restore also failed: {restore_error:#}"
        )),
    }
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

#[derive(Debug, Clone, Copy, Default)]
struct CodexSwitchClientPatchSelection {
    overrides: CodexClientPatchOverrides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexSwitchCredentialReadiness {
    Ready,
    Degraded,
    Blocked,
    Unverified,
}

fn classify_switch_credential_readiness(
    expected_service: &str,
    actual_service: &str,
    readiness: Option<CredentialAggregateReadiness>,
) -> anyhow::Result<CodexSwitchCredentialReadiness> {
    anyhow::ensure!(
        actual_service == expected_service,
        "refusing to switch because the verified runtime service identity is '{actual_service}', not '{expected_service}'"
    );
    Ok(match readiness {
        Some(CredentialAggregateReadiness::Ready) => CodexSwitchCredentialReadiness::Ready,
        Some(CredentialAggregateReadiness::Degraded) => CodexSwitchCredentialReadiness::Degraded,
        Some(CredentialAggregateReadiness::Blocked) => CodexSwitchCredentialReadiness::Blocked,
        None => CodexSwitchCredentialReadiness::Unverified,
    })
}

fn ensure_switch_credential_readiness(
    readiness: CodexSwitchCredentialReadiness,
    require_observed: bool,
) -> anyhow::Result<()> {
    match readiness {
        CodexSwitchCredentialReadiness::Ready => Ok(()),
        CodexSwitchCredentialReadiness::Degraded => {
            eprintln!(
                "Warning: Codex switch target credential readiness is degraded; at least one route remains usable."
            );
            tracing::warn!(
                "switching Codex to a relay with degraded credential readiness because at least one route remains usable"
            );
            Ok(())
        }
        CodexSwitchCredentialReadiness::Unverified if !require_observed => {
            eprintln!(
                "Warning: Codex switch target credential readiness is unverified because its operator contract does not publish this state."
            );
            tracing::warn!(
                "the relay does not expose credential readiness; preserving compatibility and continuing the switch"
            );
            Ok(())
        }
        CodexSwitchCredentialReadiness::Unverified => anyhow::bail!(
            "refusing to switch Codex because the owned runtime did not publish credential readiness"
        ),
        CodexSwitchCredentialReadiness::Blocked => anyhow::bail!(
            "refusing to switch Codex because the relay has no route with a usable credential"
        ),
    }
}

fn ensure_owned_runtime_switch_readiness(
    readiness: CredentialAggregateReadiness,
) -> anyhow::Result<()> {
    let readiness = classify_switch_credential_readiness("codex", "codex", Some(readiness))?;
    ensure_switch_credential_readiness(readiness, true)
}

fn classify_operator_switch_readiness(
    model: &OperatorReadModel,
) -> anyhow::Result<CodexSwitchCredentialReadiness> {
    let readiness = model
        .data
        .as_ref()
        .and_then(|data| data.summary.credential_readiness);
    classify_switch_credential_readiness("codex", model.service_name.as_str(), readiness)
}

async fn preflight_owned_local_runtime_switch(
    proxy: &crate::proxy::ProxyService,
) -> anyhow::Result<()> {
    let model = proxy
        .operator_read_model()
        .await
        .context("capture local runtime credential readiness before switching Codex")?;
    let readiness = classify_operator_switch_readiness(&model)?;
    ensure_switch_credential_readiness(readiness, true)
}

async fn preflight_local_admin_switch_target(port: u16) -> CliResult<()> {
    let client = local_control_plane_client(port).map_err(|_| {
        CliError::Other(
            "failed to initialize the owned local runtime credential readiness probe".to_string(),
        )
    })?;
    let model = client
        .operator_read_model()
        .await
        .map_err(|error| configured_relay_readiness_probe_error(&error))?;
    let readiness = classify_operator_switch_readiness(&model)
        .map_err(|error| CliError::Other(error.to_string()))?;
    ensure_switch_credential_readiness(readiness, true)
        .map_err(|error| CliError::Other(error.to_string()))
}

fn relay_target_has_authenticated_admin(target: &ResolvedRelayTarget) -> bool {
    target.admin_url.is_some() && target.admin_token_env.is_some()
}

fn configured_codex_relay_target_for_switch(
    config: &HelperConfig,
    target: &ValidatedCodexBaseUrl,
) -> anyhow::Result<Option<ResolvedRelayTarget>> {
    let mut matches = config.relay_targets.iter().filter_map(|(name, candidate)| {
        if name == "local" || candidate.service.unwrap_or(ServiceKind::Codex) != ServiceKind::Codex
        {
            return None;
        }
        normalize_base_url(&candidate.proxy_url)
            .filter(|proxy_url| proxy_url == target.as_str())
            .map(|_| name.as_str())
    });
    let first = matches.next();
    anyhow::ensure!(
        matches.next().is_none(),
        "multiple configured Codex relay targets match the requested switch URL"
    );
    first
        .map(|name| resolve_relay_target(config, name))
        .transpose()
}

fn configured_relay_readiness_probe_error(error: &ControlPlaneError) -> CliError {
    let detail = match error {
        ControlPlaneError::HttpStatus {
            status: 401 | 403, ..
        } => "admin authentication was rejected".to_string(),
        ControlPlaneError::HttpStatus { status, .. } => {
            format!("admin endpoint returned HTTP {status}")
        }
        ControlPlaneError::Transport { .. } => "admin endpoint is unreachable".to_string(),
        ControlPlaneError::Decode { .. } => "admin endpoint returned invalid JSON".to_string(),
        ControlPlaneError::InvalidPayload { .. } => {
            "admin endpoint returned an invalid operator contract".to_string()
        }
        ControlPlaneError::UntrustedRequestPath { .. } => {
            "admin endpoint request path was rejected".to_string()
        }
    };
    CliError::Other(format!(
        "failed to verify relay credential readiness: {detail}"
    ))
}

async fn preflight_configured_relay_switch_target(target: &ResolvedRelayTarget) -> CliResult<()> {
    if target.service != ServiceKind::Codex {
        return Err(CliError::Other(
            "refusing to switch Codex to a non-Codex relay target".to_string(),
        ));
    }
    if !relay_target_has_authenticated_admin(target) {
        eprintln!(
            "Warning: Codex switch target credential readiness is unverified because the configured relay target has no authenticated admin authority."
        );
        return Ok(());
    }
    let endpoint = ControlPlaneEndpoint::new(
        target
            .admin_url
            .as_deref()
            .expect("authenticated relay target has an admin URL"),
        target.admin_token_env.as_deref(),
    )
    .map_err(|_| {
        CliError::Other(
            "failed to construct the configured relay credential readiness probe".to_string(),
        )
    })?;
    let client = ControlPlaneClient::new(endpoint).map_err(|_| {
        CliError::Other(
            "failed to initialize the configured relay credential readiness probe".to_string(),
        )
    })?;
    let model = client
        .operator_read_model()
        .await
        .map_err(|error| configured_relay_readiness_probe_error(&error))?;
    let readiness = classify_operator_switch_readiness(&model)
        .map_err(|error| CliError::Other(error.to_string()))?;
    ensure_switch_credential_readiness(readiness, false)
        .map_err(|error| CliError::Other(error.to_string()))
}

fn apply_codex_switch(
    validated_base_url: ValidatedCodexBaseUrl,
    selection: CodexSwitchClientPatchSelection,
    configured: CodexClientPatchConfig,
) -> CliResult<()> {
    let client_patch = resolve_codex_switch_client_patch(configured, selection);
    let outcome = codex_switch::apply_with_client_patch(
        CodexSwitchIntent::On { validated_base_url },
        client_patch,
    )
    .map_err(|error| CliError::CodexConfig(error.to_string()))?;
    println!(
        "Codex switch: {} ({})",
        outcome.change.as_str(),
        outcome.status.phase.as_str()
    );
    if let Some(base_url) = outcome.status.base_url.as_deref() {
        println!("  base_url: {base_url}");
    }
    if let Some(client_patch) = outcome.status.client_patch.as_ref() {
        print_codex_client_patch(client_patch);
    }
    println!("  config: {:?}", outcome.status.config_path);
    println!("  state:  {:?}", outcome.status.state_path);
    Ok(())
}

async fn do_switch_on(
    port: Option<u16>,
    base_url: Option<String>,
    selection: CodexSwitchClientPatchSelection,
) -> CliResult<()> {
    let validated_base_url = match base_url {
        Some(base_url) => ValidatedCodexBaseUrl::parse(base_url),
        None => Ok(ValidatedCodexBaseUrl::local(port.unwrap_or_else(|| {
            default_proxy_port_for_service_kind(ServiceKind::Codex)
        }))),
    }
    .map_err(|error| CliError::CodexConfig(error.to_string()))?;
    let config = load_config()
        .await
        .map_err(|error| CliError::Configuration(error.to_string()))?;
    if let Some(target) = configured_codex_relay_target_for_switch(&config, &validated_base_url)
        .map_err(|error| CliError::Configuration(error.to_string()))?
    {
        preflight_configured_relay_switch_target(&target).await?;
    } else {
        eprintln!(
            "Warning: Codex switch target credential readiness is unverified because the URL does not match a configured Codex relay target with authenticated admin access."
        );
    }
    let configured = config.codex.client_patch.unwrap_or_default();
    apply_codex_switch(validated_base_url, selection, configured)
}

fn resolve_codex_switch_client_patch(
    configured: CodexClientPatchConfig,
    selection: CodexSwitchClientPatchSelection,
) -> CodexClientPatchConfig {
    configured.with_overrides(selection.overrides)
}

fn resolve_relay_codex_client_patch(
    global: CodexClientPatchConfig,
    target: Option<CodexClientPatchOverrides>,
    selection: CodexSwitchClientPatchSelection,
) -> CodexClientPatchConfig {
    resolve_codex_switch_client_patch(
        global.with_field_overrides(target.unwrap_or_default()),
        selection,
    )
}

fn print_codex_client_patch(client_patch: &CodexClientPatchConfig) {
    println!("  preset: {}", client_patch.preset);
    println!("  compaction: {}", client_patch.compaction);
    println!(
        "  responses_websocket: {}",
        client_patch.responses_websocket
    );
    println!("  translate_models: {}", client_patch.translate_models);
    println!(
        "  hosted_image_generation: {}",
        client_patch.hosted_image_generation
    );
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

    if codex || claude {
        let service = if claude {
            ServiceKind::Claude
        } else {
            ServiceKind::Codex
        };
        mutate_helper_config(move |source| {
            source.default_service = Some(service);
            Ok(())
        })
        .await
        .map_err(|e| CliError::Configuration(e.to_string()))?;

        let name = if claude { "Claude" } else { "Codex" };
        println!("Default target service has been set to {}.", name);
    } else {
        let configured = load_config()
            .await
            .map_err(|e| CliError::Configuration(e.to_string()))?;
        let name = match configured.default_service {
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
    if let Some(client_patch) = status.client_patch.as_ref() {
        print_codex_client_patch(client_patch);
    }
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
mod switch_client_patch_tests {
    use super::{
        CodexSwitchClientPatchSelection, CodexSwitchCredentialReadiness,
        classify_switch_credential_readiness, configured_codex_relay_target_for_switch,
        configured_relay_readiness_probe_error, ensure_switch_credential_readiness,
        preflight_configured_relay_switch_target, relay_target_has_authenticated_admin,
        relay_target_list_line, resolve_codex_switch_client_patch,
        resolve_relay_codex_client_patch,
    };
    use axum::Router;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::get;
    use codex_helper_core::codex_switch::ValidatedCodexBaseUrl;
    use codex_helper_core::config::{
        CodexClientPatchConfig, CodexClientPatchOverrides, CodexClientPreset,
        CodexCompactionStrategy, CodexHostedImageGenerationMode, HelperConfig, RelayTargetConfig,
        ServiceKind,
    };
    use codex_helper_core::control_plane_client::ControlPlaneError;
    use codex_helper_core::credentials::CredentialAggregateReadiness;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ScopedUniqueEnv {
        key: String,
        previous: Option<OsString>,
    }

    impl ScopedUniqueEnv {
        fn set(key: String, value: &str) -> Self {
            let previous = std::env::var_os(&key);
            // SAFETY: each test uses a UUID-scoped key that no other test reads or mutates.
            unsafe { std::env::set_var(&key, value) };
            Self { key, previous }
        }
    }

    impl Drop for ScopedUniqueEnv {
        fn drop(&mut self) {
            // SAFETY: the UUID-scoped key is exclusively owned by this guard.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(&self.key, value),
                    None => std::env::remove_var(&self.key),
                }
            }
        }
    }

    fn configured_patch() -> CodexClientPatchConfig {
        CodexClientPatchConfig {
            preset: CodexClientPreset::OfficialImagegen,
            responses_websocket: true,
            compaction: CodexCompactionStrategy::RemoteV2,
            translate_models: true,
            hosted_image_generation: CodexHostedImageGenerationMode::Disabled,
        }
    }

    #[test]
    fn switch_without_overrides_uses_the_complete_configured_patch() {
        let configured = configured_patch();
        let resolved = resolve_codex_switch_client_patch(
            configured,
            CodexSwitchClientPatchSelection::default(),
        );

        assert_eq!(resolved, configured);
    }

    #[test]
    fn explicit_preset_resets_dependent_defaults_but_preserves_other_config_fields() {
        let resolved = resolve_codex_switch_client_patch(
            configured_patch(),
            CodexSwitchClientPatchSelection {
                overrides: CodexClientPatchOverrides {
                    preset: Some(CodexClientPreset::Default),
                    ..CodexClientPatchOverrides::default()
                },
            },
        );

        assert_eq!(resolved.preset, CodexClientPreset::Default);
        assert!(!resolved.responses_websocket);
        assert_eq!(resolved.compaction, CodexCompactionStrategy::Auto);
        assert!(resolved.translate_models);
        assert_eq!(
            resolved.hosted_image_generation,
            CodexHostedImageGenerationMode::Disabled
        );
    }

    #[test]
    fn explicit_transport_and_compaction_override_the_preset_defaults() {
        let resolved = resolve_codex_switch_client_patch(
            configured_patch(),
            CodexSwitchClientPatchSelection {
                overrides: CodexClientPatchOverrides {
                    preset: Some(CodexClientPreset::OfficialImagegen),
                    responses_websocket: Some(false),
                    compaction: Some(CodexCompactionStrategy::RemoteV1),
                    translate_models: Some(false),
                    hosted_image_generation: Some(CodexHostedImageGenerationMode::Enabled),
                },
            },
        );

        assert_eq!(resolved.preset, CodexClientPreset::OfficialImagegen);
        assert!(!resolved.responses_websocket);
        assert_eq!(resolved.compaction, CodexCompactionStrategy::RemoteV1);
        assert!(!resolved.translate_models);
        assert_eq!(
            resolved.hosted_image_generation,
            CodexHostedImageGenerationMode::Enabled
        );
    }

    #[test]
    fn relay_target_fields_override_global_before_cli_fields() {
        let resolved = resolve_relay_codex_client_patch(
            configured_patch(),
            Some(CodexClientPatchOverrides {
                preset: Some(CodexClientPreset::OfficialRelay),
                compaction: Some(CodexCompactionStrategy::RemoteV1),
                ..CodexClientPatchOverrides::default()
            }),
            CodexSwitchClientPatchSelection {
                overrides: CodexClientPatchOverrides {
                    translate_models: Some(false),
                    ..CodexClientPatchOverrides::default()
                },
            },
        );

        assert_eq!(resolved.preset, CodexClientPreset::OfficialRelay);
        assert!(resolved.responses_websocket);
        assert_eq!(resolved.compaction, CodexCompactionStrategy::RemoteV1);
        assert!(!resolved.translate_models);
        assert_eq!(
            resolved.hosted_image_generation,
            CodexHostedImageGenerationMode::Disabled
        );
    }

    #[test]
    fn switch_readiness_distinguishes_routable_blocked_and_unverified_credentials() {
        assert_eq!(
            classify_switch_credential_readiness(
                "codex",
                "codex",
                Some(CredentialAggregateReadiness::Ready),
            )
            .expect("ready observation"),
            CodexSwitchCredentialReadiness::Ready
        );
        assert_eq!(
            classify_switch_credential_readiness(
                "codex",
                "codex",
                Some(CredentialAggregateReadiness::Degraded),
            )
            .expect("degraded observation"),
            CodexSwitchCredentialReadiness::Degraded
        );
        assert_eq!(
            classify_switch_credential_readiness(
                "codex",
                "codex",
                Some(CredentialAggregateReadiness::Blocked),
            )
            .expect("blocked observation"),
            CodexSwitchCredentialReadiness::Blocked
        );
        assert_eq!(
            classify_switch_credential_readiness("codex", "codex", None)
                .expect("legacy observation without credential projection"),
            CodexSwitchCredentialReadiness::Unverified
        );
        ensure_switch_credential_readiness(CodexSwitchCredentialReadiness::Unverified, false)
            .expect("an observable older remote may continue with an explicit warning");
        let missing_owned_projection =
            ensure_switch_credential_readiness(CodexSwitchCredentialReadiness::Unverified, true)
                .expect_err("an owned current-process runtime must publish readiness");
        assert!(
            missing_owned_projection
                .to_string()
                .contains("owned runtime")
        );

        let mismatch = classify_switch_credential_readiness(
            "codex",
            "claude",
            Some(CredentialAggregateReadiness::Ready),
        )
        .expect_err("a Claude runtime cannot authorize a Codex switch");
        assert!(mismatch.to_string().contains("service identity"));
    }

    #[test]
    fn relay_list_marks_legacy_bookmarks_without_hiding_valid_targets() {
        let config = HelperConfig {
            relay_targets: BTreeMap::from([
                (
                    "legacy".to_string(),
                    RelayTargetConfig {
                        service: Some(ServiceKind::Codex),
                        proxy_url: "https://legacy.example.test/v1/".to_string(),
                        ..RelayTargetConfig::default()
                    },
                ),
                (
                    "ready".to_string(),
                    RelayTargetConfig {
                        service: Some(ServiceKind::Codex),
                        proxy_url: "https://ready.example.test/v1/".to_string(),
                        admin_url: Some("https://ready-admin.example.test/".to_string()),
                        admin_token_env: Some("READY_ADMIN_TOKEN".to_string()),
                        ..RelayTargetConfig::default()
                    },
                ),
            ]),
            ..HelperConfig::default()
        };

        let legacy = relay_target_list_line(&config, "legacy");
        let ready = relay_target_list_line(&config, "ready");

        assert!(legacy.contains("legacy"));
        assert!(legacy.contains("migration-required"));
        assert!(legacy.contains("admin=<unavailable>"));
        assert!(ready.contains("ready"));
        assert!(ready.contains("(configured)"));
        assert!(!ready.contains("migration-required"));
    }

    #[test]
    fn explicit_switch_discovers_only_exact_configured_codex_relay_targets() {
        let config = HelperConfig {
            relay_targets: BTreeMap::from([
                (
                    "nas".to_string(),
                    RelayTargetConfig {
                        service: Some(ServiceKind::Codex),
                        proxy_url: "https://relay.example.test/v1/".to_string(),
                        admin_url: Some("https://admin.example.test/".to_string()),
                        admin_token_env: Some("RELAY_ADMIN_TOKEN".to_string()),
                        ..RelayTargetConfig::default()
                    },
                ),
                (
                    "claude".to_string(),
                    RelayTargetConfig {
                        service: Some(ServiceKind::Claude),
                        proxy_url: "https://claude.example.test/v1".to_string(),
                        admin_url: Some("https://claude-admin.example.test".to_string()),
                        admin_token_env: Some("CLAUDE_ADMIN_TOKEN".to_string()),
                        ..RelayTargetConfig::default()
                    },
                ),
                (
                    "legacy".to_string(),
                    RelayTargetConfig {
                        service: Some(ServiceKind::Codex),
                        proxy_url: "https://legacy.example.test/v1".to_string(),
                        ..RelayTargetConfig::default()
                    },
                ),
            ]),
            ..HelperConfig::default()
        };

        let exact = configured_codex_relay_target_for_switch(
            &config,
            &ValidatedCodexBaseUrl::parse("https://relay.example.test/v1")
                .expect("valid switch URL"),
        )
        .expect("resolve exact target")
        .expect("configured target");
        assert_eq!(exact.name, "nas");
        assert!(relay_target_has_authenticated_admin(&exact));

        let arbitrary = configured_codex_relay_target_for_switch(
            &config,
            &ValidatedCodexBaseUrl::parse("https://unconfigured.example.test/v1")
                .expect("valid arbitrary URL"),
        )
        .expect("classify arbitrary target");
        assert!(arbitrary.is_none());

        let claude = configured_codex_relay_target_for_switch(
            &config,
            &ValidatedCodexBaseUrl::parse("https://claude.example.test/v1")
                .expect("valid Claude URL"),
        )
        .expect("classify Claude target");
        assert!(claude.is_none());

        let legacy_error = configured_codex_relay_target_for_switch(
            &config,
            &ValidatedCodexBaseUrl::parse("https://legacy.example.test/v1")
                .expect("valid legacy URL"),
        )
        .expect_err("the selected legacy target must still validate admin authority");
        assert!(legacy_error.to_string().contains("legacy"));
        assert!(legacy_error.to_string().contains("explicit admin_url"));

        let unauthenticated_loopback = super::ResolvedRelayTarget {
            name: "ssh-tunnel".to_string(),
            service: ServiceKind::Codex,
            proxy_url: "http://127.0.0.1:3211".to_string(),
            admin_url: Some("http://127.0.0.1:4211".to_string()),
            admin_token_env: None,
            client_patch: None,
            built_in_local: false,
        };
        assert!(!relay_target_has_authenticated_admin(
            &unauthenticated_loopback
        ));
    }

    #[test]
    fn configured_relay_probe_errors_never_render_remote_bodies_or_credential_references() {
        let error = ControlPlaneError::HttpStatus {
            status: 500,
            body_excerpt: "secret-canary native://credential/ref-canary".to_string(),
        };

        let rendered = configured_relay_readiness_probe_error(&error).to_string();

        assert!(rendered.contains("credential readiness"));
        assert!(!rendered.contains("secret-canary"));
        assert!(!rendered.contains("ref-canary"));
        assert!(!rendered.contains("native://"));
    }

    #[tokio::test]
    async fn configured_relay_switch_preflight_queries_authenticated_admin_without_leaking_body() {
        let token = "admin-secret-canary";
        let env_name = format!(
            "CODEX_HELPER_TEST_RELAY_ADMIN_{}",
            uuid::Uuid::new_v4()
                .simple()
                .to_string()
                .to_ascii_uppercase()
        );
        let _env = ScopedUniqueEnv::set(env_name.clone(), token);
        let hits = Arc::new(AtomicUsize::new(0));
        let route_hits = Arc::clone(&hits);
        let app = Router::new().route(
            "/__codex_helper/api/v1/operator/read-model",
            get(move |headers: HeaderMap| {
                let route_hits = Arc::clone(&route_hits);
                async move {
                    route_hits.fetch_add(1, Ordering::SeqCst);
                    if headers
                        .get(codex_helper_core::proxy::ADMIN_TOKEN_HEADER)
                        .and_then(|value| value.to_str().ok())
                        != Some(token)
                    {
                        return (
                            StatusCode::UNAUTHORIZED,
                            "missing admin credential".to_string(),
                        );
                    }
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "response-secret-canary native://credential/ref-canary".to_string(),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake relay admin");
        let addr = listener.local_addr().expect("fake relay admin address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fake admin");
        });
        let target = super::ResolvedRelayTarget {
            name: "test-relay".to_string(),
            service: ServiceKind::Codex,
            proxy_url: "https://relay.example.test/v1".to_string(),
            admin_url: Some(format!("http://{addr}")),
            admin_token_env: Some(env_name),
            client_patch: None,
            built_in_local: false,
        };

        let error = preflight_configured_relay_switch_target(&target)
            .await
            .expect_err("failed operator probe must stop the switch");
        let rendered = error.to_string();

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(rendered.contains("credential readiness"));
        assert!(!rendered.contains(token));
        assert!(!rendered.contains("response-secret-canary"));
        assert!(!rendered.contains("ref-canary"));
        server.abort();
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

    #[test]
    fn supervised_readiness_requires_the_exact_spawn_identity() {
        let path = Path::new("unused-ready-record.json");
        let expected = SupervisorReadyExpectation {
            path,
            owner_id: "owner-a",
            child_generation: "child-a",
            child_pid: 42,
            service_name: "codex",
            proxy_port: 3211,
        };
        let ready = || SupervisorChildReady {
            version: SUPERVISOR_READY_VERSION,
            supervisor_owner_id: "owner-a".to_string(),
            child_generation: "child-a".to_string(),
            child_pid: 42,
            service_name: "codex".to_string(),
            proxy_port: 3211,
            admin_port: admin_loopback_addr_for_proxy_port(3211).port(),
        };

        assert!(supervisor_ready_matches(&ready(), &expected));
        let mut mismatches = Vec::new();
        let mut value = ready();
        value.supervisor_owner_id = "owner-b".to_string();
        mismatches.push(value);
        let mut value = ready();
        value.child_generation = "child-b".to_string();
        mismatches.push(value);
        let mut value = ready();
        value.child_pid = 43;
        mismatches.push(value);
        let mut value = ready();
        value.service_name = "claude".to_string();
        mismatches.push(value);
        let mut value = ready();
        value.proxy_port = 3210;
        mismatches.push(value);
        let mut value = ready();
        value.admin_port = value.admin_port.saturating_add(1);
        mismatches.push(value);

        for mismatch in mismatches {
            assert!(!supervisor_ready_matches(&mismatch, &expected));
        }
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

    #[derive(Clone, Copy)]
    enum ChAttachOwnerMarkerFixture {
        Matching,
        Missing,
        Corrupt,
    }

    #[derive(Clone, Copy)]
    enum ChAttachRuntimeIdentityFixture {
        Matching,
        ReceiptGenerationMismatch,
    }

    #[derive(Clone, Copy)]
    enum ChAttachStartupFixture {
        AlreadyRunning,
        DelayedStarting,
    }

    enum ChAttachExpectation {
        Attached,
        Refused(&'static str),
    }

    fn exercise_ch_native_service_attach(
        owner_marker_fixture: ChAttachOwnerMarkerFixture,
        runtime_identity_fixture: ChAttachRuntimeIdentityFixture,
        startup_fixture: ChAttachStartupFixture,
        expectation: ChAttachExpectation,
    ) {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-ch-service-attach-test-{}",
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
        write_file(
            &codex_home.join("config.toml"),
            "model_provider = \"openai\"\n",
        );

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let (attach_result, switch, service_still_running) = runtime.block_on(async {
            let loaded = loaded_runtime_test_config();
            save_helper_config(&loaded.source)
                .await
                .expect("persist helper config for ch switch");
            codex_helper_core::local_operator::ensure_local_operator_token()
                .expect("create local operator token");

            let (port, admin_reservation) = reserve_admin_for_free_proxy_port();
            drop(admin_reservation);
            let admin_addr = admin_loopback_addr_for_proxy_port(port);
            let runtime_generation =
                codex_helper_core::service_target::ServiceInstallGeneration::generate();
            let receipt_generation = match runtime_identity_fixture {
                ChAttachRuntimeIdentityFixture::Matching => runtime_generation.clone(),
                ChAttachRuntimeIdentityFixture::ReceiptGenerationMismatch => {
                    codex_helper_core::service_target::ServiceInstallGeneration::generate()
                }
            };
            let identity = codex_helper_core::service_target::ServiceRuntimeIdentity {
                service: ServiceKind::Codex,
                helper_home: helper_home.clone(),
                client_home: codex_home.clone(),
                install_generation: runtime_generation.clone(),
            };
            let service_runtime = build_proxy_runtime_from_loaded_with_options(
                "codex",
                IpAddr::from([127, 0, 0, 1]),
                port,
                ProxyRuntimeOptions::for_proxy_port(port)
                    .with_admin_addr(admin_addr)
                    .with_credential_sources(CredentialSourceCapabilities::platform_native())
                    .with_service_runtime_identity(Some(identity)),
                loaded,
            )
            .await
            .expect("build native service runtime");

            let receipt = ServiceReceipt::new(
                ServiceKind::Codex,
                helper_home.clone(),
                codex_home.clone(),
                daemon_admin_base_url_for_proxy_port(port),
                ServicePlatformBackend::current().expect("supported service platform"),
                receipt_generation,
            )
            .expect("build service receipt");
            let runtime_receipt = ServiceReceipt::new(
                ServiceKind::Codex,
                helper_home.clone(),
                codex_home.clone(),
                daemon_admin_base_url_for_proxy_port(port),
                ServicePlatformBackend::current().expect("supported service platform"),
                runtime_generation,
            )
            .expect("build runtime identity receipt");
            let mut receipt_transaction =
                crate::service_receipt::ServiceReceiptTransaction::begin(helper_home.clone())
                    .expect("begin service receipt transaction");
            receipt_transaction
                .replace(&receipt)
                .expect("write service receipt");

            let owner_lease = match owner_marker_fixture {
                ChAttachOwnerMarkerFixture::Matching => {
                    let marker =
                        RuntimeOwnerMarker::new(RuntimeOwnerKind::SystemService, "codex", port);
                    Some(
                        RuntimeOwnerLease::acquire(&marker)
                            .expect("acquire native service runtime ownership"),
                    )
                }
                ChAttachOwnerMarkerFixture::Missing => None,
                ChAttachOwnerMarkerFixture::Corrupt => {
                    write_file(
                        &crate::runtime_manager::owner_marker_path("codex", port),
                        "{not-json",
                    );
                    None
                }
            };
            let shutdown_tx = service_runtime.shutdown_tx.clone();
            let (attach_result, switch, service_still_running) = match startup_fixture {
                ChAttachStartupFixture::AlreadyRunning => {
                    let mut running_service = service_runtime.start();
                    let attach_result = run_server(
                        "codex",
                        IpAddr::from([127, 0, 0, 1]),
                        port,
                        ServeRuntimeOptions {
                            enable_tui: false,
                            auto_manage_codex_switch: true,
                            ..ServeRuntimeOptions::default()
                        },
                    )
                    .await;
                    let switch =
                        codex_switch::inspect().expect("inspect service attach switch state");
                    let service_still_running = read_service_runtime_for_receipt(runtime_receipt)
                        .await
                        .is_ok();

                    if switch.enabled {
                        codex_switch::apply(CodexSwitchIntent::Off)
                            .expect("restore Codex after service attach test");
                    }
                    let _ = shutdown_tx.send(true);
                    tokio::time::timeout(Duration::from_secs(5), running_service.wait())
                        .await
                        .expect("native service shutdown timeout")
                        .expect("native service shutdown");
                    (attach_result, switch, service_still_running)
                }
                ChAttachStartupFixture::DelayedStarting => {
                    let attach = async {
                        let attach_result = try_attach_ch_to_running_service_with_platform_state(
                            "codex",
                            IpAddr::from([127, 0, 0, 1]),
                            port,
                            ServeRuntimeOptions {
                                enable_tui: false,
                                auto_manage_codex_switch: true,
                                ..ServeRuntimeOptions::default()
                            },
                            false,
                            || Ok(service_manager::ServiceRuntimeState::Starting),
                        )
                        .await
                        .and_then(|attached| {
                            anyhow::ensure!(
                                attached,
                                "starting native service was mistaken for a foreground candidate"
                            );
                            Ok(())
                        });
                        let switch =
                            codex_switch::inspect().expect("inspect delayed service attach switch");
                        let service_still_running =
                            read_service_runtime_for_receipt(runtime_receipt)
                                .await
                                .is_ok();
                        if switch.enabled {
                            codex_switch::apply(CodexSwitchIntent::Off)
                                .expect("restore Codex after delayed service attach test");
                        }
                        let _ = shutdown_tx.send(true);
                        (attach_result, switch, service_still_running)
                    };
                    let service = async {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        let mut running_service = service_runtime.start();
                        tokio::time::timeout(Duration::from_secs(5), running_service.wait())
                            .await
                            .expect("delayed native service shutdown timeout")
                            .expect("delayed native service shutdown");
                    };
                    let (result, ()) = tokio::join!(attach, service);
                    result
                }
            };
            drop(owner_lease);
            (attach_result, switch, service_still_running)
        });

        assert!(
            service_still_running,
            "the attach attempt must not replace or stop the native service"
        );
        match expectation {
            ChAttachExpectation::Attached => {
                attach_result.expect("attach ch to the already running native service");
                assert_eq!(switch.phase, CodexSwitchPhase::Applied);
                assert!(switch.enabled);
            }
            ChAttachExpectation::Refused(expected_error) => {
                let error = attach_result.expect_err("reject mismatched native service evidence");
                assert!(
                    format!("{error:#}").contains(expected_error),
                    "unexpected attach error: {error:#}"
                );
                assert_eq!(switch.phase, CodexSwitchPhase::Off);
                assert!(!switch.enabled);
            }
        }

        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn client_auto_management_is_owned_only_by_the_ch_entrypoint() {
        assert!(!CliEntrypoint::CodexHelper.auto_manages_codex_client());
        assert!(CliEntrypoint::Ch.auto_manages_codex_client());
    }

    #[test]
    fn ch_auto_switch_is_limited_to_user_owned_codex_servers() {
        let foreground = ServeRuntimeOptions {
            auto_manage_codex_switch: true,
            ..ServeRuntimeOptions::default()
        };
        assert!(foreground.should_auto_manage_codex_switch("codex"));
        assert!(!foreground.should_auto_manage_codex_switch("claude"));

        for managed in [
            ServeRuntimeOptions {
                supervisor_managed: true,
                ..foreground
            },
            ServeRuntimeOptions {
                desktop_managed: true,
                ..foreground
            },
            ServeRuntimeOptions {
                service_managed: true,
                ..foreground
            },
        ] {
            assert!(!managed.should_auto_manage_codex_switch("codex"));
        }
    }

    #[test]
    fn ch_waits_only_for_platform_states_that_may_own_a_starting_endpoint() {
        for state in [
            service_manager::ServiceRuntimeState::Running,
            service_manager::ServiceRuntimeState::Starting,
        ] {
            assert!(service_state_can_own_starting_endpoint(state));
        }
        for state in [
            service_manager::ServiceRuntimeState::Stopped,
            service_manager::ServiceRuntimeState::Stopping,
            service_manager::ServiceRuntimeState::Installed,
            service_manager::ServiceRuntimeState::NotInstalled,
            service_manager::ServiceRuntimeState::Unknown,
        ] {
            assert!(!service_state_can_own_starting_endpoint(state));
        }
    }

    #[test]
    fn ch_attaches_to_matching_native_service_before_binding_and_keeps_switch_applied() {
        exercise_ch_native_service_attach(
            ChAttachOwnerMarkerFixture::Matching,
            ChAttachRuntimeIdentityFixture::Matching,
            ChAttachStartupFixture::AlreadyRunning,
            ChAttachExpectation::Attached,
        );
    }

    #[test]
    fn ch_attaches_to_signed_native_service_without_owner_marker() {
        exercise_ch_native_service_attach(
            ChAttachOwnerMarkerFixture::Missing,
            ChAttachRuntimeIdentityFixture::Matching,
            ChAttachStartupFixture::AlreadyRunning,
            ChAttachExpectation::Attached,
        );
    }

    #[test]
    fn ch_attaches_to_signed_native_service_with_corrupt_owner_marker() {
        exercise_ch_native_service_attach(
            ChAttachOwnerMarkerFixture::Corrupt,
            ChAttachRuntimeIdentityFixture::Matching,
            ChAttachStartupFixture::AlreadyRunning,
            ChAttachExpectation::Attached,
        );
    }

    #[test]
    fn ch_rejects_receipt_and_signed_runtime_identity_generation_mismatch() {
        exercise_ch_native_service_attach(
            ChAttachOwnerMarkerFixture::Matching,
            ChAttachRuntimeIdentityFixture::ReceiptGenerationMismatch,
            ChAttachStartupFixture::AlreadyRunning,
            ChAttachExpectation::Refused("service generation"),
        );
    }

    #[test]
    fn ch_waits_for_a_receipted_native_service_during_its_starting_window() {
        exercise_ch_native_service_attach(
            ChAttachOwnerMarkerFixture::Missing,
            ChAttachRuntimeIdentityFixture::Matching,
            ChAttachStartupFixture::DelayedStarting,
            ChAttachExpectation::Attached,
        );
    }

    #[test]
    fn ch_does_not_attach_to_a_non_service_runtime_owner() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-ch-non-service-attach-test-{}",
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
        let occupied =
            std::net::TcpListener::bind("127.0.0.1:0").expect("occupy a non-service port");
        let addr = occupied.local_addr().expect("read occupied address");
        let owner_marker =
            RuntimeOwnerMarker::new(RuntimeOwnerKind::ManualCli, "codex", addr.port());
        let owner_lease =
            RuntimeOwnerLease::acquire(&owner_marker).expect("acquire manual runtime ownership");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let error = runtime
            .block_on(run_server(
                "codex",
                addr.ip(),
                addr.port(),
                ServeRuntimeOptions {
                    enable_tui: false,
                    auto_manage_codex_switch: true,
                    ..ServeRuntimeOptions::default()
                },
            ))
            .expect_err("an explicit non-service owner must reject service attachment");
        assert!(
            format!("{error:#}").contains("runtime owner marker does not match"),
            "unexpected attach error: {error:#}"
        );
        assert!(
            !codex_home.join("config.toml").exists(),
            "an explicit owner mismatch must be rejected before Codex is switched"
        );

        drop(owner_lease);
        drop(occupied);
        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ch_rejects_every_parseable_owner_identity_mismatch() {
        let expected = RuntimeOwnerMarker::new(RuntimeOwnerKind::SystemService, "codex", 3211);
        let mut mismatches = Vec::new();

        let mut marker = expected.clone();
        marker.owner = RuntimeOwnerKind::ManualCli;
        mismatches.push(marker);

        let mut marker = expected.clone();
        marker.service_name = "claude".to_string();
        mismatches.push(marker);

        let mut marker = expected.clone();
        marker.proxy_port = 3212;
        mismatches.push(marker);

        let mut marker = expected;
        marker.admin_port = marker.admin_port.saturating_add(1);
        mismatches.push(marker);

        for mismatch in mismatches {
            let error = validate_ch_service_owner_marker(&mismatch, "codex", 3211)
                .expect_err("reject parseable owner identity mismatch");
            assert!(error.to_string().contains("runtime owner marker"));
        }
    }

    #[test]
    fn ch_accepts_receipt_codex_home_before_selected_directory_exists() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-ch-missing-codex-home-test-{}",
            uuid::Uuid::new_v4()
        ));
        let helper_home = root.join("helper");
        let codex_home = root.join("not-created").join("codex");
        std::fs::create_dir_all(&helper_home).expect("create helper home");
        assert!(!codex_home.exists());

        let mut env = ScopedEnv::new();
        unsafe {
            env.set_path("CODEX_HELPER_HOME", &helper_home);
            env.set_path("CODEX_HOME", &codex_home);
        }
        let receipt = ServiceReceipt::new(
            ServiceKind::Codex,
            helper_home,
            codex_home.clone(),
            daemon_admin_base_url_for_proxy_port(3211),
            ServicePlatformBackend::current().expect("supported service platform"),
            codex_helper_core::service_target::ServiceInstallGeneration::generate(),
        )
        .expect("build service receipt for an uncreated Codex home");

        validate_ch_service_attach_receipt(&receipt, "codex", 3211)
            .expect("compare the uncreated Codex home by resolved path identity");
        assert!(!codex_home.exists());

        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ch_foreground_switch_restores_original_codex_config_on_drop() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-ch-switch-test-{}",
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
        let original = r#"# keep this comment
model_provider = "external"

[model_providers.external]
name = "external"
base_url = "https://external.example.com/v1"
env_key = "EXTERNAL_API_KEY"
"#;
        write_file(&codex_config_path, original);

        let owner_marker = RuntimeOwnerMarker::new(RuntimeOwnerKind::ManualCli, "codex", 33211)
            .with_lifecycle_mode(ProxyLifecycleMode::EphemeralConsole);
        let owner_lease =
            RuntimeOwnerLease::acquire(&owner_marker).expect("acquire foreground runtime owner");

        let guard = prepare_ch_codex_switch(
            "codex",
            ServeRuntimeOptions {
                enable_tui: true,
                auto_manage_codex_switch: true,
                ..ServeRuntimeOptions::default()
            },
            Some(&owner_lease),
            CodexClientPatchConfig::default(),
        )
        .expect("automatically switch Codex for ch")
        .expect("foreground ch must own a restore guard");

        let status = codex_switch::inspect().expect("inspect applied switch");
        assert_eq!(status.phase, CodexSwitchPhase::Applied);
        assert!(status.enabled);
        assert_eq!(status.base_url.as_deref(), Some("http://127.0.0.1:33211"));

        drop(guard);

        let restored = codex_switch::inspect().expect("inspect restored switch");
        assert_eq!(restored.phase, CodexSwitchPhase::Off);
        assert!(!restored.enabled);
        assert_eq!(
            std::fs::read_to_string(&codex_config_path).expect("read restored Codex config"),
            original
        );

        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ch_failed_start_preserves_an_existing_switch_generation() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-ch-failed-start-switch-test-{}",
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
        write_file(&codex_config_path, "model_provider = \"openai\"\n");
        codex_switch::apply(CodexSwitchIntent::On {
            validated_base_url: ValidatedCodexBaseUrl::local(33212),
        })
        .expect("prepare existing switch");
        let switch_state_path = codex_switch::inspect()
            .expect("inspect applied switch")
            .state_path;
        let config_before = std::fs::read(&codex_config_path).expect("read applied config");
        let state_before = std::fs::read(&switch_state_path).expect("read applied state");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let error = runtime
            .block_on(run_server(
                "codex",
                IpAddr::from([127, 0, 0, 1]),
                33212,
                ServeRuntimeOptions {
                    enable_tui: false,
                    auto_manage_codex_switch: true,
                    ..ServeRuntimeOptions::default()
                },
            ))
            .expect_err("empty helper config must fail startup");
        assert!(error.to_string().contains("provider"), "{error:#}");
        assert_eq!(
            std::fs::read(&codex_config_path).expect("read preserved config"),
            config_before
        );
        assert_eq!(
            std::fs::read(&switch_state_path).expect("read preserved state"),
            state_before
        );

        drop(env);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ch_imports_before_bind_but_never_switches_codex_when_bind_fails() {
        let _lock = env_lock();
        let root = std::env::temp_dir().join(format!(
            "codex-helper-ch-onboarding-bind-failure-test-{}",
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
        let codex_config = r#"# preserve the original client file
model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1"
env_key = "RELAY_API_KEY"
requires_openai_auth = false
"#;
        let codex_auth = r#"{"RELAY_API_KEY":"bind-failure-secret-canary"}"#;
        write_file(&codex_config_path, codex_config);
        write_file(&codex_auth_path, codex_auth);
        let occupied_proxy =
            std::net::TcpListener::bind("127.0.0.1:0").expect("occupy proxy address");
        let proxy_addr = occupied_proxy.local_addr().expect("occupied proxy address");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let error = runtime
            .block_on(run_server(
                "codex",
                proxy_addr.ip(),
                proxy_addr.port(),
                ServeRuntimeOptions {
                    enable_tui: false,
                    auto_manage_codex_switch: true,
                    ..ServeRuntimeOptions::default()
                },
            ))
            .expect_err("occupied proxy address must fail ch startup");
        assert!(
            error.chain().any(|cause| {
                matches!(
                    cause.downcast_ref::<ProxyListenerBindError>(),
                    Some(error) if error.kind() == crate::runtime_host::ProxyListenerKind::Proxy
                )
            }),
            "unexpected startup error: {error:#}"
        );

        let helper_config = std::fs::read_to_string(helper_home.join("config.toml"))
            .expect("read imported helper config");
        assert!(helper_config.contains("[codex.providers.relay]"));
        assert!(helper_config.contains("auth_token_env = \"RELAY_API_KEY\""));
        assert!(!helper_config.contains("bind-failure-secret-canary"));
        assert_eq!(
            std::fs::read_to_string(&codex_config_path).expect("read unchanged Codex config"),
            codex_config
        );
        assert_eq!(
            std::fs::read_to_string(&codex_auth_path).expect("read unchanged Codex auth"),
            codex_auth
        );
        let switch = codex_switch::inspect().expect("inspect switch after failed startup");
        assert_eq!(switch.phase, CodexSwitchPhase::Off);
        assert!(!switch.managed);

        drop(occupied_proxy);
        drop(env);
        let _ = std::fs::remove_dir_all(root);
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
        let _ = runtime.block_on(resolve_serve_tui_language(&loaded));
        let updated_helper_config =
            std::fs::read_to_string(&helper_config_path).expect("read helper config");
        assert!(updated_helper_config.contains("[notify]\nenabled = false"));
        assert!(updated_helper_config.contains("[ui]"));
        assert!(updated_helper_config.contains("language = \"auto\""));
        assert_eq!(
            std::fs::read_to_string(helper_config_path.with_file_name("config.toml.bak"))
                .expect("read exact helper config backup"),
            helper_config,
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
