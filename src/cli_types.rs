use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use codex_helper_core::runtime_identity::ProviderEndpointKey;
use std::net::IpAddr;
use std::str::FromStr;

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
    /// Errors related to the canonical codex-helper config.toml
    Configuration(String),
    /// Errors while inspecting or explicitly patching Codex config.toml
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
            CliError::Configuration(msg) => write!(f, "Configuration error: {}", msg),
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
        /// Keep the proxy resident when the operator console exits
        #[arg(long)]
        resident: bool,
        /// Mark the resident child as supervisor-owned; internal use by `daemon supervise`
        #[arg(long, hide = true)]
        supervisor_managed: bool,
        /// Mark a resident proxy as desktop/tray-owned; intended for future desktop shells
        #[arg(long, hide = true)]
        desktop_managed: bool,
        /// Mark a resident proxy as system-service-owned; internal use by `service`
        #[arg(long, hide = true)]
        service_managed: bool,
    },
    /// Inspect or control a resident codex-helper proxy
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCommand,
    },
    /// Install and control the resident operating-system service
    Service {
        #[command(subcommand)]
        cmd: ServiceCommand,
    },
    /// Attach a TUI operator console to an already-running local resident proxy
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
    /// Select, inspect, and use local or remote relay targets
    Relay {
        #[command(subcommand)]
        cmd: RelayCommand,
    },
    /// Manage explicit Codex switch state
    Switch {
        #[command(subcommand)]
        cmd: SwitchCommand,
    },
    /// Manage codex-helper configuration
    Config {
        #[command(subcommand)]
        cmd: ConfigCommand,
    },
    /// Manage provider routing policy and runtime routing views
    Routing {
        #[command(subcommand)]
        cmd: RoutingCommand,
    },
    /// Manage the canonical route graph provider catalog
    Provider {
        #[command(subcommand)]
        cmd: ProviderCommand,
    },
    /// Manage installation-scoped native credentials
    Credential {
        #[command(subcommand)]
        cmd: CredentialCommand,
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
    /// Inspect the runtime's read-only operator usage projection
    Usage {
        /// Target Codex runtime (default if neither service flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude runtime
        #[arg(long)]
        claude: bool,
        /// Proxy port; defaults to 3211 for Codex and 3210 for Claude
        #[arg(long)]
        port: Option<u16>,
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
pub(crate) enum ServiceCommand {
    /// Install or update the operating-system service
    Install {
        /// Target Codex service (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude service
        #[arg(long)]
        claude: bool,
        /// Listen host for the resident proxy
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Proxy port; defaults to 3211 for Codex and 3210 for Claude
        #[arg(long)]
        port: Option<u16>,
        /// Install without starting immediately
        #[arg(long)]
        no_start: bool,
    },
    /// Uninstall the operating-system service
    Uninstall {
        /// Leave a currently running service alive until it exits
        #[arg(long)]
        keep_running: bool,
    },
    /// Start the installed service
    Start,
    /// Stop the installed service
    Stop,
    /// Restart the installed service
    Restart,
    /// Show installation and runtime status
    Status {
        /// Output status as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show service log locations
    Logs,
    /// Enter the legacy Windows Service Control Manager dispatcher
    #[command(hide = true)]
    Run {
        /// Target service name
        #[arg(long, default_value = "codex")]
        service_name: String,
        /// Listen host for the resident proxy
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Proxy port
        #[arg(long)]
        port: Option<u16>,
        /// Helper home captured at installation time
        #[arg(long)]
        helper_home: Option<std::path::PathBuf>,
        /// Codex or Claude home captured at installation time
        #[arg(long)]
        client_home: Option<std::path::PathBuf>,
    },
    /// Run a Windows per-user scheduled task
    #[command(hide = true)]
    TaskRun {
        /// Target service name
        #[arg(long, default_value = "codex")]
        service_name: String,
        /// Listen host for the resident proxy
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        /// Proxy port
        #[arg(long)]
        port: Option<u16>,
        /// Helper home captured at installation time
        #[arg(long)]
        helper_home: Option<std::path::PathBuf>,
        /// Codex or Claude home captured at installation time
        #[arg(long)]
        client_home: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum RelayCommand {
    /// Add or update a named remote relay target
    Add {
        /// Target name, for example "nas"
        name: String,
        /// Proxy base URL, for example http://nas.local:3211
        #[arg(long)]
        proxy_url: String,
        /// Admin API base URL. Required for remote targets; loopback targets derive it.
        #[arg(long)]
        admin_url: Option<String>,
        /// Environment variable that stores the admin token for this target
        #[arg(long)]
        admin_token_env: Option<String>,
        /// Target Codex service
        #[arg(long)]
        codex: bool,
        /// Target Claude service
        #[arg(long)]
        claude: bool,
    },
    /// List configured relay targets
    List,
    /// Show target reachability and current Codex switch state
    Status {
        /// Optional target name. Defaults to the current summary.
        target: Option<String>,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
    /// Explain how to disable an explicit Codex switch without changing client config
    Off,
    /// Use a relay target; starts local or attaches remote without changing client config
    Use {
        /// Target name, for example "local" or "nas"
        target: String,
        /// Do not open TUI when starting the built-in local target
        #[arg(long)]
        no_tui: bool,
        /// Attach to an existing runtime instead of starting a local proxy
        #[arg(long)]
        attach_only: bool,
    },
    /// Shorthand for `ch relay <target> [--no-tui] [--attach-only]`
    #[command(external_subcommand)]
    Target(Vec<String>),
}

#[derive(Subcommand, Debug)]
pub(crate) enum CodexCommand {
    /// Run validation-only Codex relay capability diagnostics
    #[command(name = "relay-capabilities")]
    Capabilities {
        /// Target canonical provider id; defaults to the current runtime target when omitted
        #[arg(long)]
        provider: Option<String>,
        /// Target canonical endpoint id; requires --provider
        #[arg(long)]
        endpoint: Option<String>,
        /// Requested model used for model-catalog capability interpretation
        #[arg(long)]
        model: Option<String>,
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
        /// Target canonical provider id; defaults to the current runtime target when omitted
        #[arg(long)]
        provider: Option<String>,
        /// Target canonical endpoint id; requires --provider
        #[arg(long)]
        endpoint: Option<String>,
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
        /// Match canonical provider id substring
        #[arg(long)]
        provider: Option<String>,
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
    /// Switch Codex config to a helper proxy
    On {
        /// Listen port for local proxy; defaults to 3211
        #[arg(long, conflicts_with = "base_url")]
        port: Option<u16>,
        /// Explicit helper proxy base URL
        #[arg(long, conflicts_with = "port")]
        base_url: Option<String>,
    },
    /// Restore the selector and helper stanza recorded by the explicit switch operation
    Off,
    /// Show the helper-owned Codex switch status
    Status,
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
    },
    /// Set retry policy to a curated profile (writes to ~/.codex-helper/config.*)
    #[command(name = "set-retry-profile")]
    SetRetryProfile {
        #[arg(value_enum)]
        profile: RetryProfile,
    },
    /// Preview or explicitly apply a legacy configuration migration.
    Migrate {
        /// Preview the migrated v6 TOML without writing any file (the default).
        #[arg(long, conflicts_with = "write")]
        dry_run: bool,
        /// Write the migrated v6 TOML after creating a source backup.
        #[arg(long, conflicts_with = "dry_run", requires = "yes")]
        write: bool,
        /// Confirm the replacement of the canonical configuration file.
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
    /// List providers in the canonical configuration
    List {
        /// Target Codex routing (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude routing
        #[arg(long)]
        claude: bool,
    },
    /// Preview routing from configuration only; runtime health is not queried
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
    /// Show providers in the source catalog
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
        /// Explicitly allow anonymous requests to a remote third-party upstream
        #[arg(long)]
        allow_anonymous: bool,
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
    /// Bind one provider authentication kind to a non-inline source
    #[command(group(
        ArgGroup::new("provider_auth_source")
            .required(true)
            .multiple(false)
            .args(["native", "secret_file", "environment"])
    ))]
    SetAuth {
        name: String,
        /// Authentication header kind to bind
        #[arg(long, value_enum)]
        kind: ProviderAuthKind,
        /// Bind to an installation-scoped native credential name
        #[arg(long, value_name = "CREDENTIAL")]
        native: Option<String>,
        /// Bind to an absolute mounted secret-file path
        #[arg(long = "secret-file", value_name = "PATH")]
        secret_file: Option<std::path::PathBuf>,
        /// Bind to an environment variable name
        #[arg(long, value_name = "ENV")]
        environment: Option<String>,
        /// Target Codex provider catalog (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude provider catalog
        #[arg(long)]
        claude: bool,
    },
    /// Clear one provider authentication kind without changing other settings
    ClearAuth {
        name: String,
        /// Authentication header kind to clear
        #[arg(long, value_enum)]
        kind: ProviderAuthKind,
        /// Target Codex provider catalog (default if neither flag is set)
        #[arg(long)]
        codex: bool,
        /// Target Claude provider catalog
        #[arg(long)]
        claude: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CredentialCommand {
    /// Create a new native credential and fail if it already exists
    Create {
        name: String,
        /// Read the credential from standard input instead of a masked TTY prompt
        #[arg(long)]
        stdin: bool,
    },
    /// Create or replace a native credential
    Set {
        name: String,
        /// Read the credential from standard input instead of a masked TTY prompt
        #[arg(long)]
        stdin: bool,
    },
    /// Import a native credential from an explicitly named source
    Import {
        name: String,
        /// Read the credential from this environment variable without modifying it
        #[arg(long, value_name = "ENV")]
        from_env: String,
    },
    /// Inspect one credential or all native credentials referenced by configuration
    Status {
        name: Option<String>,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Delete a native credential without rewriting provider configuration
    Delete {
        name: String,
        /// Confirm deletion without an interactive prompt
        #[arg(long)]
        yes: bool,
        /// Succeed when the credential is already absent
        #[arg(long)]
        if_exists: bool,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum ProviderAuthKind {
    Bearer,
    ApiKey,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum RoutingPolicy {
    ManualSticky,
    OrderedFailover,
    RoundRobin,
    TagPreferred,
}

impl From<RoutingPolicy> for codex_helper_core::config::RouteStrategy {
    fn from(value: RoutingPolicy) -> Self {
        match value {
            RoutingPolicy::ManualSticky => Self::ManualSticky,
            RoutingPolicy::OrderedFailover => Self::OrderedFailover,
            RoutingPolicy::RoundRobin => Self::RoundRobin,
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

impl From<RoutingExhaustedAction> for codex_helper_core::config::RouteExhaustedAction {
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
    /// Show daemon-owned remote quota pools and pacing without recalculating them locally
    Quota {
        /// Configured relay target whose canonical operator read model should be queried
        #[arg(long, default_value = "local")]
        target: String,
        /// Output the quota analytics DTO as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show recent requests from the runtime operator read model
    Tail {
        /// Maximum number of recent entries to print
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Print raw JSON lines instead of human-friendly format
        #[arg(long)]
        raw: bool,
    },
    /// Summarize token usage from the runtime operator read model
    Summary {
        /// Maximum number of summary rows to show (sorted by total_tokens desc)
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Group summary rows by provider endpoint, provider, model, or session
        #[arg(long, value_enum, default_value_t = UsageSummaryBy::ProviderEndpoint)]
        by: UsageSummaryBy,
    },
    /// Find matching requests in the runtime operator read model
    Find {
        /// Maximum number of matching entries to print, newest first
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Match a redacted operator session key substring
        #[arg(long)]
        session: Option<String>,
        /// Match model id substring
        #[arg(long)]
        model: Option<String>,
        /// Match an exact canonical service/provider/endpoint key
        #[arg(long)]
        provider_endpoint: Option<ProviderEndpointArg>,
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
    /// Export a sanitized request chain by trace, request id, or session
    Chain {
        /// Maximum number of matching requests to export
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Match an exact trace id
        #[arg(long)]
        trace_id: Option<String>,
        /// Match an exact request id
        #[arg(long)]
        request_id: Option<u64>,
        /// Match session id substring
        #[arg(long)]
        session: Option<String>,
        /// Print the sanitized chain as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "kebab-case")]
pub enum UsageSummaryBy {
    ProviderEndpoint,
    Provider,
    Model,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEndpointArg(ProviderEndpointKey);

impl ProviderEndpointArg {
    pub fn into_key(self) -> ProviderEndpointKey {
        self.0
    }

    #[cfg(test)]
    fn stable_key(&self) -> String {
        self.0.stable_key()
    }
}

impl FromStr for ProviderEndpointArg {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut parts = value.split('/');
        let service_name = parts.next().unwrap_or_default().trim();
        let provider_id = parts.next().unwrap_or_default().trim();
        let endpoint_id = parts.next().unwrap_or_default().trim();
        if service_name.is_empty()
            || provider_id.is_empty()
            || endpoint_id.is_empty()
            || parts.next().is_some()
        {
            return Err(
                "provider endpoint must use the exact service/provider/endpoint form".to_string(),
            );
        }
        Ok(Self(ProviderEndpointKey::new(
            service_name,
            provider_id,
            endpoint_id,
        )))
    }
}

#[derive(Subcommand, Debug)]
pub enum PricingCommand {
    /// Print the local pricing override path
    Path,
    /// Show BaseLLM LKG, last-check, effective catalog, and manual override status
    Status {
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
    /// Force an immediate validated BaseLLM LKG refresh without changing manual overrides
    ForceRefresh {
        /// URL returning BaseLLM all.json data
        #[arg(
            long,
            default_value = "https://basellm.github.io/llm-metadata/api/all.json"
        )]
        url: String,
        /// Approve the exact economic-change candidate from the last quarantine
        #[arg(long)]
        approve_economic_changes: bool,
        /// Output JSON instead of text
        #[arg(long)]
        json: bool,
    },
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
        /// Filter rows by canonical pricing provider
        #[arg(long)]
        provider: Option<String>,
    },
    /// Add or replace a local model price override
    Set {
        model_id: String,
        /// Canonical pricing provider namespace
        #[arg(long, default_value = "openai")]
        provider: String,
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
    Remove {
        model_id: String,
        /// Canonical pricing provider namespace
        #[arg(long, default_value = "openai")]
        provider: String,
    },
    /// Pull a remote pricing catalog JSON into local overrides
    Sync {
        /// URL returning a ModelPriceCatalogSnapshot JSON payload
        url: String,
        /// Import only rows matching these model ids or aliases
        #[arg(long = "model")]
        models: Vec<String>,
        /// Import only rows from this canonical provider
        #[arg(long)]
        provider: Option<String>,
        /// Replace local overrides instead of merging into them
        #[arg(long)]
        replace: bool,
        /// Show what would be imported without writing pricing_overrides.toml
        #[arg(long)]
        dry_run: bool,
    },
    /// Explicitly import BaseLLM pricing rows into manual overrides
    #[command(name = "import-basellm", visible_alias = "sync-basellm")]
    ImportBasellm {
        /// URL returning basellm all.json data
        #[arg(
            long,
            default_value = "https://basellm.github.io/llm-metadata/api/all.json"
        )]
        url: String,
        /// Import only rows matching these model ids or aliases
        #[arg(long = "model")]
        models: Vec<String>,
        /// BaseLLM provider namespace to import for this Codex catalog
        #[arg(long, default_value = "openai")]
        provider: String,
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
    fn config_init_rejects_removed_no_import_flag() {
        assert!(Cli::try_parse_from(["codex-helper", "config", "init", "--no-import"]).is_err());
    }

    #[test]
    fn config_rejects_removed_codex_import_commands() {
        for command in ["import-from-codex", "overwrite-from-codex"] {
            assert!(Cli::try_parse_from(["codex-helper", "config", command]).is_err());
        }
    }

    #[test]
    fn config_migrate_requires_explicit_write_confirmation() {
        assert!(Cli::try_parse_from(["codex-helper", "config", "migrate"]).is_ok());
        assert!(Cli::try_parse_from(["codex-helper", "config", "migrate", "--dry-run"]).is_ok());
        assert!(Cli::try_parse_from(["codex-helper", "config", "migrate", "--write"]).is_err());
        assert!(
            Cli::try_parse_from([
                "codex-helper",
                "config",
                "migrate",
                "--write",
                "--yes",
                "--dry-run",
            ])
            .is_err()
        );
    }

    #[test]
    fn switch_on_accepts_explicit_base_url() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "switch",
            "on",
            "--base-url",
            "https://relay.example/v1",
        ])
        .expect("parse explicit base URL");
        let Some(Command::Switch {
            cmd:
                SwitchCommand::On {
                    port: None,
                    base_url: Some(base_url),
                },
        }) = cli.command
        else {
            panic!("expected explicit switch-on base URL");
        };
        assert_eq!(base_url, "https://relay.example/v1");
    }

    #[test]
    fn switch_rejects_removed_mutation_surfaces() {
        for args in [
            vec!["codex-helper", "switch", "on", "--preset", "default"],
            vec!["codex-helper", "switch", "on", "--mode", "official-relay"],
            vec!["codex-helper", "switch", "on", "--compaction", "local"],
            vec!["codex-helper", "switch", "on", "--responses-websocket"],
            vec!["codex-helper", "switch", "on", "--claude"],
            vec!["codex-helper", "switch", "on", "--codex"],
            vec!["codex-helper", "switch", "off", "--claude"],
            vec!["codex-helper", "switch", "off", "--codex"],
            vec!["codex-helper", "switch", "status", "--codex"],
            vec!["codex-helper", "switch", "status", "--claude"],
            vec!["codex-helper", "switch", "remote-control", "enable"],
            vec!["codex-helper", "switch", "remote-control", "status"],
            vec!["codex-helper", "switch", "remote-control", "check-logs"],
        ] {
            assert!(
                Cli::try_parse_from(args.clone()).is_err(),
                "removed switch surface should be rejected: {args:?}"
            );
        }
    }

    #[test]
    fn switch_rejects_port_and_base_url_together() {
        assert!(
            Cli::try_parse_from([
                "codex-helper",
                "switch",
                "on",
                "--port",
                "4321",
                "--base-url",
                "https://relay.example/v1",
            ])
            .is_err()
        );
    }

    #[test]
    fn codex_relay_cli_parses_capability_diagnostics() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "codex",
            "relay-capabilities",
            "--model",
            "gpt-5.5",
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
        assert_eq!(provider.as_deref(), Some("ciii"));
        assert_eq!(endpoint.as_deref(), Some("default"));
        assert!(json);
    }

    #[test]
    fn codex_relay_cli_rejects_removed_capability_assumptions() {
        for removed in [
            vec!["--preset", "official-imagegen"],
            vec!["--mode", "official-imagegen-bridge"],
            vec!["--compaction", "local"],
        ] {
            let mut args = vec!["codex-helper", "codex", "relay-capabilities"];
            args.extend(removed);
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn codex_relay_cli_rejects_legacy_target_flags() {
        for args in [
            vec![
                "codex-helper",
                "codex",
                "relay-capabilities",
                "--station",
                "legacy-station",
            ],
            vec![
                "codex-helper",
                "codex",
                "relay-live-smoke",
                "--acknowledgement",
                "run-live-codex-relay-smoke",
                "--model",
                "gpt-5.5",
                "--upstream-index",
                "1",
            ],
            vec![
                "codex-helper",
                "codex",
                "relay-evidence",
                "--station",
                "legacy-station",
            ],
        ] {
            let error = Cli::try_parse_from(args).expect_err("legacy target flag should fail");
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        }
    }

    #[test]
    fn usage_cli_uses_canonical_provider_endpoint_identity() {
        let summary = Cli::try_parse_from([
            "codex-helper",
            "usage",
            "--claude",
            "--port",
            "4210",
            "summary",
            "--by",
            "provider-endpoint",
        ])
        .expect("parse provider-endpoint summary");
        let Some(Command::Usage {
            claude, port, cmd, ..
        }) = summary.command
        else {
            panic!("expected usage summary command");
        };
        assert!(claude);
        assert_eq!(port, Some(4210));
        assert!(matches!(
            cmd,
            UsageCommand::Summary {
                by: UsageSummaryBy::ProviderEndpoint,
                ..
            }
        ));

        let find = Cli::try_parse_from([
            "codex-helper",
            "usage",
            "find",
            "--provider-endpoint",
            "codex/sol/responses",
        ])
        .expect("parse provider endpoint filter");
        let Some(Command::Usage {
            cmd: UsageCommand::Find {
                provider_endpoint, ..
            },
            ..
        }) = find.command
        else {
            panic!("expected usage find command");
        };
        assert_eq!(
            provider_endpoint
                .as_ref()
                .map(ProviderEndpointArg::stable_key),
            Some("codex/sol/responses".to_string())
        );
    }

    #[test]
    fn usage_cli_rejects_removed_station_identity() {
        for args in [
            vec!["codex-helper", "usage", "summary", "--by", "station"],
            vec![
                "codex-helper",
                "usage",
                "find",
                "--station",
                "legacy-station",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
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
            "--provider",
            "input",
            "--limit",
            "5",
        ])
        .expect("parse codex relay evidence");

        let Some(Command::Codex {
            cmd:
                CodexCommand::Evidence {
                    kind,
                    provider,
                    limit,
                    ..
                },
        }) = cli.command
        else {
            panic!("expected codex relay evidence command");
        };
        assert_eq!(kind, Some(CodexRelayEvidenceKindArg::LiveSmoke));
        assert_eq!(provider.as_deref(), Some("input"));
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
    fn serve_cli_parses_hidden_service_managed_flag() {
        let cli = Cli::try_parse_from(["codex-helper", "serve", "--codex", "--service-managed"])
            .expect("parse service-managed serve command");

        let Some(Command::Serve {
            codex,
            resident,
            service_managed,
            ..
        }) = cli.command
        else {
            panic!("expected serve command");
        };
        assert!(codex);
        assert!(!resident);
        assert!(service_managed);
    }

    #[test]
    fn service_cli_parses_install_and_status_contract() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "service",
            "install",
            "--claude",
            "--host",
            "127.0.0.2",
            "--port",
            "4210",
            "--no-start",
        ])
        .expect("parse service install command");

        let Some(Command::Service {
            cmd:
                ServiceCommand::Install {
                    codex,
                    claude,
                    host,
                    port,
                    no_start,
                },
        }) = cli.command
        else {
            panic!("expected service install command");
        };
        assert!(!codex);
        assert!(claude);
        assert_eq!(host, IpAddr::from([127, 0, 0, 2]));
        assert_eq!(port, Some(4210));
        assert!(no_start);

        let status = Cli::try_parse_from(["codex-helper", "service", "status", "--json"])
            .expect("parse service status command");
        assert!(matches!(
            status.command,
            Some(Command::Service {
                cmd: ServiceCommand::Status { json: true }
            })
        ));
    }

    #[test]
    fn service_cli_internal_run_carries_installed_runtime_identity() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "service",
            "run",
            "--service-name",
            "claude",
            "--host",
            "127.0.0.3",
            "--port",
            "4210",
            "--helper-home",
            "/tmp/helper-home",
            "--client-home",
            "/tmp/client-home",
        ])
        .expect("parse internal service run command");

        let Some(Command::Service {
            cmd:
                ServiceCommand::Run {
                    service_name,
                    host,
                    port,
                    helper_home,
                    client_home,
                },
        }) = cli.command
        else {
            panic!("expected internal service run command");
        };
        assert_eq!(service_name, "claude");
        assert_eq!(host, IpAddr::from([127, 0, 0, 3]));
        assert_eq!(port, Some(4210));
        assert_eq!(
            helper_home,
            Some(std::path::PathBuf::from("/tmp/helper-home"))
        );
        assert_eq!(
            client_home,
            Some(std::path::PathBuf::from("/tmp/client-home"))
        );
    }

    #[test]
    fn service_cli_task_run_carries_installed_runtime_identity() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "service",
            "task-run",
            "--service-name",
            "codex",
            "--host",
            "127.0.0.1",
            "--port",
            "3211",
            "--helper-home",
            r"C:\Users\test\.codex-helper",
            "--client-home",
            r"C:\Users\test\.codex",
        ])
        .expect("parse internal scheduled-task run command");

        let Some(Command::Service {
            cmd:
                ServiceCommand::TaskRun {
                    service_name,
                    host,
                    port,
                    helper_home,
                    client_home,
                },
        }) = cli.command
        else {
            panic!("expected internal scheduled-task run command");
        };
        assert_eq!(service_name, "codex");
        assert_eq!(host, IpAddr::from([127, 0, 0, 1]));
        assert_eq!(port, Some(3211));
        assert_eq!(
            helper_home,
            Some(std::path::PathBuf::from(r"C:\Users\test\.codex-helper"))
        );
        assert_eq!(
            client_home,
            Some(std::path::PathBuf::from(r"C:\Users\test\.codex"))
        );
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
    fn daemon_cli_rejects_http_stop_but_service_stop_remains_explicit() {
        assert!(Cli::try_parse_from(["codex-helper", "daemon", "stop", "--codex"]).is_err());

        let service_stop = Cli::try_parse_from(["codex-helper", "service", "stop"])
            .expect("parse local service stop command");
        assert!(matches!(
            service_stop.command,
            Some(Command::Service {
                cmd: ServiceCommand::Stop
            })
        ));
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
    fn relay_cli_parses_target_shorthand() {
        let cli = Cli::try_parse_from(["ch", "relay", "nas", "--attach-only"])
            .expect("parse relay target shorthand");

        let Some(Command::Relay {
            cmd: RelayCommand::Target(args),
        }) = cli.command
        else {
            panic!("expected relay target shorthand");
        };
        assert_eq!(args, vec!["nas".to_string(), "--attach-only".to_string()]);
    }

    #[test]
    fn relay_cli_parses_use_target() {
        let cli = Cli::try_parse_from(["ch", "relay", "use", "nas", "--no-tui"])
            .expect("parse relay use command");

        let Some(Command::Relay {
            cmd:
                RelayCommand::Use {
                    target,
                    no_tui,
                    attach_only,
                },
        }) = cli.command
        else {
            panic!("expected relay use command");
        };
        assert_eq!(target, "nas");
        assert!(no_tui);
        assert!(!attach_only);
    }

    #[test]
    fn relay_cli_rejects_removed_client_preset() {
        for removed in [
            vec!["--preset", "chatgpt-bridge"],
            vec!["--mode", "official-relay"],
            vec!["--responses-websocket"],
        ] {
            let mut args = vec![
                "ch",
                "relay",
                "add",
                "nas",
                "--proxy-url",
                "http://nas.local:3211",
                "--admin-token-env",
                "NAS_ADMIN_TOKEN",
            ];
            args.extend(removed);
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn relay_cli_off_is_read_only_and_rejects_old_client_flags() {
        let cli = Cli::try_parse_from(["ch", "relay", "off"]).expect("parse relay off");
        assert!(matches!(
            cli.command,
            Some(Command::Relay {
                cmd: RelayCommand::Off
            })
        ));
        assert!(Cli::try_parse_from(["ch", "relay", "off", "--codex"]).is_err());
        assert!(Cli::try_parse_from(["ch", "relay", "off", "--claude"]).is_err());
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

    #[test]
    fn pricing_sync_basellm_defaults_to_openai_provider() {
        let cli = Cli::try_parse_from(["codex-helper", "pricing", "sync-basellm", "--dry-run"])
            .expect("parse pricing sync-basellm");

        let Some(Command::Pricing {
            cmd: PricingCommand::ImportBasellm {
                provider, dry_run, ..
            },
        }) = cli.command
        else {
            panic!("expected pricing sync-basellm command");
        };
        assert_eq!(provider, "openai");
        assert!(dry_run);
    }

    #[test]
    fn pricing_status_and_force_refresh_parse_operator_flags() {
        let status = Cli::try_parse_from(["codex-helper", "pricing", "status", "--json"])
            .expect("parse pricing status");
        assert!(matches!(
            status.command,
            Some(Command::Pricing {
                cmd: PricingCommand::Status { json: true }
            })
        ));

        let refresh = Cli::try_parse_from([
            "codex-helper",
            "pricing",
            "force-refresh",
            "--json",
            "--approve-economic-changes",
        ])
        .expect("parse pricing force refresh");
        assert!(matches!(
            refresh.command,
            Some(Command::Pricing {
                cmd: PricingCommand::ForceRefresh {
                    json: true,
                    approve_economic_changes: true,
                    ..
                }
            })
        ));
    }

    #[test]
    fn pricing_import_basellm_is_primary_name_with_legacy_alias() {
        for command in ["import-basellm", "sync-basellm"] {
            let cli = Cli::try_parse_from(["codex-helper", "pricing", command, "--dry-run"])
                .expect("parse BaseLLM manual import");
            assert!(matches!(
                cli.command,
                Some(Command::Pricing {
                    cmd: PricingCommand::ImportBasellm { dry_run: true, .. }
                })
            ));
        }
    }

    #[test]
    fn usage_quota_parses_target_and_json_passthrough() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "usage",
            "quota",
            "--target",
            "nas",
            "--json",
        ])
        .expect("parse usage quota");

        let Some(Command::Usage {
            cmd: UsageCommand::Quota { target, json },
            ..
        }) = cli.command
        else {
            panic!("expected usage quota command");
        };
        assert_eq!(target, "nas");
        assert!(json);
    }

    #[test]
    fn provider_add_parses_explicit_anonymous_opt_in() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "provider",
            "add",
            "relay",
            "--base-url",
            "https://relay.example/v1",
            "--allow-anonymous",
        ])
        .expect("parse provider anonymous opt-in");

        assert!(matches!(
            cli.command,
            Some(Command::Provider {
                cmd: ProviderCommand::Add {
                    allow_anonymous: true,
                    ..
                }
            })
        ));
    }

    #[test]
    fn credential_commands_parse_only_explicit_safe_input_sources() {
        let create = Cli::try_parse_from([
            "codex-helper",
            "credential",
            "create",
            "relay.primary",
            "--stdin",
        ])
        .expect("parse credential create");
        assert!(matches!(
            create.command,
            Some(Command::Credential {
                cmd: CredentialCommand::Create { stdin: true, .. }
            })
        ));

        let set = Cli::try_parse_from(["codex-helper", "credential", "set", "relay.primary"])
            .expect("parse credential set");
        assert!(matches!(
            set.command,
            Some(Command::Credential {
                cmd: CredentialCommand::Set { stdin: false, .. }
            })
        ));

        let import = Cli::try_parse_from([
            "codex-helper",
            "credential",
            "import",
            "relay.primary",
            "--from-env",
            "RELAY_TOKEN",
        ])
        .expect("parse credential import");
        assert!(matches!(
            import.command,
            Some(Command::Credential {
                cmd: CredentialCommand::Import { from_env, .. }
            }) if from_env == "RELAY_TOKEN"
        ));

        let status = Cli::try_parse_from([
            "codex-helper",
            "credential",
            "status",
            "relay.primary",
            "--json",
        ])
        .expect("parse credential status");
        assert!(matches!(
            status.command,
            Some(Command::Credential {
                cmd: CredentialCommand::Status {
                    name: Some(_),
                    json: true,
                }
            })
        ));

        let delete = Cli::try_parse_from([
            "codex-helper",
            "credential",
            "delete",
            "relay.primary",
            "--yes",
            "--if-exists",
        ])
        .expect("parse credential delete");
        assert!(matches!(
            delete.command,
            Some(Command::Credential {
                cmd: CredentialCommand::Delete {
                    yes: true,
                    if_exists: true,
                    ..
                }
            })
        ));
    }

    #[test]
    fn credential_commands_reject_argv_secret_values_and_conflicting_sources() {
        for args in [
            vec![
                "codex-helper",
                "credential",
                "create",
                "relay.primary",
                "--value",
                "secret-canary",
            ],
            vec![
                "codex-helper",
                "credential",
                "create",
                "relay.primary",
                "secret-canary",
            ],
            vec![
                "codex-helper",
                "credential",
                "create",
                "relay.primary",
                "--stdin",
                "--from-env",
                "RELAY_TOKEN",
            ],
            vec![
                "codex-helper",
                "credential",
                "import",
                "relay.primary",
                "--from-env",
                "RELAY_TOKEN",
                "--stdin",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn provider_auth_commands_require_one_kind_and_one_reference_source() {
        let set = Cli::try_parse_from([
            "codex-helper",
            "provider",
            "set-auth",
            "relay",
            "--kind",
            "api-key",
            "--native",
            "relay.primary",
            "--claude",
        ])
        .expect("parse provider set-auth");
        assert!(matches!(
            set.command,
            Some(Command::Provider {
                cmd: ProviderCommand::SetAuth {
                    kind: ProviderAuthKind::ApiKey,
                    native: Some(_),
                    claude: true,
                    ..
                }
            })
        ));

        for args in [
            vec![
                "codex-helper",
                "provider",
                "set-auth",
                "relay",
                "--kind",
                "bearer",
                "--secret-file",
                "/run/secrets/relay",
                "--codex",
            ],
            vec![
                "codex-helper",
                "provider",
                "set-auth",
                "relay",
                "--kind",
                "api-key",
                "--environment",
                "RELAY_API_KEY",
            ],
        ] {
            Cli::try_parse_from(args).expect("parse provider reference source");
        }

        let clear = Cli::try_parse_from([
            "codex-helper",
            "provider",
            "clear-auth",
            "relay",
            "--kind",
            "bearer",
        ])
        .expect("parse provider clear-auth");
        assert!(matches!(
            clear.command,
            Some(Command::Provider {
                cmd: ProviderCommand::ClearAuth {
                    kind: ProviderAuthKind::Bearer,
                    ..
                }
            })
        ));

        for args in [
            vec![
                "codex-helper",
                "provider",
                "set-auth",
                "relay",
                "--native",
                "relay.primary",
            ],
            vec![
                "codex-helper",
                "provider",
                "set-auth",
                "relay",
                "--kind",
                "bearer",
            ],
            vec![
                "codex-helper",
                "provider",
                "set-auth",
                "relay",
                "--kind",
                "bearer",
                "--native",
                "relay.primary",
                "--environment",
                "RELAY_TOKEN",
            ],
            vec![
                "codex-helper",
                "provider",
                "set-auth",
                "relay",
                "--kind",
                "bearer",
                "--kind",
                "api-key",
                "--native",
                "relay.primary",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn pricing_set_accepts_explicit_provider_namespace() {
        let cli = Cli::try_parse_from([
            "codex-helper",
            "pricing",
            "set",
            "shared-model",
            "--provider",
            "routing-run",
            "--input-per-1m-usd",
            "1",
            "--output-per-1m-usd",
            "2",
        ])
        .expect("parse provider-scoped pricing set");

        let Some(Command::Pricing {
            cmd: PricingCommand::Set {
                provider, model_id, ..
            },
        }) = cli.command
        else {
            panic!("expected pricing set command");
        };
        assert_eq!(provider, "routing-run");
        assert_eq!(model_id, "shared-model");
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
