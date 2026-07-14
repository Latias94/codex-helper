use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::path::PathBuf;

use crate::client_config::codex_home;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::fs;
use toml::Value as TomlValue;

pub use crate::client_config::{
    claude_settings_backup_path, claude_settings_path, codex_config_path,
};
pub use crate::fleet::registry::{FleetNodeConfig, FleetRegistryConfig};

#[path = "config_storage.rs"]
mod storage_impl;

#[path = "config_legacy_json.rs"]
mod legacy_json_impl;

#[path = "config_retry.rs"]
mod retry_impl;

#[path = "config_profiles.rs"]
mod profiles_impl;

#[path = "helper_config.rs"]
mod helper_config_impl;

pub use helper_config_impl::{effective_routing, resolved_provider_order, validate_helper_config};
pub(crate) use profiles_impl::validate_service_profile_catalog;
pub use profiles_impl::{ServiceControlProfile, resolve_service_profile_from_catalog};
pub use retry_impl::{
    ReasoningGuardAction, ReasoningGuardConfig, ReasoningGuardRetryExhaustedAction,
    ReasoningGuardStreamMode, ResolvedReasoningGuardConfig, ResolvedRetryConfig,
    ResolvedRetryLayerConfig, RetryConfig, RetryLayerConfig, RetryProfileName, RetryStrategy,
};
pub use storage_impl::{
    ConfigInitOutcome, LoadedConfig, config_file_path, init_config_toml,
    init_config_toml_with_outcome, load_config, load_config_with_source, save_helper_config,
};

pub mod storage {
    pub use super::storage_impl::{
        ConfigInitOutcome, LoadedConfig, config_file_path, init_config_toml,
        init_config_toml_with_outcome, load_config, load_config_with_source, save_helper_config,
    };
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpstreamAuth {
    /// Bearer token, e.g. OpenAI style
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    /// Environment variable name for bearer token (preferred over storing secrets on disk)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token_env: Option<String>,
    /// Optional API key header for some providers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Environment variable name for API key header value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

impl UpstreamAuth {
    pub fn with_overrides(&self, overrides: &Self) -> Self {
        Self {
            auth_token: overrides
                .auth_token
                .clone()
                .or_else(|| self.auth_token.clone()),
            auth_token_env: overrides
                .auth_token_env
                .clone()
                .or_else(|| self.auth_token_env.clone()),
            api_key: overrides.api_key.clone().or_else(|| self.api_key.clone()),
            api_key_env: overrides
                .api_key_env
                .clone()
                .or_else(|| self.api_key_env.clone()),
        }
    }

    pub fn resolve_auth_token(&self) -> Option<String> {
        if let Some(token) = self.auth_token.as_deref()
            && !token.trim().is_empty()
        {
            return Some(token.to_string());
        }
        if let Some(env_name) = self.auth_token_env.as_deref()
            && let Ok(v) = env::var(env_name)
            && !v.trim().is_empty()
        {
            return Some(v);
        }
        None
    }

    pub fn resolve_api_key(&self) -> Option<String> {
        if let Some(key) = self.api_key.as_deref()
            && !key.trim().is_empty()
        {
            return Some(key.to_string());
        }
        if let Some(env_name) = self.api_key_env.as_deref()
            && let Ok(v) = env::var(env_name)
            && !v.trim().is_empty()
        {
            return Some(v);
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub base_url: String,
    #[serde(default)]
    pub auth: UpstreamAuth,
    /// Optional free-form metadata, e.g. region / label
    #[serde(default)]
    pub tags: HashMap<String, String>,
    /// Optional model whitelist for this upstream (exact or wildcard patterns like `gpt-*`).
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        alias = "supportedModels"
    )]
    pub supported_models: HashMap<String, bool>,
    /// Optional model mapping: external model name -> upstream-specific model name (supports wildcards).
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        alias = "modelMapping"
    )]
    pub model_mapping: HashMap<String, String>,
}

fn default_service_config_enabled() -> bool {
    true
}

fn is_default_service_config_enabled(value: &bool) -> bool {
    *value == default_service_config_enabled()
}

fn default_provider_endpoint_priority() -> u32 {
    0
}

