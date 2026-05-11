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

    /// Preview or write migration output for the v4 route graph schema
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
    /// Show the v4 route graph recipe for Codex or Claude
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
    },
    /// Patch routing fields directly
    Set {
        /// Routing policy
        #[arg(long, value_enum)]
        policy: Option<RoutingPolicy>,
        /// Manual-sticky target provider; implies manual-sticky when policy is omitted
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
    /// Pin routing to one provider
    Pin {
        provider: String,
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
