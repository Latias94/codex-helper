mod codex_integration;
mod commands;
mod config;
mod filter;
mod lb;
mod logging;
mod notify;
mod proxy;
mod sessions;
mod state;
mod tui;
mod usage;
mod usage_providers;

use axum::Router;
use clap::{Parser, Subcommand};
use owo_colors::OwoColorize;
use reqwest::Client;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::config::{
    ServiceKind, claude_settings_backup_path, claude_settings_path, codex_backup_config_path,
    codex_config_path, load_config, load_or_bootstrap_for_service,
};
use crate::proxy::{ProxyService, router as proxy_router};

#[derive(Parser, Debug)]
#[command(name = "codex-helper")]
#[command(about = "Helper proxy for Codex CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

pub type CliResult<T> = Result<T, CliError>;

#[derive(Debug)]
pub enum CliError {
    /// Errors related to codex-helper's config (config.json/config.toml)
    ProxyConfig(String),
    /// Errors while reading or interpreting Codex CLI config/auth files
    CodexConfig(String),
    /// Errors while working with usage logs / usage_providers.json
    Usage(String),
    /// Generic fallback for other failures
    Other(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::ProxyConfig(msg) => write!(f, "Proxy config error: {}", msg),
            CliError::CodexConfig(msg) => write!(f, "Codex config error: {}", msg),
            CliError::Usage(msg) => write!(f, "Usage error: {}", msg),
            CliError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for CliError {}

impl From<anyhow::Error> for CliError {
    fn from(e: anyhow::Error) -> Self {
        CliError::Other(e.to_string())
    }
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start HTTP proxy server (default Codex; use --claude for Claude)
    Serve {
        /// Target Codex service (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude service (experimental)
        #[arg(long)]
        claude: bool,
        /// Listen port (3211 for Codex, 3210 for Claude by default)
        #[arg(long)]
        port: Option<u16>,
        /// Disable built-in TUI dashboard (enabled by default when running in an interactive terminal)
        #[arg(long)]
        no_tui: bool,
    },
    /// Manage Codex/Claude switch-on/off state
    Switch {
        #[command(subcommand)]
        cmd: SwitchCommand,
    },
    /// Legacy: patch ~/.codex/config.toml to use local proxy (use `switch on` instead)
    #[command(hide = true)]
    SwitchOn {
        #[arg(long, default_value_t = 3211)]
        port: u16,
        /// Target Codex config (default)
        #[arg(long)]
        codex: bool,
        /// Target Claude settings (experimental)
        #[arg(long)]
        claude: bool,
    },
    /// Legacy: restore ~/.codex/config.toml from backup (use `switch off` instead)
    #[command(hide = true)]
    SwitchOff {
        /// Target Codex config (default)
        #[arg(long)]
        codex: bool,
        /// Target Claude settings (experimental)
        #[arg(long)]
        claude: bool,
    },
    /// Manage proxy configs for Codex / Claude
    Config {
        #[command(subcommand)]
        cmd: ConfigCommand,
    },
    /// Session-related helper commands (Codex sessions)
    Session {
        #[command(subcommand)]
        cmd: SessionCommand,
    },
    /// Run environment diagnostics for Codex CLI and codex-helper
    Doctor {
        /// Output diagnostics as JSON (machine-readable), without ANSI colors
        #[arg(long)]
        json: bool,
    },
    /// Show a brief status summary of codex-helper and upstream configs
    Status {
        /// Output status as JSON (machine-readable), without ANSI colors
        #[arg(long)]
        json: bool,
    },
    /// Inspect usage logs written by codex-helper
    Usage {
        #[command(subcommand)]
        cmd: UsageCommand,
    },
    /// Handle Codex notifications (for Codex `notify` hook)
    Notify {
        #[command(subcommand)]
        cmd: NotifyCommand,
    },
    /// Get or set the default target service (Codex/Claude) used by other commands
    Default {
        /// Set default to Codex
        #[arg(long)]
        codex: bool,
        /// Set default to Claude (experimental)
        #[arg(long)]
        claude: bool,
    },
}

#[derive(Subcommand, Debug)]
enum NotifyCommand {
    /// Process a Codex `notify` payload and show a system notification (best-effort)
    Codex {
        /// Codex passes the notification JSON as a single argument; for manual testing you can omit
        /// it and pipe JSON via stdin.
        notification_json: Option<String>,
        /// Do not show a system notification; only update notify state / run exec callbacks.
        #[arg(long)]
        no_toast: bool,
        /// Force enable system notification for this invocation (overrides config).
        #[arg(long)]
        toast: bool,
    },
    /// Internal: flush pending merged events and emit notifications (spawned in background).
    #[command(hide = true)]
    FlushCodex,
}

#[derive(Subcommand, Debug)]
enum SwitchCommand {
    /// Switch Codex/Claude config to use local proxy
    On {
        /// Listen port for local proxy; defaults to 3211
        #[arg(long, default_value_t = 3211)]
        port: u16,
        /// Target Codex config (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude settings (experimental)
        #[arg(long)]
        claude: bool,
    },
    /// Restore Codex/Claude config from backup (if present)
    Off {
        /// Target Codex config (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude settings (experimental)
        #[arg(long)]
        claude: bool,
    },
    /// Show current switch status for Codex/Claude
    Status {
        /// Show Codex switch status
        #[arg(long)]
        codex: bool,
        /// Show Claude switch status
        #[arg(long)]
        claude: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    /// Initialize a commented config template (TOML)
    Init {
        /// Overwrite existing config.toml (backing up to config.toml.bak)
        #[arg(long)]
        force: bool,
    },
    /// List configs in ~/.codex-helper/config.toml (or config.json)
    List {
        /// Target Codex configs (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude configs
        #[arg(long)]
        claude: bool,
    },
    /// Add a new config
    Add {
        name: String,
        #[arg(long)]
        base_url: String,
        #[arg(long)]
        auth_token: Option<String>,
        /// Read bearer token from an environment variable instead of storing it on disk
        #[arg(long, conflicts_with = "auth_token")]
        auth_token_env: Option<String>,
        /// Use X-API-Key header value (some providers)
        #[arg(long, conflicts_with = "api_key_env")]
        api_key: Option<String>,
        /// Read X-API-Key header value from an environment variable
        #[arg(long, conflicts_with = "api_key")]
        api_key_env: Option<String>,
        /// Optional alias for this config
        #[arg(long)]
        alias: Option<String>,
        /// Target Codex configs (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude configs
        #[arg(long)]
        claude: bool,
    },
    /// Set active config
    SetActive {
        name: String,
        /// Target Codex configs (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude configs
        #[arg(long)]
        claude: bool,
    },
    /// Import Codex upstream config from ~/.codex/config.toml + auth.json into ~/.codex-helper/config (toml/json)
    ImportFromCodex {
        /// Overwrite existing Codex configs in ~/.codex-helper/config (toml/json)
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
enum SessionCommand {
    /// List recent Codex sessions for the current project
    List {
        /// Maximum number of sessions to show
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Optional directory to search sessions for; defaults to current dir
        #[arg(long)]
        path: Option<String>,
    },
    /// Search Codex sessions by user message content
    Search {
        /// Substring to search in user messages
        query: String,
        /// Maximum number of sessions to show
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Optional directory to search sessions for; defaults to current dir
        #[arg(long)]
        path: Option<String>,
    },
    /// Export a Codex session to a file
    Export {
        /// Session id to export
        id: String,
        /// Output format: markdown or json
        #[arg(long, default_value = "markdown")]
        format: String,
        /// Optional output path; defaults to stdout
        #[arg(long)]
        output: Option<String>,
    },
    /// Show the last Codex session for the current project
    Last {
        /// Optional directory to search sessions for; defaults to current dir
        #[arg(long)]
        path: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum UsageCommand {
    /// Show recent requests with basic usage info from ~/.codex-helper/logs/requests.jsonl
    Tail {
        /// Maximum number of recent entries to print
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Print raw JSON lines instead of human-friendly format
        #[arg(long)]
        raw: bool,
    },
    /// Summarize total token usage per config from ~/.codex-helper/logs/requests.jsonl
    Summary {
        /// Maximum number of configs to show (sorted by total_tokens desc)
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[tokio::main]
async fn main() {
    if let Err(err) = real_main().await {
        eprintln!("{}", err.to_string().red());
        std::process::exit(1);
    }
}

async fn real_main() -> CliResult<()> {
    let cli = Cli::parse();
    let _log_guard = init_tracing(&cli);

    match cli.command.unwrap_or(Command::Serve {
        port: None,
        codex: false,
        claude: false,
        no_tui: false,
    }) {
        Command::Default { codex, claude } => {
            handle_default_cmd(codex, claude).await?;
            return Ok(());
        }
        Command::Switch { cmd } => {
            match cmd {
                SwitchCommand::On {
                    port,
                    codex,
                    claude,
                } => do_switch_on(port, codex, claude)?,
                SwitchCommand::Off { codex, claude } => do_switch_off(codex, claude)?,
                SwitchCommand::Status { codex, claude } => do_switch_status(codex, claude),
            }
            return Ok(());
        }
        Command::SwitchOn {
            port,
            codex,
            claude,
        } => {
            eprintln!(
                "{}",
                "Warning: `switch-on` is deprecated, please use `switch on` instead.".yellow()
            );
            do_switch_on(port, codex, claude)?;
            return Ok(());
        }
        Command::SwitchOff { codex, claude } => {
            eprintln!(
                "{}",
                "Warning: `switch-off` is deprecated, please use `switch off` instead.".yellow()
            );
            do_switch_off(codex, claude)?;
            return Ok(());
        }
        Command::Config { cmd } => {
            commands::config::handle_config_cmd(cmd).await?;
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
            commands::doctor::handle_status_cmd(json).await?;
            return Ok(());
        }
        Command::Usage { cmd } => {
            commands::usage::handle_usage_cmd(cmd).await?;
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
            no_tui,
        } => {
            if codex && claude {
                return Err(CliError::Other(
                    "Please specify at most one of --codex / --claude".to_string(),
                ));
            }

            // Explicit flags win; otherwise decide based on default_service (fallback: Codex).
            let service_name = if claude {
                "claude"
            } else if codex {
                "codex"
            } else {
                match load_config().await {
                    Ok(cfg) => match cfg.default_service {
                        Some(ServiceKind::Claude) => "claude",
                        _ => "codex",
                    },
                    Err(err) => {
                        tracing::warn!(
                            "Failed to load config for default service, falling back to Codex: {}",
                            err
                        );
                        "codex"
                    }
                }
            };
            let port = port.unwrap_or_else(|| if service_name == "codex" { 3211 } else { 3210 });
            run_server(service_name, port, !no_tui)
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
        Some(Command::Serve { no_tui, .. }) => {
            !*no_tui && atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout)
        }
        None => atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout),
        _ => false,
    };

    if interactive_tui {
        let log_dir = crate::config::proxy_home_dir().join("logs");
        let _ = std::fs::create_dir_all(&log_dir);

        rotate_runtime_log_if_needed(&log_dir);

        let file_appender = tracing_appender::rolling::never(&log_dir, "runtime.log");
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

fn rotate_runtime_log_if_needed(log_dir: &std::path::Path) {
    fn parse_u64_env(key: &str) -> Option<u64> {
        std::env::var(key)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&n| n > 0)
    }

    fn parse_usize_env(key: &str) -> Option<usize> {
        std::env::var(key)
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
    }

    let max_bytes = parse_u64_env("CODEX_HELPER_RUNTIME_LOG_MAX_BYTES").unwrap_or(20 * 1024 * 1024);
    let max_files = parse_usize_env("CODEX_HELPER_RUNTIME_LOG_MAX_FILES").unwrap_or(10);
    if max_bytes == 0 || max_files == 0 {
        return;
    }

    let path = log_dir.join("runtime.log");
    let Ok(meta) = std::fs::metadata(&path) else {
        return;
    };
    if meta.len() < max_bytes {
        return;
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let rotated_path = log_dir.join(format!("runtime.log.{ts}"));
    let _ = std::fs::rename(&path, &rotated_path);

    let Ok(rd) = std::fs::read_dir(log_dir) else {
        return;
    };
    let mut rotated: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.starts_with("runtime.log.") && s != "runtime.log")
                .unwrap_or(false)
        })
        .collect();
    if rotated.len() <= max_files {
        return;
    }
    rotated.sort();
    let remove_count = rotated.len().saturating_sub(max_files);
    for p in rotated.into_iter().take(remove_count) {
        let _ = std::fs::remove_file(p);
    }
}

async fn run_server(service_name: &'static str, port: u16, enable_tui: bool) -> anyhow::Result<()> {
    let interactive = enable_tui && atty::is(atty::Stream::Stdin) && atty::is(atty::Stream::Stdout);

    struct AutoRestoreGuard {
        service_name: &'static str,
    }

    impl Drop for AutoRestoreGuard {
        fn drop(&mut self) {
            // Always try to restore the upstream config on exit; if no backup exists, this is a no-op.
            if self.service_name == "claude" {
                match codex_integration::claude_switch_off() {
                    Ok(()) => tracing::info!("Claude settings restored from backup"),
                    Err(err) => {
                        tracing::warn!("Failed to restore Claude settings from backup: {}", err)
                    }
                }
            } else if self.service_name == "codex" {
                match codex_integration::switch_off() {
                    Ok(()) => tracing::info!("Codex config restored from backup"),
                    Err(err) => {
                        tracing::warn!("Failed to restore Codex config from backup: {}", err)
                    }
                }
            }
        }
    }

    let _restore_guard = AutoRestoreGuard { service_name };

    // In Codex mode, automatically switch Codex to the local proxy; in Claude mode, try updating
    // settings.json as well (experimental).
    if service_name == "codex" {
        // Guard before switching: if Codex is already pointing to the local proxy and a backup exists,
        // ask whether to restore first (interactive only).
        if let Err(err) = codex_integration::guard_codex_config_before_switch_on_interactive() {
            tracing::warn!("Failed to guard Codex config before switch-on: {}", err);
        }
        match codex_integration::switch_on(port) {
            Ok(()) => {
                tracing::info!("Codex config switched to local proxy on port {}", port);
            }
            Err(err) => {
                tracing::warn!("Failed to switch Codex config to local proxy: {}", err);
            }
        }
    } else if service_name == "claude" {
        if let Err(err) = codex_integration::guard_claude_settings_before_switch_on_interactive() {
            tracing::warn!("Failed to guard Claude settings before switch-on: {}", err);
        }
        match codex_integration::claude_switch_on(port) {
            Ok(()) => {
                tracing::info!(
                    "Claude settings updated to use local proxy on port {}",
                    port
                );
            }
            Err(err) => {
                tracing::warn!("Failed to update Claude settings for local proxy: {}", err);
            }
        }
    }

    let cfg = match service_name {
        "codex" => Arc::new(load_or_bootstrap_for_service(ServiceKind::Codex).await?),
        "claude" => Arc::new(load_or_bootstrap_for_service(ServiceKind::Claude).await?),
        _ => Arc::new(load_or_bootstrap_for_service(ServiceKind::Codex).await?),
    };

    // Require at least one valid upstream config, so we fail fast instead of discovering
    // it during an actual user request.
    if service_name == "codex" {
        if cfg.codex.configs.is_empty() || cfg.codex.active_config().is_none() {
            anyhow::bail!(
                "未找到任何可用的 Codex 上游配置，请先确保 ~/.codex/config.toml 与 ~/.codex/auth.json 配置完整，或手动编辑 ~/.codex-helper/config.toml（或 config.json）添加配置"
            );
        }
    } else if service_name == "claude"
        && (cfg.claude.configs.is_empty() || cfg.claude.active_config().is_none())
    {
        anyhow::bail!(
            "未找到任何可用的 Claude 上游配置，请先确保 ~/.claude/settings.json 配置完整，\
或在 ~/.codex-helper/config.toml（或 config.json）的 `claude` 段下手动添加上游配置"
        );
    }
    let client = Client::builder().build()?;

    // Shared LB state (failure counters, cooldowns, usage flags).
    let lb_states = Arc::new(Mutex::new(HashMap::new()));

    // Select service config based on service_name.
    let proxy = ProxyService::new(client, cfg.clone(), service_name, lb_states.clone());
    let state = proxy.state_handle();
    let app: Router = proxy_router(proxy);

    let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!(
        "codex-helper listening on http://{} (service: {})",
        addr,
        service_name
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    {
        let shutdown_tx = shutdown_tx.clone();
        tokio::spawn(async move {
            wait_for_shutdown_signal().await;
            let _ = shutdown_tx.send(true);
        });
    }

    let result = if interactive {
        let server_shutdown = {
            let mut rx = shutdown_rx.clone();
            async move {
                let _ = rx.changed().await;
            }
        };
        let mut server_handle = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .with_graceful_shutdown(server_shutdown)
                .await
        });

        let mut providers: Vec<tui::ProviderOption> = match service_name {
            "claude" => cfg
                .claude
                .configs
                .iter()
                .map(|(name, svc)| tui::ProviderOption {
                    name: name.clone(),
                    alias: svc.alias.clone(),
                })
                .collect(),
            _ => cfg
                .codex
                .configs
                .iter()
                .map(|(name, svc)| tui::ProviderOption {
                    name: name.clone(),
                    alias: svc.alias.clone(),
                })
                .collect(),
        };
        providers.sort_by(|a, b| a.name.cmp(&b.name));

        let mut tui_handle = tokio::spawn(tui::run_dashboard(
            state,
            service_name,
            port,
            providers,
            shutdown_tx.clone(),
            shutdown_rx.clone(),
        ));

        tokio::select! {
            server_res = &mut server_handle => {
                let _ = shutdown_tx.send(true);
                let _ = tui_handle.await;
                server_res.map_err(|e| anyhow::anyhow!("server task join error: {e}"))??;
                Ok::<(), anyhow::Error>(())
            }
            tui_res = &mut tui_handle => {
                match tui_res {
                    Ok(Ok(())) => {
                        // The dashboard requested a shutdown (or exited because shutdown was already triggered).
                        let _ = shutdown_tx.send(true);
                        server_handle.await.map_err(|e| anyhow::anyhow!("server task join error: {e}"))??;
                        Ok::<(), anyhow::Error>(())
                    }
                    Ok(Err(err)) => {
                        // If the dashboard fails (e.g. terminal issues), keep running without it.
                        tracing::warn!("TUI dashboard failed; continuing without TUI: {}", err);
                        server_handle.await.map_err(|e| anyhow::anyhow!("server task join error: {e}"))??;
                        Ok::<(), anyhow::Error>(())
                    }
                    Err(join_err) => {
                        tracing::warn!("TUI task join error; continuing without TUI: {}", join_err);
                        server_handle.await.map_err(|e| anyhow::anyhow!("server task join error: {e}"))??;
                        Ok::<(), anyhow::Error>(())
                    }
                }
            }
        }
    } else {
        let server_shutdown = {
            let mut rx = shutdown_rx.clone();
            async move {
                let _ = rx.changed().await;
            }
        };
        axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(server_shutdown)
            .await?;
        Ok(())
    };

    result?;

    Ok(())
}

fn do_switch_on(port: u16, codex: bool, claude: bool) -> CliResult<()> {
    if codex && claude {
        return Err(CliError::Other(
            "Please specify at most one of --codex / --claude".to_string(),
        ));
    }
    if claude {
        if let Err(err) = codex_integration::guard_claude_settings_before_switch_on_interactive() {
            tracing::warn!("Failed to guard Claude settings before switch-on: {}", err);
        }
        codex_integration::claude_switch_on(port)
            .map_err(|e| CliError::CodexConfig(e.to_string()))?;
    } else {
        codex_integration::guard_codex_config_before_switch_on_interactive()?;
        codex_integration::switch_on(port).map_err(|e| CliError::CodexConfig(e.to_string()))?;
    }
    Ok(())
}

fn do_switch_off(codex: bool, claude: bool) -> CliResult<()> {
    if codex && claude {
        return Err(CliError::Other(
            "Please specify at most one of --codex / --claude".to_string(),
        ));
    }
    if claude {
        codex_integration::claude_switch_off().map_err(|e| CliError::CodexConfig(e.to_string()))?;
    } else {
        codex_integration::switch_off().map_err(|e| CliError::CodexConfig(e.to_string()))?;
    }
    Ok(())
}

fn do_switch_status(codex_flag: bool, claude_flag: bool) {
    let both_unspecified = !codex_flag && !claude_flag;
    let show_codex = codex_flag || both_unspecified;
    let show_claude = claude_flag || both_unspecified;

    if show_codex {
        print_codex_switch_status();
        if show_claude {
            println!();
        }
    }
    if show_claude {
        print_claude_switch_status();
    }
}

async fn handle_default_cmd(codex: bool, claude: bool) -> CliResult<()> {
    if codex && claude {
        return Err(CliError::Other(
            "Please specify at most one of --codex / --claude".to_string(),
        ));
    }

    let mut cfg = load_config()
        .await
        .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

    if codex || claude {
        cfg.default_service = Some(if claude {
            ServiceKind::Claude
        } else {
            ServiceKind::Codex
        });
        crate::config::save_config(&cfg)
            .await
            .map_err(|e| CliError::ProxyConfig(e.to_string()))?;

        let name = if claude { "Claude" } else { "Codex" };
        println!("Default target service has been set to {}.", name);
    } else {
        let name = match cfg.default_service {
            Some(ServiceKind::Claude) => "Claude",
            _ => "Codex",
        };
        println!("Current default target service: {}.", name);
    }

    Ok(())
}

fn print_codex_switch_status() {
    use std::fs;

    let cfg_path = codex_config_path();
    let backup_path = codex_backup_config_path();

    println!("{}", "Codex 开关状态".bold());
    println!("  配置文件路径: {:?}", cfg_path);

    if !cfg_path.exists() {
        println!(
            "  当前未检测到 {:?}，可能尚未安装或初始化 Codex CLI。",
            cfg_path
        );
        return;
    }

    let text = match fs::read_to_string(&cfg_path) {
        Ok(t) => t,
        Err(err) => {
            println!("  无法读取配置文件：{}", err.to_string().red());
            return;
        }
    };

    let value: toml::Value = match text.parse() {
        Ok(v) => v,
        Err(err) => {
            println!("  无法解析配置为 TOML：{}", err.to_string().red());
            return;
        }
    };

    let table = match value.as_table() {
        Some(t) => t,
        None => {
            println!("  配置根节点不是 TOML 表，无法解析 model_provider。");
            return;
        }
    };

    let provider = table
        .get("model_provider")
        .and_then(|v| v.as_str())
        .unwrap_or("<未设置>");
    println!("  当前 model_provider: {}", provider.bold());

    if provider == "codex_proxy"
        && let Some(providers) = table.get("model_providers").and_then(|v| v.as_table())
        && let Some(proxy) = providers.get("codex_proxy").and_then(|v| v.as_table())
    {
        let base_url = proxy.get("base_url").and_then(|v| v.as_str()).unwrap_or("");
        let name = proxy.get("name").and_then(|v| v.as_str()).unwrap_or("");
        println!("  codex_proxy.name: {}", name);
        println!("  codex_proxy.base_url: {}", base_url);

        let is_local = base_url.contains("127.0.0.1") || base_url.contains("localhost");
        if is_local {
            println!("  -> 当前 Codex 已指向本地 codex-helper 代理。");
        }
    }

    if backup_path.exists() {
        println!(
            "  已检测到备份文件：{:?}（switch-off 将尝试从此处恢复）",
            backup_path
        );
    } else {
        println!(
            "  未检测到备份文件：{:?}，如直接修改过 config.toml，建议手动备份。",
            backup_path
        );
    }
}

fn print_claude_switch_status() {
    use serde_json::Value as JsonValue;
    use std::fs;

    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path();

    println!("{}", "Claude 开关状态（实验性）".bold());
    println!("  配置文件路径: {:?}", settings_path);

    if !settings_path.exists() {
        println!(
            "  当前未检测到 Claude 配置文件 {:?}，可能尚未安装或初始化 Claude Code。",
            settings_path
        );
        return;
    }

    let text = match fs::read_to_string(&settings_path) {
        Ok(t) => t,
        Err(err) => {
            println!("  无法读取配置文件：{}", err.to_string().red());
            return;
        }
    };

    let value: JsonValue = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(err) => {
            println!("  无法解析配置为 JSON：{}", err.to_string().red());
            return;
        }
    };

    let obj = match value.as_object() {
        Some(o) => o,
        None => {
            println!("  配置根节点不是 JSON 对象，无法解析 env 字段。");
            return;
        }
    };

    let env_obj = match obj.get("env").and_then(|v| v.as_object()) {
        Some(e) => e,
        None => {
            println!("  未检测到 env 字段，可能不是标准的 Claude 配置结构。");
            return;
        }
    };

    let base_url = env_obj
        .get("ANTHROPIC_BASE_URL")
        .and_then(|v| v.as_str())
        .unwrap_or("<未设置>");
    println!("  ANTHROPIC_BASE_URL: {}", base_url.bold());

    let is_local = base_url.contains("127.0.0.1") || base_url.contains("localhost");
    if is_local {
        println!("  -> 当前 Claude 已指向本地 codex-helper 代理。");
    }

    if backup_path.exists() {
        println!(
            "  已检测到备份文件：{:?}（switch off --claude 将尝试从此处恢复）",
            backup_path
        );
    } else {
        println!(
            "  未检测到备份文件：{:?}，如直接修改过 settings.json/claude.json，建议手动备份。",
            backup_path
        );
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