fn is_default_provider_endpoint_priority(value: &u32) -> bool {
    *value == default_provider_endpoint_priority()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, Hash)]
pub struct ProviderConcurrencyLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_group: Option<String>,
}

fn is_default_provider_concurrency_limits(value: &ProviderConcurrencyLimits) -> bool {
    value == &ProviderConcurrencyLimits::default()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyPolicyConfig {
    /// Only notify when proxy duration_ms is >= this threshold.
    pub min_duration_ms: u64,
    /// At most one notification per global_cooldown_ms.
    pub global_cooldown_ms: u64,
    /// Events within this window will be merged into one notification.
    pub merge_window_ms: u64,
    /// Suppress notifications for the same thread-id within this cooldown.
    pub per_thread_cooldown_ms: u64,
    /// How far back to search typed operator requests when matching a thread-id.
    pub recent_search_window_ms: u64,
    /// Timeout for fetching the typed operator read model.
    pub recent_endpoint_timeout_ms: u64,
}

impl Default for NotifyPolicyConfig {
    fn default() -> Self {
        Self {
            min_duration_ms: 60_000,
            global_cooldown_ms: 60_000,
            merge_window_ms: 10_000,
            per_thread_cooldown_ms: 180_000,
            recent_search_window_ms: 5 * 60_000,
            recent_endpoint_timeout_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotifySystemConfig {
    /// Whether to show system notifications (toasts). Default: false.
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotifyExecConfig {
    /// Enable executing an external command for each aggregated notification.
    pub enabled: bool,
    /// Command to execute; the aggregated JSON is written to stdin.
    /// Example: ["python", "my_script.py"].
    #[serde(default)]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotifyConfig {
    /// Whether notify processing is enabled at all (system toast and exec are both disabled by default).
    pub enabled: bool,
    #[serde(default)]
    pub policy: NotifyPolicyConfig,
    #[serde(default)]
    pub system: NotifySystemConfig,
    #[serde(default)]
    pub exec: NotifyExecConfig,
}

fn default_service_status_refresh_interval_secs() -> u64 {
    60
}

fn default_service_status_timeout_ms() -> u64 {
    3_000
}

fn default_service_status_high_latency_ms() -> u64 {
    3_000
}

fn default_service_status_history_cells() -> usize {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ServiceStatusConfig {
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub enabled: bool,
    #[serde(
        default = "default_service_status_refresh_interval_secs",
        skip_serializing_if = "is_default_service_status_refresh_interval_secs"
    )]
    pub refresh_interval_secs: u64,
    #[serde(
        default = "default_service_status_timeout_ms",
        skip_serializing_if = "is_default_service_status_timeout_ms"
    )]
    pub timeout_ms: u64,
    #[serde(
        default = "default_service_status_high_latency_ms",
        skip_serializing_if = "is_default_service_status_high_latency_ms"
    )]
    pub high_latency_ms: u64,
    #[serde(
        default = "default_service_status_history_cells",
        skip_serializing_if = "is_default_service_status_history_cells"
    )]
    pub history_cells: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<ServiceStatusProbeConfig>,
}

impl Default for ServiceStatusConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            refresh_interval_secs: default_service_status_refresh_interval_secs(),
            timeout_ms: default_service_status_timeout_ms(),
            high_latency_ms: default_service_status_high_latency_ms(),
            history_cells: default_service_status_history_cells(),
            probes: Vec::new(),
        }
    }
}

