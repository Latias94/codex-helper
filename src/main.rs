mod codex_integration;
mod commands;
mod config;
mod filter;
mod healthcheck;
mod lb;
mod logging;
mod model_routing;
mod notify;
mod proxy;
mod sessions;
mod state;
mod tui;
mod usage;
mod usage_providers;

use axum::Router;
use clap::{Parser, Subcommand, ValueEnum};
use owo_colors::OwoColorize;
use reqwest::Client;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::process::Command as ProcessCommand;
use std::sync::Arc;
use std::sync::Mutex;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::config::{
    ServiceKind, claude_settings_backup_path, claude_settings_path, codex_backup_config_path,
    codex_config_path, load_config, load_or_bootstrap_for_service, model_routing_warnings,
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
        /// Do not auto-import Codex providers from ~/.codex/config.toml (template only)
        #[arg(long)]
        no_import: bool,
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
        /// Priority group for level-based config routing (1..=10, lower is higher priority)
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=10))]
        level: u8,
        /// Exclude this config from automatic routing (unless it is active)
        #[arg(long)]
        disabled: bool,
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
    /// Set a config's level (1..=10, lower is higher priority)
    SetLevel {
        name: String,
        #[arg(value_parser = clap::value_parser!(u8).range(1..=10))]
        level: u8,
        /// Target Codex configs (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude configs
        #[arg(long)]
        claude: bool,
    },
    /// Enable a config for automatic routing
    Enable {
        name: String,
        /// Target Codex configs (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude configs
        #[arg(long)]
        claude: bool,
    },
    /// Disable a config for automatic routing (unless it is active)
    Disable {
        name: String,
        /// Target Codex configs (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude configs
        #[arg(long)]
        claude: bool,
    },
    /// Set retry policy to a curated profile (writes to ~/.codex-helper/config.*)
    #[command(name = "set-retry-profile")]
    SetRetryProfile {
        #[arg(value_enum)]
        profile: RetryProfile,
    },
    /// Import Codex upstream config from ~/.codex/config.toml + auth.json into ~/.codex-helper/config (toml/json)
    ImportFromCodex {
        /// Overwrite existing Codex configs in ~/.codex-helper/config (toml/json)
        #[arg(long)]
        force: bool,
    },
    /// Overwrite Codex upstream configs from ~/.codex/config.toml + auth.json
    ///
    /// This resets Codex configs in codex-helper back to Codex CLI defaults (including grouping/levels).
    #[command(name = "overwrite-from-codex")]
    OverwriteFromCodex {
        /// Preview changes without writing ~/.codex-helper/config (toml/json)
        #[arg(long)]
        dry_run: bool,
        /// Confirm overwriting configs (required unless --dry-run)
        #[arg(long)]
        yes: bool,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
enum RetryProfile {
    /// Default balanced settings (recommended for most users).
    Balanced,
    /// Prefer retrying the same upstream (useful for CF/network flakiness).
    SameUpstream,
    /// Try harder before giving up (more attempts; can increase cost/latency).
    AggressiveFailover,
    /// Cost-optimized primary/backup: enable cooldown exponential backoff for probe-back.
    CostPrimary,
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
        /// Truncate the first prompt to N characters (default: do not truncate)
        #[arg(long)]
        truncate: Option<usize>,
    },
    /// Print recent Codex sessions as `project_root session_id` (one per line)
    Recent {
        /// Maximum number of sessions to show
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Only include sessions updated within this duration (based on session file mtime)
        #[arg(long, default_value = "12h")]
        since: humantime::Duration,
        /// Print the raw session cwd instead of inferring a git project root
        #[arg(long)]
        raw_cwd: bool,
        /// Output format: text | tsv | json
        #[arg(long, value_enum, default_value_t = RecentFormat::Text)]
        format: RecentFormat,
        /// Open each session in a new terminal window/tab (best-effort; Windows-first)
        #[arg(long)]
        open: bool,
        /// Terminal backend used by --open (Windows: wt recommended; cross-platform: wezterm)
        #[arg(long, value_enum)]
        terminal: Option<RecentTerminal>,
        /// Shell executable for --open (Windows examples: `pwsh` or full path to pwsh.exe)
        #[arg(long)]
        shell: Option<String>,
        /// Keep the terminal open after running the resume command (best-effort)
        #[arg(long, default_value_t = true)]
        keep_open: bool,
        /// Resume command template; supports `{id}` placeholder
        #[arg(long, default_value = "codex resume {id}")]
        resume_cmd: String,
        /// Windows Terminal window id; use -1 to force a new window (wt only)
        #[arg(long, default_value_t = -1)]
        wt_window: i32,
        /// Delay between opening terminals (milliseconds)
        #[arg(long, default_value_t = 500)]
        delay_ms: u64,
        /// Print the spawn commands without executing them
        #[arg(long)]
        dry_run: bool,
    },
    /// Print a session transcript (best-effort) by session id
    Transcript {
        /// Session id (from `session list`)
        id: String,
        /// Print the full transcript (can be large)
        #[arg(long)]
        all: bool,
        /// Print only the last N messages (ignored with --all)
        #[arg(long, default_value_t = 40)]
        tail: usize,
        /// Output format: text | markdown | json
        #[arg(long, default_value = "text")]
        format: String,
        /// Include timestamps when available (text format only)
        #[arg(long)]
        timestamps: bool,
        /// Optional directory hint to resolve the session id; defaults to current dir
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

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
enum RecentFormat {
    Text,
    Tsv,
    Json,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
enum RecentTerminal {
    /// Windows Terminal (`wt`)
    Wt,
    /// WezTerm (`wezterm`)
    Wezterm,
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

    let mut cfg = match service_name {
        "codex" => load_or_bootstrap_for_service(ServiceKind::Codex).await?,
        "claude" => load_or_bootstrap_for_service(ServiceKind::Claude).await?,
        _ => load_or_bootstrap_for_service(ServiceKind::Codex).await?,
    };

    let tui_lang = {
        let env_lang_raw = std::env::var("CODEX_HELPER_TUI_LANG").ok();
        let env_lang = env_lang_raw.as_deref().and_then(|s| {
            if s.trim().eq_ignore_ascii_case("auto") {
                Some(tui::detect_system_language())
            } else {
                tui::parse_language(s)
            }
        });
        if let Some(l) = env_lang {
            l
        } else if let Some(s) = cfg.ui.language.as_deref() {
            if s.trim().eq_ignore_ascii_case("auto") {
                tui::detect_system_language()
            } else {
                tui::parse_language(s).unwrap_or_else(|| {
                    tracing::warn!("Invalid ui.language '{}', falling back to system locale", s);
                    tui::detect_system_language()
                })
            }
        } else {
            let detected = tui::detect_system_language();
            cfg.ui.language = Some(match detected {
                tui::Language::Zh => "zh".to_string(),
                tui::Language::En => "en".to_string(),
            });
            if let Err(err) = crate::config::save_config(&cfg).await {
                tracing::warn!("Failed to persist ui.language to config: {}", err);
            }
            detected
        }
    };

    let cfg = Arc::new(cfg);

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
    let listener = bind_local_listener_or_explain(addr, service_name).await?;
    tracing::info!(
        "codex-helper listening on http://{} (service: {})",
        addr,
        service_name
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let warnings = model_routing_warnings(&cfg, service_name);
    if !warnings.is_empty() {
        tracing::warn!("======== Model routing config warnings ========");
        for w in warnings {
            tracing::warn!("{}", w);
        }
        tracing::warn!("==============================================");
    }

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

        let providers = tui::build_provider_options(&cfg, service_name);

        let mut tui_handle = tokio::spawn(tui::run_dashboard(
            state,
            service_name,
            port,
            providers,
            tui_lang,
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

async fn bind_local_listener_or_explain(
    addr: SocketAddr,
    service_name: &'static str,
) -> anyhow::Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(addr).await.map_err(|err| {
        let help = listener_bind_help(addr, service_name, &err);
        anyhow::Error::new(err).context(help)
    })
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
        if let Some(hint) = port_owner_hint(port) {
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
        if let Some(hint) = port_owner_hint(port) {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct PortOwner {
    pid: u32,
    name: Option<String>,
}

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

fn run_cmd_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = ProcessCommand::new(program).args(args).output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return None;
    }
    Some(stdout)
}

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
                rest = &after;
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
