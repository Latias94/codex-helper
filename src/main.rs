mod codex_integration;
mod commands;
mod config;
mod filter;
mod lb;
mod logging;
mod proxy;
mod sessions;
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
    /// Errors related to codex-helper's own config.json
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
    /// List configs in ~/.codex-helper/config.json
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
    /// Import Codex upstream config from ~/.codex/config.toml + auth.json into ~/.codex-helper/config.json
    ImportFromCodex {
        /// Overwrite existing Codex configs in ~/.codex-helper/config.json
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
    // 默认启用 info 级别日志，若用户设置了 RUST_LOG 则按其配置。
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Serve {
        port: None,
        codex: false,
        claude: false,
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
        Command::Serve {
            port,
            codex,
            claude,
        } => {
            if codex && claude {
                return Err(CliError::Other(
                    "Please specify at most one of --codex / --claude".to_string(),
                ));
            }

            // 显式指定优先；否则根据配置中的 default_service 决定默认服务（缺省为 Codex）。
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
            run_server(service_name, port)
                .await
                .map_err(|e| CliError::Other(e.to_string()))?;
        }
    }

    Ok(())
}

async fn run_server(service_name: &'static str, port: u16) -> anyhow::Result<()> {
    // Codex 模式下，自动将 Codex 切换到本地代理；Claude 模式也会尝试修改 settings.json（实验性）。
    if service_name == "codex" {
        // 在切换前做一次守护性检查：如发现 Codex 已指向本地代理且存在备份，则在交互模式下询问是否先恢复。
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

    // 严格要求存在至少一个有效的上游配置，否则直接报错退出，
    // 避免在运行时才发现无可用上游。
    if service_name == "codex" {
        if cfg.codex.configs.is_empty() || cfg.codex.active_config().is_none() {
            anyhow::bail!(
                "未找到任何可用的 Codex 上游配置，请先确保 ~/.codex/config.toml 与 ~/.codex/auth.json 配置完整，或手动编辑 ~/.codex-helper/config.json 添加配置"
            );
        }
    } else if service_name == "claude"
        && (cfg.claude.configs.is_empty() || cfg.claude.active_config().is_none())
    {
        anyhow::bail!(
            "未找到任何可用的 Claude 上游配置，请先确保 ~/.claude/settings.json 配置完整，\
或在 ~/.codex-helper/config.json 的 `claude` 段下手动添加上游配置"
        );
    }
    let client = Client::builder().build()?;

    // 统一的 LB 状态（失败计数、冷却、用量状态）
    let lb_states = Arc::new(Mutex::new(HashMap::new()));

    // 根据 service_name 选择对应服务配置。
    let proxy = ProxyService::new(client, cfg.clone(), service_name, lb_states.clone());
    let app: Router = proxy_router(proxy);

    let addr: SocketAddr = SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!(
        "codex-helper listening on http://{} (service: {})",
        addr,
        service_name
    );

    axum::serve(
        tokio::net::TcpListener::bind(addr).await?,
        app.into_make_service(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

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

    if provider == "codex_proxy" {
        if let Some(providers) = table.get("model_providers").and_then(|v| v.as_table()) {
            if let Some(proxy) = providers.get("codex_proxy").and_then(|v| v.as_table()) {
                let base_url = proxy.get("base_url").and_then(|v| v.as_str()).unwrap_or("");
                let name = proxy.get("name").and_then(|v| v.as_str()).unwrap_or("");
                println!("  codex_proxy.name: {}", name);
                println!("  codex_proxy.base_url: {}", base_url);

                let is_local = base_url.contains("127.0.0.1") || base_url.contains("localhost");
                if is_local {
                    println!("  -> 当前 Codex 已指向本地 codex-helper 代理。");
                }
            }
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

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = sigint.recv() => {},
            _ = sigterm.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }

    // 优雅退出时自动恢复 Codex 配置
    match codex_integration::switch_off() {
        Ok(()) => {
            tracing::info!("Codex config restored from backup");
        }
        Err(err) => {
            tracing::warn!("Failed to restore Codex config from backup: {}", err);
        }
    }
}