impl ServiceStatusConfig {
    pub fn has_probes(&self) -> bool {
        self.probes.iter().any(|probe| {
            probe
                .provider
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || probe
                    .url
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
        })
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.has_probes()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ServiceStatusProbeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

fn is_default_service_status_config(value: &ServiceStatusConfig) -> bool {
    value == &ServiceStatusConfig::default()
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

fn is_default_service_status_refresh_interval_secs(value: &u64) -> bool {
    *value == default_service_status_refresh_interval_secs()
}

fn is_default_service_status_timeout_ms(value: &u64) -> bool {
    *value == default_service_status_timeout_ms()
}

fn is_default_service_status_high_latency_ms(value: &u64) -> bool {
    *value == default_service_status_high_latency_ms()
}

fn is_default_service_status_history_cells(value: &usize) -> bool {
    *value == default_service_status_history_cells()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct RelayTargetConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<ServiceKind>,
    pub proxy_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_token_env: Option<String>,
}

pub const CURRENT_CONFIG_VERSION: u32 = 5;

pub fn is_supported_config_version(version: u32) -> bool {
    version == CURRENT_CONFIG_VERSION
}

fn default_config_version() -> u32 {
    CURRENT_CONFIG_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperConfig {
    #[serde(default = "default_config_version")]
    pub version: u32,
    #[serde(default)]
    pub codex: ServiceRouteConfig,
    #[serde(default)]
    pub claude: ServiceRouteConfig,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub notify: NotifyConfig,
    #[serde(default)]
    pub default_service: Option<ServiceKind>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub relay_targets: BTreeMap<String, RelayTargetConfig>,
    #[serde(default, skip_serializing_if = "FleetRegistryConfig::is_empty")]
    pub fleet: FleetRegistryConfig,
    #[serde(default)]
    pub ui: UiConfig,
}

impl Default for HelperConfig {
    fn default() -> Self {
        Self {
            version: default_config_version(),
            codex: ServiceRouteConfig::default(),
            claude: ServiceRouteConfig::default(),
            retry: RetryConfig::default(),
            notify: NotifyConfig::default(),
            default_service: None,
            relay_targets: BTreeMap::new(),
            fleet: FleetRegistryConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceRouteConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ServiceControlProfile>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RouteGraphConfig>,
}

impl ServiceRouteConfig {
    pub fn ensure_routing_mut(&mut self) -> &mut RouteGraphConfig {
        self.routing.get_or_insert_with(RouteGraphConfig::default)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(
        default = "default_service_config_enabled",
        skip_serializing_if = "is_default_service_config_enabled"
    )]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_domain: Option<String>,
    #[serde(default, skip_serializing_if = "is_default_upstream_auth")]
    pub auth: UpstreamAuth,
    #[serde(default, flatten)]
    pub inline_auth: UpstreamAuth,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        alias = "supportedModels"
    )]
    pub supported_models: BTreeMap<String, bool>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        alias = "modelMapping"
    )]
    pub model_mapping: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "is_default_provider_concurrency_limits"
    )]
    pub limits: ProviderConcurrencyLimits,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub endpoints: BTreeMap<String, ProviderEndpointConfig>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            alias: None,
            enabled: default_service_config_enabled(),
            base_url: None,
            continuity_domain: None,
            auth: UpstreamAuth::default(),
            inline_auth: UpstreamAuth::default(),
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            limits: ProviderConcurrencyLimits::default(),
            endpoints: BTreeMap::new(),
        }
    }
}

impl ProviderConfig {
    pub fn effective_auth(&self) -> UpstreamAuth {
        self.auth.with_overrides(&self.inline_auth)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEndpointConfig {
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_domain: Option<String>,
    #[serde(
        default = "default_service_config_enabled",
        skip_serializing_if = "is_default_service_config_enabled"
    )]
    pub enabled: bool,
    #[serde(
        default = "default_provider_endpoint_priority",
        skip_serializing_if = "is_default_provider_endpoint_priority"
    )]
    pub priority: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        alias = "supportedModels"
    )]
    pub supported_models: BTreeMap<String, bool>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        alias = "modelMapping"
    )]
    pub model_mapping: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "is_default_provider_concurrency_limits"
    )]
    pub limits: ProviderConcurrencyLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteGraphConfig {
    #[serde(default = "default_route_entry")]
    pub entry: String,
    #[serde(default = "default_route_affinity_policy")]
    pub affinity_policy: RouteAffinityPolicy,
    #[serde(default, skip_serializing_if = "SchedulingPreset::is_default")]
    pub scheduling_preset: SchedulingPreset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reprobe_preferred_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routes: BTreeMap<String, RouteNodeConfig>,
}

impl Default for RouteGraphConfig {
    fn default() -> Self {
        Self {
            entry: default_route_entry(),
            affinity_policy: default_route_affinity_policy(),
            scheduling_preset: SchedulingPreset::default(),
            fallback_ttl_ms: None,
            reprobe_preferred_after_ms: None,
            routes: BTreeMap::new(),
        }
    }
}

