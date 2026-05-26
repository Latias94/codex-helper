use clap::{Parser, Subcommand, ValueEnum};
use std::net::IpAddr;

#[derive(Parser, Debug)]
#[command(name = "codex-helper", version)]
#[command(about = "Helper proxy for Codex CLI", long_about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
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
    /// Errors while reading or writing local pricing overrides
    Pricing(String),
    /// Generic fallback for other failures
    Other(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::ProxyConfig(msg) => write!(f, "Proxy config error: {}", msg),
            CliError::CodexConfig(msg) => write!(f, "Codex config error: {}", msg),
            CliError::Usage(msg) => write!(f, "Usage error: {}", msg),
            CliError::Pricing(msg) => write!(f, "Pricing error: {}", msg),
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
pub(crate) enum Command {
    /// Start HTTP proxy server (default Codex; use --claude for Claude)
    Serve {
        /// Target Codex service (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude service (experimental)
        #[arg(long)]
        claude: bool,
        /// Listen host (127.0.0.1 by default; use 0.0.0.0 to expose on LAN at your own risk)
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Listen port (3211 for Codex, 3210 for Claude by default)
        #[arg(long)]
        port: Option<u16>,
        /// Disable built-in TUI dashboard (enabled by default when running in an interactive terminal)
        #[arg(long)]
        no_tui: bool,
        /// Keep the proxy resident when the operator console exits; client patch is not auto-restored
        #[arg(long)]
        resident: bool,
        /// Mark the resident child as supervisor-owned; internal use by `daemon supervise`
        #[arg(long, hide = true)]
        supervisor_managed: bool,
        /// Mark a resident proxy as desktop/tray-owned; intended for future desktop shells
        #[arg(long, hide = true)]
        desktop_managed: bool,
    },
    /// Inspect or control a resident codex-helper proxy
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCommand,
    },
    /// Attach a read-only TUI dashboard to an already-running local resident proxy
    Tui {
        /// Target Codex proxy (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude proxy
        #[arg(long)]
        claude: bool,
        /// Proxy port; defaults to 3211 for Codex and 3210 for Claude
        #[arg(long)]
        port: Option<u16>,
    },
    /// Manage Codex/Claude switch on/off state
    Switch {
        #[command(subcommand)]
        cmd: SwitchCommand,
    },
    /// Manage codex-helper config files and schema migration
    Config {
        #[command(subcommand)]
        cmd: ConfigCommand,
    },
    /// Manage provider routing policy and runtime routing views
    Routing {
        #[command(subcommand)]
        cmd: RoutingCommand,
    },
    /// Manage route graph provider catalog for v4 configs
    Provider {
        #[command(subcommand)]
        cmd: ProviderCommand,
    },
    /// Codex-specific relay diagnostics and evidence commands
    Codex {
        #[command(subcommand)]
        cmd: CodexCommand,
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
    /// Show a brief status summary of codex-helper and upstream routing
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
    /// Manage local model pricing overrides
    Pricing {
        #[command(subcommand)]
        cmd: PricingCommand,
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
pub(crate) enum DaemonCommand {
    /// Show whether a local resident proxy is reachable
    Status {
        /// Target Codex proxy (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude proxy
        #[arg(long)]
        claude: bool,
        /// Proxy port; defaults to 3211 for Codex and 3210 for Claude
        #[arg(long)]
        port: Option<u16>,
        /// Output status as JSON
        #[arg(long)]
        json: bool,
    },
    /// Ask a resident proxy to gracefully stop
    Stop {
        /// Target Codex proxy (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude proxy
        #[arg(long)]
        claude: bool,
        /// Proxy port; defaults to 3211 for Codex and 3210 for Claude
        #[arg(long)]
        port: Option<u16>,
    },
    /// Run a foreground watchdog that restarts a resident proxy child after crashes
    Supervise {
        /// Target Codex proxy (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude proxy
        #[arg(long)]
        claude: bool,
        /// Listen host for the child resident proxy
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Proxy port; defaults to 3211 for Codex and 3210 for Claude
        #[arg(long)]
        port: Option<u16>,
        /// Maximum crash restarts before the supervisor gives up
        #[arg(long, default_value_t = 10)]
        max_restarts: u32,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum CodexCommand {
    /// Run validation-only Codex relay capability diagnostics
    #[command(name = "relay-capabilities")]
    Capabilities {
        /// Target station/provider name; defaults to the current Codex routing target
        #[arg(long)]
        station: Option<String>,
        /// Target route-graph provider id; mutually exclusive with --station
        #[arg(long)]
        provider: Option<String>,
        /// Target route-graph endpoint id; requires --provider
        #[arg(long)]
        endpoint: Option<String>,
        /// Target upstream index inside a legacy station/provider
        #[arg(long = "upstream-index")]
        upstream_index: Option<usize>,
        /// Requested model used for model-catalog capability interpretation
        #[arg(long)]
        model: Option<String>,
        /// Preset to evaluate; defaults to current switch/config preset
        #[arg(long = "preset", alias = "mode", value_enum)]
        preset: Option<CodexClientPatchPresetArg>,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Run strongly opt-in Codex relay live smoke against one selected upstream
    #[command(name = "relay-live-smoke")]
    LiveSmoke {
        /// Required exact acknowledgement before any upstream live-smoke request is sent
        #[arg(long = "acknowledgement", value_name = "ACK")]
        acknowledgement: String,
        /// Target station/provider name; defaults to the current Codex routing target
        #[arg(long)]
        station: Option<String>,
        /// Target route-graph provider id; mutually exclusive with --station
        #[arg(long)]
        provider: Option<String>,
        /// Target route-graph endpoint id; requires --provider
        #[arg(long)]
        endpoint: Option<String>,
        /// Target upstream index inside a legacy station/provider
        #[arg(long = "upstream-index")]
        upstream_index: Option<usize>,
        /// Requested model; required for live smoke
        #[arg(long)]
        model: String,
        /// Run hosted image_generation smoke instead of the default compact smoke
        #[arg(long)]
        image: bool,
        /// Run remote_compaction_v2 streaming smoke instead of the default compact smoke
        #[arg(long = "compact-v2", alias = "remote-compaction-v2")]
        compact_v2: bool,
        /// Run Responses WebSocket smoke instead of the default compact smoke
        #[arg(long)]
        websocket: bool,
        /// Optional service tier forwarded to the live-smoke request body
        #[arg(long = "service-tier")]
        service_tier: Option<String>,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// List local Codex relay diagnostic evidence records
    #[command(name = "relay-evidence")]
    Evidence {
        /// Maximum number of records to show, newest first
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Filter by evidence kind
        #[arg(long, value_enum)]
        kind: Option<CodexRelayEvidenceKindArg>,
        /// Match station/provider name substring
        #[arg(long)]
        station: Option<String>,
        /// Match requested model substring
        #[arg(long)]
        model: Option<String>,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum NotifyCommand {
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
pub(crate) enum SwitchCommand {
    /// Switch Codex/Claude config to use local proxy
    On {
        /// Listen port for local proxy; defaults to 3211
        #[arg(long, default_value_t = 3211)]
        port: u16,
        /// Codex client preset; if omitted, use the preset from ~/.codex-helper/config.toml
        #[arg(long = "preset", alias = "mode", value_enum)]
        preset: Option<CodexClientPatchPresetArg>,
        /// Enable Responses WebSocket transport advertising for official bridge presets
        #[arg(long)]
        responses_websocket: bool,
        /// Target Codex config (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude settings (experimental)
        #[arg(long)]
        claude: bool,
    },
    /// Disable local proxy integration (Codex is patched in place; Claude restores backup)
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
    /// Manage Codex App mobile remote-control enablement
    RemoteControl {
        #[command(subcommand)]
        cmd: RemoteControlCommand,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum RemoteControlCommand {
    /// Enable Codex App mobile remote control on this machine
    Enable,
    /// Show Codex App mobile remote-control status
    Status,
    /// Check Codex logs for a successful experimentalFeature/enablement/set response
    CheckLogs,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexClientPatchPresetArg {
    /// Historical codex-helper patch behavior
    Default,
    /// Keep ChatGPT account auth while routing model traffic through codex-helper
    ChatgptBridge,
    /// Experimental image generation bridge using a minimal ChatGPT auth facade
    ImagegenBridge,
    /// Experimental official relay preset for HTTP OpenAI Responses features
    #[value(name = "official-relay", alias = "official-relay-bridge")]
    OfficialRelayBridge,
    /// Experimental official imagegen preset plus minimal image generation auth facade
    #[value(name = "official-imagegen", alias = "official-imagegen-bridge")]
    OfficialImagegenBridge,
}

impl From<CodexClientPatchPresetArg> for codex_helper_core::codex_integration::CodexPatchMode {
    fn from(value: CodexClientPatchPresetArg) -> Self {
        match value {
            CodexClientPatchPresetArg::Default => Self::Default,
            CodexClientPatchPresetArg::ChatgptBridge => Self::ChatGptBridge,
            CodexClientPatchPresetArg::ImagegenBridge => Self::ImagegenBridge,
            CodexClientPatchPresetArg::OfficialRelayBridge => Self::OfficialRelayBridge,
            CodexClientPatchPresetArg::OfficialImagegenBridge => Self::OfficialImagegenBridge,
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub(crate) enum CodexRelayEvidenceKindArg {
    CapabilityDiagnostics,
    LiveSmoke,
}

impl From<CodexRelayEvidenceKindArg> for codex_helper_core::proxy::CodexRelayEvidenceKind {
    fn from(value: CodexRelayEvidenceKindArg) -> Self {
        match value {
            CodexRelayEvidenceKindArg::CapabilityDiagnostics => Self::CapabilityDiagnostics,
            CodexRelayEvidenceKindArg::LiveSmoke => Self::LiveSmoke,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Initialize a commented routing-first TOML config template
    Init {
        /// Overwrite existing config.toml (backing up to config.toml.bak)
        #[arg(long)]
        force: bool,
        /// Do not auto-import Codex providers from ~/.codex/config.toml (template only)
        #[arg(long)]
        no_import: bool,
    },
    /// Set retry policy to a curated profile (writes to ~/.codex-helper/config.*)
    #[command(name = "set-retry-profile")]
    SetRetryProfile {
        #[arg(value_enum)]
        profile: RetryProfile,
    },
    /// Import Codex providers from ~/.codex/config.toml + auth.json into codex-helper config
    ImportFromCodex {
        /// Overwrite existing Codex providers in codex-helper config
        #[arg(long)]
        force: bool,
    },
    /// Overwrite Codex providers from ~/.codex/config.toml + auth.json
    ///
    /// This resets Codex providers in codex-helper back to Codex CLI defaults.
    #[command(name = "overwrite-from-codex")]
    OverwriteFromCodex {
        /// Preview changes without writing ~/.codex-helper/config (toml/json)
        #[arg(long)]
        dry_run: bool,
        /// Confirm overwriting providers (required unless --dry-run)
        #[arg(long)]
        yes: bool,
    },

    /// Preview or write migration output for the current route graph schema
    Migrate {
        /// Preview only; print migrated TOML to stdout
        #[arg(long, conflicts_with = "write")]
        dry_run: bool,
        /// Write migrated TOML to ~/.codex-helper/config.toml
        #[arg(long, conflicts_with = "dry_run")]
        write: bool,
        /// Confirm writing migrated config to disk
        #[arg(long, requires = "write")]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RoutingCommand {
    /// Show the current route graph recipe for Codex or Claude
    Show {
        /// Target Codex routing (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude routing
        #[arg(long)]
        claude: bool,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// List the current runtime routing candidates
    List {
        /// Target Codex routing (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude routing
        #[arg(long)]
        claude: bool,
    },
    /// Explain the current runtime routing order
    Explain {
        /// Target Codex routing (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude routing
        #[arg(long)]
        claude: bool,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
        /// Show details for a single provider
        #[arg(long)]
        provider: Option<String>,
        /// Explain selection for a requested model
        #[arg(long)]
        model: Option<String>,
        /// Explain selection for a requested service tier
        #[arg(long = "service-tier")]
        service_tier: Option<String>,
        /// Explain selection for a requested reasoning effort
        #[arg(long = "reasoning-effort")]
        reasoning_effort: Option<String>,
        /// Explain selection for a request path
        #[arg(long)]
        path: Option<String>,
        /// Explain selection for an HTTP method
        #[arg(long)]
        method: Option<String>,
        /// Explain selection for a header condition, as NAME=VALUE; repeatable
        #[arg(long = "header", value_name = "NAME=VALUE")]
        headers: Vec<String>,
    },
    /// Patch routing fields directly
    Set {
        /// Routing policy
        #[arg(long, value_enum)]
        policy: Option<RoutingPolicy>,
        /// Manual-sticky target route, provider, or provider endpoint; implies manual-sticky when policy is omitted
        #[arg(long)]
        target: Option<String>,
        /// Clear manual target; switches manual-sticky back to ordered-failover
        #[arg(long, conflicts_with = "target")]
        clear_target: bool,
        /// Ordered provider list; comma-separated values are accepted
        #[arg(
            long = "order",
            value_delimiter = ',',
            value_name = "PROVIDER[,PROVIDER...]"
        )]
        order: Vec<String>,
        /// Preferred provider tag in KEY=VALUE form; can be passed multiple times
        #[arg(long = "prefer-tag", value_name = "KEY=VALUE")]
        prefer_tags: Vec<String>,
        /// Clear all preferred tag filters
        #[arg(long, conflicts_with = "prefer_tags")]
        clear_prefer_tags: bool,
        /// What to do after preferred providers are exhausted
        #[arg(long, value_enum)]
        on_exhausted: Option<RoutingExhaustedAction>,
        /// Target Codex routing (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude routing
        #[arg(long)]
        claude: bool,
    },
    /// Pin routing to one route, provider, or provider endpoint
    Pin {
        target: String,
        /// Target Codex routing (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude routing
        #[arg(long)]
        claude: bool,
    },
    /// Switch to ordered failover with an explicit provider order
    Order {
        /// Provider order, first provider is tried first
        #[arg(value_name = "PROVIDER", required = true)]
        providers: Vec<String>,
        /// Target Codex routing (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude routing
        #[arg(long)]
        claude: bool,
    },
    /// Prefer providers matching tags, then fall back by order unless stopped
    PreferTag {
        /// Preferred provider tag in KEY=VALUE form; can be passed multiple times
        #[arg(long = "tag", value_name = "KEY=VALUE", required = true)]
        tags: Vec<String>,
        /// Optional fallback order; comma-separated values are accepted
        #[arg(
            long = "order",
            value_delimiter = ',',
            value_name = "PROVIDER[,PROVIDER...]"
        )]
        order: Vec<String>,
        /// What to do after preferred providers are exhausted
        #[arg(long, value_enum)]
        on_exhausted: Option<RoutingExhaustedAction>,
        /// Target Codex routing (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude routing
        #[arg(long)]
        claude: bool,
    },
    /// Clear manual target and return to ordered failover
    ClearTarget {
        /// Target Codex routing (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude routing
        #[arg(long)]
        claude: bool,
    },
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
pub enum ProviderCommand {
    /// Show providers in the v4 catalog
    List {
        /// Target Codex provider catalog (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude provider catalog
        #[arg(long)]
        claude: bool,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Show one provider in detail
    Show {
        name: String,
        /// Target Codex provider catalog (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude provider catalog
        #[arg(long)]
        claude: bool,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Add or replace a provider
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
        /// Optional alias for this provider
        #[arg(long)]
        alias: Option<String>,
        /// Provider tag in KEY=VALUE form; can be passed multiple times
        #[arg(long = "tag", value_name = "KEY=VALUE")]
        tags: Vec<String>,
        /// Model that this provider can accept before mapping; can be passed multiple times
        #[arg(long = "supported-model", value_name = "MODEL")]
        supported_models: Vec<String>,
        /// Rewrite requested model names for this provider, in FROM=TO form; supports one '*' wildcard
        #[arg(long = "model-map", value_name = "FROM=TO")]
        model_mapping: Vec<String>,
        /// Exclude this provider from automatic routing
        #[arg(long)]
        disabled: bool,
        /// Replace an existing provider with the same name
        #[arg(long)]
        replace: bool,
        /// Target Codex provider catalog (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude provider catalog
        #[arg(long)]
        claude: bool,
    },
    /// Enable a provider for automatic routing
    Enable {
        name: String,
        /// Target Codex provider catalog (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude provider catalog
        #[arg(long)]
        claude: bool,
    },
    /// Disable a provider for automatic routing
    Disable {
        name: String,
        /// Target Codex provider catalog (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude provider catalog
        #[arg(long)]
        claude: bool,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum RoutingPolicy {
    ManualSticky,
    OrderedFailover,
    TagPreferred,
}

impl From<RoutingPolicy> for codex_helper_core::config::RoutingPolicyV4 {
    fn from(value: RoutingPolicy) -> Self {
        match value {
            RoutingPolicy::ManualSticky => Self::ManualSticky,
            RoutingPolicy::OrderedFailover => Self::OrderedFailover,
            RoutingPolicy::TagPreferred => Self::TagPreferred,
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum RoutingExhaustedAction {
    Continue,
    Stop,
}

impl From<RoutingExhaustedAction> for codex_helper_core::config::RoutingExhaustedActionV4 {
    fn from(value: RoutingExhaustedAction) -> Self {
        match value {
            RoutingExhaustedAction::Continue => Self::Continue,
            RoutingExhaustedAction::Stop => Self::Stop,
        }
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum RetryProfile {
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
pub enum SessionCommand {
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
        /// Truncate the first prompt to N characters (default: do not truncate)
        #[arg(long)]
        truncate: Option<usize>,
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
pub enum RecentFormat {
    Text,
    Tsv,
    Json,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum RecentTerminal {
    /// Windows Terminal (`wt`)
    Wt,
    /// WezTerm (`wezterm`)
    Wezterm,
}

#[derive(Subcommand, Debug)]
pub enum UsageCommand {
    /// Show recent requests with basic usage info from ~/.codex-helper/logs/requests.jsonl
    Tail {
        /// Maximum number of recent entries to print
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Print raw JSON lines instead of human-friendly format
        #[arg(long)]
        raw: bool,
    },
    /// Summarize total token usage from ~/.codex-helper/logs/requests.jsonl
    Summary {
        /// Maximum number of summary rows to show (sorted by total_tokens desc)
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Group summary rows by routing target, provider, model, or session
        #[arg(long, value_enum, default_value_t = UsageSummaryBy::Station)]
        by: UsageSummaryBy,
    },
    /// Find matching request records in ~/.codex-helper/logs/requests.jsonl
    Find {
        /// Maximum number of matching entries to print, newest first
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Match session id substring
        #[arg(long)]
        session: Option<String>,
        /// Match model id substring
        #[arg(long)]
        model: Option<String>,
        /// Match routing target/config name substring
        #[arg(long)]
        station: Option<String>,
        /// Match provider id substring
        #[arg(long)]
        provider: Option<String>,
        /// Match request path substring, for example responses/compact
        #[arg(long)]
        path: Option<String>,
        /// Match status_code >= this value
        #[arg(long)]
        status_min: Option<u64>,
        /// Match status_code <= this value
        #[arg(long)]
        status_max: Option<u64>,
        /// Shortcut for status_code >= 400
        #[arg(long)]
        errors: bool,
        /// Match fast/priority requests
        #[arg(long)]
        fast: bool,
        /// Match retried/failover requests
        #[arg(long)]
        retried: bool,
        /// Print raw JSON lines instead of human-friendly format
        #[arg(long)]
        raw: bool,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum UsageSummaryBy {
    Station,
    Provider,
    Model,
    Session,
}

#[derive(Subcommand, Debug)]
pub enum PricingCommand {
    /// Print the local pricing override path
    Path,
    /// List the merged price catalog or only local overrides
    List {
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
        /// Show only local override rows instead of the merged operator catalog
        #[arg(long)]
        local: bool,
        /// Filter rows by model id or alias match
        #[arg(long)]
        model: Option<String>,
    },
    /// Add or replace a local model price override
    Set {
        model_id: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long = "alias")]
        aliases: Vec<String>,
        #[arg(long)]
        input_per_1m_usd: String,
        #[arg(long)]
        output_per_1m_usd: String,
        #[arg(long)]
        cache_read_input_per_1m_usd: Option<String>,
        #[arg(long)]
        cache_creation_input_per_1m_usd: Option<String>,
        #[arg(long, value_enum, default_value_t = PricingConfidence::Estimated)]
        confidence: PricingConfidence,
    },
    /// Remove a local model price override
    Remove { model_id: String },
    /// Pull a remote pricing catalog JSON into local overrides
    Sync {
        /// URL returning a ModelPriceCatalogSnapshot JSON payload
        url: String,
        /// Import only rows matching these model ids or aliases
        #[arg(long = "model")]
        models: Vec<String>,
        /// Replace local overrides instead of merging into them
        #[arg(long)]
        replace: bool,
        /// Show what would be imported without writing pricing_overrides.toml
        #[arg(long)]
        dry_run: bool,
    },
    /// Pull basellm llm-metadata pricing JSON into local overrides
    SyncBasellm {
        /// URL returning basellm all.json data
        #[arg(
            long,
            default_value = "https://basellm.github.io/llm-metadata/api/all.json"
        )]
        url: String,
        /// Import only rows matching these model ids or aliases
        #[arg(long = "model")]
        models: Vec<String>,
        /// Replace local overrides instead of merging into them
        #[arg(long)]
        replace: bool,
        /// Show what would be imported without writing pricing_overrides.toml
        #[arg(long)]
        dry_run: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn switch_on_without_preset_leaves_preset_unset_for_config_fallback() {
        let cli = Cli::try_parse_from(["codex-helper", "switch", "on"])
            .expect("parse switch on without explicit preset");

        let Some(Command::Switch {
            cmd:
                SwitchCommand::On {
                    preset,
                    responses_websocket,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected switch on command");
        };
        assert_eq!(preset, None);
        assert!(!responses_websocket);
    }

    #[test]
    fn switch_on_with_explicit_preset_keeps_requested_preset() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "switch",
            "on",
            "--preset",
            "official-imagegen",
            "--responses-websocket",
        ])
        .expect("parse switch on with explicit preset");

        let Some(Command::Switch {
            cmd:
                SwitchCommand::On {
                    preset,
                    responses_websocket,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected switch on command");
        };
        assert_eq!(
            preset,
            Some(CodexClientPatchPresetArg::OfficialImagegenBridge)
        );
        assert!(responses_websocket);
    }

    #[test]
    fn codex_relay_cli_parses_capability_diagnostics() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "codex",
            "relay-capabilities",
            "--model",
            "gpt-5.5",
            "--preset",
            "official-imagegen",
            "--provider",
            "ciii",
            "--endpoint",
            "default",
            "--json",
        ])
        .expect("parse codex relay capabilities");

        let Some(Command::Codex {
            cmd:
                CodexCommand::Capabilities {
                    model,
                    preset,
                    provider,
                    endpoint,
                    json,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected codex relay capabilities command");
        };
        assert_eq!(model.as_deref(), Some("gpt-5.5"));
        assert_eq!(
            preset,
            Some(CodexClientPatchPresetArg::OfficialImagegenBridge)
        );
        assert_eq!(provider.as_deref(), Some("ciii"));
        assert_eq!(endpoint.as_deref(), Some("default"));
        assert!(json);
    }

    #[test]
    fn codex_relay_cli_accepts_legacy_mode_alias() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "codex",
            "relay-capabilities",
            "--mode",
            "official-imagegen-bridge",
        ])
        .expect("parse legacy codex relay capabilities mode alias");

        let Some(Command::Codex {
            cmd: CodexCommand::Capabilities { preset, .. },
        }) = cli.command
        else {
            panic!("expected codex relay capabilities command");
        };
        assert_eq!(
            preset,
            Some(CodexClientPatchPresetArg::OfficialImagegenBridge)
        );
    }

    #[test]
    fn codex_relay_cli_live_smoke_requires_acknowledgement_argument() {
        let error = Cli::try_parse_from([
            "codex-helper",
            "codex",
            "relay-live-smoke",
            "--model",
            "gpt-5.5",
        ])
        .expect_err("missing acknowledgement should fail clap parse");

        assert!(error.to_string().contains("acknowledgement"));
    }

    #[test]
    fn codex_relay_cli_parses_live_smoke_image_flag() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "codex",
            "relay-live-smoke",
            "--acknowledgement",
            "run-live-codex-relay-smoke",
            "--model",
            "gpt-5.5",
            "--image",
        ])
        .expect("parse codex relay live smoke");

        let Some(Command::Codex {
            cmd:
                CodexCommand::LiveSmoke {
                    acknowledgement,
                    model,
                    image,
                    compact_v2,
                    websocket,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected codex relay live smoke command");
        };
        assert_eq!(acknowledgement, "run-live-codex-relay-smoke");
        assert_eq!(model, "gpt-5.5");
        assert!(image);
        assert!(!compact_v2);
        assert!(!websocket);
    }

    #[test]
    fn codex_relay_cli_parses_live_smoke_websocket_flag() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "codex",
            "relay-live-smoke",
            "--acknowledgement",
            "run-live-codex-relay-smoke",
            "--model",
            "gpt-5.5",
            "--provider",
            "input8",
            "--websocket",
        ])
        .expect("parse codex relay live smoke websocket flag");

        let Some(Command::Codex {
            cmd:
                CodexCommand::LiveSmoke {
                    model,
                    provider,
                    compact_v2,
                    websocket,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected codex relay live smoke command");
        };
        assert_eq!(model, "gpt-5.5");
        assert_eq!(provider.as_deref(), Some("input8"));
        assert!(!compact_v2);
        assert!(websocket);
    }

    #[test]
    fn codex_relay_cli_parses_live_smoke_compact_v2_flag() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "codex",
            "relay-live-smoke",
            "--acknowledgement",
            "run-live-codex-relay-smoke",
            "--model",
            "gpt-5.5",
            "--compact-v2",
        ])
        .expect("parse codex relay live smoke compact v2 flag");

        let Some(Command::Codex {
            cmd: CodexCommand::LiveSmoke {
                model, compact_v2, ..
            },
        }) = cli.command
        else {
            panic!("expected codex relay live smoke command");
        };
        assert_eq!(model, "gpt-5.5");
        assert!(compact_v2);
    }

    #[test]
    fn codex_relay_cli_parses_evidence_filters() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "codex",
            "relay-evidence",
            "--kind",
            "live-smoke",
            "--station",
            "input",
            "--limit",
            "5",
        ])
        .expect("parse codex relay evidence");

        let Some(Command::Codex {
            cmd:
                CodexCommand::Evidence {
                    kind,
                    station,
                    limit,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected codex relay evidence command");
        };
        assert_eq!(kind, Some(CodexRelayEvidenceKindArg::LiveSmoke));
        assert_eq!(station.as_deref(), Some("input"));
        assert_eq!(limit, 5);
    }

    #[test]
    fn serve_cli_parses_resident_flag() {
        let cli = Cli::try_parse_from(["codex-helper", "serve", "--codex", "--resident"])
            .expect("parse resident serve command");

        let Some(Command::Serve {
            codex, resident, ..
        }) = cli.command
        else {
            panic!("expected serve command");
        };
        assert!(codex);
        assert!(resident);
    }

    #[test]
    fn serve_cli_parses_hidden_supervisor_managed_flag() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "serve",
            "--codex",
            "--resident",
            "--supervisor-managed",
        ])
        .expect("parse supervisor-managed serve command");

        let Some(Command::Serve {
            codex,
            resident,
            supervisor_managed,
            ..
        }) = cli.command
        else {
            panic!("expected serve command");
        };
        assert!(codex);
        assert!(resident);
        assert!(supervisor_managed);
    }

    #[test]
    fn serve_cli_parses_hidden_desktop_managed_flag() {
        let cli = Cli::try_parse_from(["codex-helper", "serve", "--codex", "--desktop-managed"])
            .expect("parse desktop-managed serve command");

        let Some(Command::Serve {
            codex,
            resident,
            desktop_managed,
            ..
        }) = cli.command
        else {
            panic!("expected serve command");
        };
        assert!(codex);
        assert!(!resident);
        assert!(desktop_managed);
    }

    #[test]
    fn daemon_cli_parses_status_json() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "daemon",
            "status",
            "--claude",
            "--port",
            "4210",
            "--json",
        ])
        .expect("parse daemon status command");

        let Some(Command::Daemon {
            cmd: DaemonCommand::Status {
                claude, port, json, ..
            },
        }) = cli.command
        else {
            panic!("expected daemon status command");
        };
        assert!(claude);
        assert_eq!(port, Some(4210));
        assert!(json);
    }

    #[test]
    fn daemon_cli_parses_stop() {
        let cli = Cli::try_parse_from(["codex-helper", "daemon", "stop", "--codex"])
            .expect("parse daemon stop command");

        let Some(Command::Daemon {
            cmd:
                DaemonCommand::Stop {
                    codex,
                    claude,
                    port,
                },
        }) = cli.command
        else {
            panic!("expected daemon stop command");
        };
        assert!(codex);
        assert!(!claude);
        assert_eq!(port, None);
    }

    #[test]
    fn tui_cli_parses_attach_target() {
        let cli = Cli::try_parse_from(["codex-helper", "tui", "--claude", "--port", "3210"])
            .expect("parse tui attach command");

        let Some(Command::Tui {
            codex,
            claude,
            port,
        }) = cli.command
        else {
            panic!("expected tui command");
        };
        assert!(!codex);
        assert!(claude);
        assert_eq!(port, Some(3210));
    }

    #[test]
    fn daemon_cli_parses_supervise() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "daemon",
            "supervise",
            "--codex",
            "--port",
            "4211",
            "--max-restarts",
            "3",
        ])
        .expect("parse daemon supervise command");

        let Some(Command::Daemon {
            cmd:
                DaemonCommand::Supervise {
                    codex,
                    port,
                    max_restarts,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected daemon supervise command");
        };
        assert!(codex);
        assert_eq!(port, Some(4211));
        assert_eq!(max_restarts, 3);
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum PricingConfidence {
    Unknown,
    Partial,
    Estimated,
    Exact,
}

impl From<PricingConfidence> for codex_helper_core::pricing::CostConfidence {
    fn from(value: PricingConfidence) -> Self {
        match value {
            PricingConfidence::Unknown => Self::Unknown,
            PricingConfidence::Partial => Self::Partial,
            PricingConfidence::Estimated => Self::Estimated,
            PricingConfidence::Exact => Self::Exact,
        }
    }
}