impl RouteGraphConfig {
    pub fn ordered_failover(children: Vec<String>) -> Self {
        Self::single_entry_node(RouteNodeConfig {
            strategy: RouteStrategy::OrderedFailover,
            children,
            ..RouteNodeConfig::default()
        })
    }

    pub fn round_robin(children: Vec<String>) -> Self {
        Self::single_entry_node(RouteNodeConfig {
            strategy: RouteStrategy::RoundRobin,
            children,
            ..RouteNodeConfig::default()
        })
    }

    pub fn manual_sticky(target: String, children: Vec<String>) -> Self {
        Self::single_entry_node(RouteNodeConfig {
            strategy: RouteStrategy::ManualSticky,
            target: Some(target),
            children,
            ..RouteNodeConfig::default()
        })
    }

    pub fn tag_preferred(
        children: Vec<String>,
        prefer_tags: Vec<BTreeMap<String, String>>,
        on_exhausted: RouteExhaustedAction,
    ) -> Self {
        Self::single_entry_node(RouteNodeConfig {
            strategy: RouteStrategy::TagPreferred,
            children,
            prefer_tags,
            on_exhausted,
            ..RouteNodeConfig::default()
        })
    }

    pub fn single_entry_node(node: RouteNodeConfig) -> Self {
        let entry = non_conflicting_default_route_entry(&node);
        Self {
            routes: BTreeMap::from([(entry.clone(), node)]),
            entry,
            affinity_policy: default_route_affinity_policy(),
            scheduling_preset: SchedulingPreset::default(),
            fallback_ttl_ms: None,
            reprobe_preferred_after_ms: None,
        }
    }

    pub fn entry_node(&self) -> Option<&RouteNodeConfig> {
        self.routes.get(self.entry.as_str())
    }

    pub fn entry_node_mut(&mut self) -> Option<&mut RouteNodeConfig> {
        self.routes.get_mut(self.entry.as_str())
    }

    pub fn ensure_entry_node_mut(&mut self) -> &mut RouteNodeConfig {
        let entry = self.entry.clone();
        self.routes.entry(entry).or_default()
    }

    pub fn set_entry_routing(
        &mut self,
        policy: RouteStrategy,
        target: Option<String>,
        children: Vec<String>,
        prefer_tags: Vec<BTreeMap<String, String>>,
        on_exhausted: RouteExhaustedAction,
    ) {
        let node = self.ensure_entry_node_mut();
        node.strategy = policy;
        node.children = children;
        node.target = target;
        node.prefer_tags = prefer_tags;
        node.on_exhausted = on_exhausted;
        if !matches!(node.strategy, RouteStrategy::ManualSticky) {
            node.target = None;
        }
        if !matches!(node.strategy, RouteStrategy::TagPreferred) {
            node.prefer_tags.clear();
        }
    }

    pub fn clear_entry_target(&mut self, children: Vec<String>) {
        self.set_entry_routing(
            RouteStrategy::OrderedFailover,
            None,
            children,
            Vec::new(),
            RouteExhaustedAction::Continue,
        );
    }

    pub fn ensure_entry_order_contains(&mut self, provider_name: &str) {
        let node = self.ensure_entry_node_mut();
        if !node.children.iter().any(|name| name == provider_name) {
            node.children.push(provider_name.to_string());
        }
    }

    pub fn clear_manual_target_for(&mut self, provider_name: &str) -> bool {
        let node = self.ensure_entry_node_mut();
        let should_clear = matches!(node.strategy, RouteStrategy::ManualSticky)
            && node.target.as_deref() == Some(provider_name);
        if !should_clear {
            return false;
        }
        node.strategy = RouteStrategy::OrderedFailover;
        node.target = None;
        node.prefer_tags.clear();
        node.on_exhausted = RouteExhaustedAction::Continue;
        true
    }

    pub fn remove_provider_references(&mut self, provider_name: &str) {
        for node in self.routes.values_mut() {
            node.children.retain(|name| name != provider_name);
            if node.target.as_deref() == Some(provider_name) {
                node.target = None;
                if matches!(node.strategy, RouteStrategy::ManualSticky) {
                    node.strategy = RouteStrategy::OrderedFailover;
                }
            }
        }
    }

    pub fn route_node_names(&self) -> Vec<String> {
        self.routes.keys().cloned().collect()
    }

    pub fn route_node_references(&self, target: &str) -> Vec<String> {
        self.routes
            .iter()
            .filter_map(|(route_name, node)| {
                if node.children.iter().any(|child| child == target)
                    || node.target.as_deref() == Some(target)
                    || node.then.as_deref() == Some(target)
                    || node.default_route.as_deref() == Some(target)
                {
                    Some(route_name.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn rename_route_node(&mut self, old: &str, new: String) -> Result<()> {
        if old == new {
            return Ok(());
        }
        if !self.routes.contains_key(old) {
            anyhow::bail!("route node '{old}' does not exist");
        }
        if self.routes.contains_key(new.as_str()) {
            anyhow::bail!("route node '{new}' already exists");
        }

        let Some(node) = self.routes.remove(old) else {
            anyhow::bail!("route node '{old}' does not exist");
        };
        self.routes.insert(new.clone(), node);
        if self.entry == old {
            self.entry = new.clone();
        }
        for node in self.routes.values_mut() {
            rewrite_route_node_refs(node, old, new.as_str());
        }
        Ok(())
    }

    pub fn delete_route_node(&mut self, name: &str) -> Result<()> {
        if self.entry == name {
            anyhow::bail!("entry route node '{name}' cannot be deleted");
        }
        if !self.routes.contains_key(name) {
            anyhow::bail!("route node '{name}' does not exist");
        }
        let refs = self.route_node_references(name);
        if !refs.is_empty() {
            anyhow::bail!(
                "route node '{name}' is still referenced by: {}",
                refs.join(", ")
            );
        }
        self.routes.remove(name);
        Ok(())
    }
}

fn default_route_entry() -> String {
    "main".to_string()
}

fn non_conflicting_default_route_entry(node: &RouteNodeConfig) -> String {
    non_conflicting_default_route_entry_from_refs(&node.children, node.target.as_deref())
}

fn non_conflicting_default_route_entry_from_refs(
    children: &[String],
    target: Option<&str>,
) -> String {
    let occupied = children
        .iter()
        .map(String::as_str)
        .chain(target)
        .collect::<BTreeSet<_>>();
    let base = default_route_entry();
    if !occupied.contains(base.as_str()) {
        return base;
    }

    let mut candidate = format!("{base}_route");
    let mut idx = 2usize;
    while occupied.contains(candidate.as_str()) {
        candidate = format!("{base}_route_{idx}");
        idx += 1;
    }
    candidate
}

fn rewrite_route_node_refs(node: &mut RouteNodeConfig, old: &str, new: &str) {
    for child in &mut node.children {
        if child == old {
            *child = new.to_string();
        }
    }
    if node.target.as_deref() == Some(old) {
        node.target = Some(new.to_string());
    }
    if node.then.as_deref() == Some(old) {
        node.then = Some(new.to_string());
    }
    if node.default_route.as_deref() == Some(old) {
        node.default_route = Some(new.to_string());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteNodeConfig {
    #[serde(default = "default_route_strategy")]
    pub strategy: RouteStrategy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer_tags: Vec<BTreeMap<String, String>>,
    #[serde(default = "default_route_exhausted_action")]
    pub on_exhausted: RouteExhaustedAction,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<RouteCondition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub then: Option<String>,
    #[serde(default, rename = "default", skip_serializing_if = "Option::is_none")]
    pub default_route: Option<String>,
}

impl Default for RouteNodeConfig {
    fn default() -> Self {
        Self {
            strategy: default_route_strategy(),
            children: Vec::new(),
            target: None,
            prefer_tags: Vec::new(),
            on_exhausted: default_route_exhausted_action(),
            metadata: BTreeMap::new(),
            when: None,
            then: None,
            default_route: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, Hash)]
pub struct RouteCondition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

impl RouteCondition {
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.service_tier.is_none()
            && self.reasoning_effort.is_none()
            && self.path.is_none()
            && self.method.is_none()
            && self.headers.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RouteStrategy {
    ManualSticky,
    OrderedFailover,
    RoundRobin,
    TagPreferred,
    Conditional,
}

fn default_route_strategy() -> RouteStrategy {
    RouteStrategy::OrderedFailover
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RouteExhaustedAction {
    Continue,
    Stop,
}

fn default_route_exhausted_action() -> RouteExhaustedAction {
    RouteExhaustedAction::Continue
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RouteAffinityPolicy {
    Off,
    PreferredGroup,
    FallbackSticky,
    Hard,
}

fn default_route_affinity_policy() -> RouteAffinityPolicy {
    RouteAffinityPolicy::FallbackSticky
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum SchedulingPreset {
    ContinuityFirst,
    #[default]
    Balanced,
    ThroughputFirst,
}

impl SchedulingPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ContinuityFirst => "continuity-first",
            Self::Balanced => "balanced",
            Self::ThroughputFirst => "throughput-first",
        }
    }

    fn is_default(value: &Self) -> bool {
        *value == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct RoutePoolConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedRoutingProviderRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default = "default_service_config_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedRoutingSpec {
    pub entry: String,
    #[serde(default = "default_route_affinity_policy")]
    pub affinity_policy: RouteAffinityPolicy,
    #[serde(default, skip_serializing_if = "SchedulingPreset::is_default")]
    pub scheduling_preset: SchedulingPreset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reprobe_preferred_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routes: BTreeMap<String, RouteNodeConfig>,
    #[serde(default = "default_route_strategy")]
    pub policy: RouteStrategy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer_tags: Vec<BTreeMap<String, String>>,
    #[serde(default = "default_route_exhausted_action")]
    pub on_exhausted: RouteExhaustedAction,
    #[serde(default = "default_route_strategy")]
    pub entry_strategy: RouteStrategy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded_order: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<PersistedRoutingProviderRef>,
}

fn is_default_upstream_auth(auth: &UpstreamAuth) -> bool {
    auth.auth_token.is_none()
        && auth.auth_token_env.is_none()
        && auth.api_key.is_none()
        && auth.api_key_env.is_none()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PersistedProviderEndpointSpec {
    pub name: String,
    pub base_url: String,
    #[serde(default = "default_service_config_enabled")]
    pub enabled: bool,
    #[serde(
        default = "default_provider_endpoint_priority",
        skip_serializing_if = "is_default_provider_endpoint_priority"
    )]
    pub priority: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "is_default_provider_concurrency_limits"
    )]
    pub limits: ProviderConcurrencyLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PersistedProviderSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default = "default_service_config_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_token_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "is_default_provider_concurrency_limits"
    )]
    pub limits: ProviderConcurrencyLimits,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<PersistedProviderEndpointSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PersistedProvidersCatalog {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<PersistedProviderSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiConfig {
    /// UI language: `en`, `zh`, or `auto` (default: unset).
    ///
    /// When unset, codex-helper will pick a default language based on system locale for the first run.
    #[serde(default)]
    pub language: Option<String>,
    /// Optional remote service status probes shown by operator UIs.
    #[serde(default, skip_serializing_if = "is_default_service_status_config")]
    pub service_status: ServiceStatusConfig,
}

/// 获取 codex-helper 的主目录（用于配置、日志等）
pub fn proxy_home_dir() -> PathBuf {
    if let Ok(dir) = env::var("CODEX_HELPER_HOME") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    #[cfg(test)]
    {
        static TEST_HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        TEST_HOME
            .get_or_init(|| {
                let mut dir = std::env::temp_dir();
                let unique = format!(
                    "codex-helper-test-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0)
                );
                dir.push(unique);
                dir.push(".codex-helper");
                let _ = std::fs::create_dir_all(&dir);
                dir
            })
            .clone()
    }

    #[cfg(not(test))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".codex-helper")
    }
}

/// Directory where Codex stores conversation sessions: `~/.codex/sessions` (or `$CODEX_HOME/sessions`).
pub fn codex_sessions_dir() -> PathBuf {
    codex_home().join("sessions")
}

/// 支持的上游服务类型：Codex / Claude。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    Codex,
    Claude,
}

#[cfg(test)]
mod tests;
