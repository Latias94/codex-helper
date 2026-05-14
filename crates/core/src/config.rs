use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fs as stdfs;
use std::path::{Path, PathBuf};

use crate::client_config::{codex_home, is_claude_absent_backup_sentinel};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::fs;
use toml::Value as TomlValue;
use tracing::{info, warn};

pub use crate::client_config::{
    claude_settings_backup_path, claude_settings_path, codex_auth_path, codex_config_path,
    codex_switch_state_path,
};

#[path = "config_storage.rs"]
mod storage_impl;

#[path = "config_bootstrap.rs"]
mod bootstrap_impl;

#[path = "config_auth_sync.rs"]
mod auth_sync_impl;

#[path = "config_retry.rs"]
mod retry_impl;

#[path = "config_profiles.rs"]
mod profiles_impl;

#[path = "config_routing.rs"]
mod routing_impl;

#[path = "config_v2.rs"]
mod v2_impl;

#[path = "config_v4.rs"]
mod v4_impl;

pub use auth_sync_impl::{
    SyncCodexAuthFromCodexOptions, SyncCodexAuthFromCodexReport, sync_codex_auth_from_codex_cli,
};
pub(crate) use auth_sync_impl::{infer_env_key_from_auth_json, read_file_if_exists};
pub use bootstrap_impl::{
    import_codex_config_from_codex_cli, load_or_bootstrap_for_service,
    load_or_bootstrap_for_service_with_v4_source, load_or_bootstrap_from_claude,
    load_or_bootstrap_from_codex, overwrite_codex_config_from_codex_cli_in_place,
    probe_codex_bootstrap_from_cli,
};
pub(crate) use profiles_impl::validate_service_profiles;
pub use profiles_impl::{
    ServiceControlProfile, resolve_service_profile, resolve_service_profile_from_catalog,
    validate_profile_station_compatibility,
};
pub use retry_impl::{
    ResolvedRetryConfig, ResolvedRetryLayerConfig, RetryConfig, RetryLayerConfig, RetryProfileName,
    RetryStrategy,
};
pub use routing_impl::{RoutingCandidate, ServiceRoutingExplanation, explain_service_routing};
pub use storage_impl::{
    LoadedProxyConfig, config_file_path, init_config_toml, load_config, load_config_with_v4_source,
    save_config, save_config_v2, save_config_v4,
};
pub use v2_impl::{
    build_persisted_provider_catalog, build_persisted_station_catalog, compact_v2_config,
    compile_v2_to_runtime, migrate_legacy_to_v2,
};
pub(crate) use v4_impl::compact_v4_config_for_write;
pub use v4_impl::{
    ConfigV4MigrationReport, collect_route_graph_affinity_migration_warnings,
    compile_v4_to_runtime, compile_v4_to_v2, effective_v4_routing, migrate_legacy_to_v4,
    migrate_legacy_to_v4_with_report, migrate_v2_to_v4, migrate_v2_to_v4_with_report,
    resolved_v4_provider_order,
};

pub mod legacy {
    pub use super::v4_impl::legacy::*;
}

#[cfg(test)]
use bootstrap_impl::bootstrap_from_codex;

pub mod storage {
    pub use super::storage_impl::{
        LoadedProxyConfig, config_file_path, init_config_toml, load_config,
        load_config_with_v4_source, save_config, save_config_v2, save_config_v4,
    };
}

pub mod bootstrap {
    pub use super::bootstrap_impl::{
        import_codex_config_from_codex_cli, load_or_bootstrap_for_service,
        load_or_bootstrap_from_claude, load_or_bootstrap_from_codex,
        overwrite_codex_config_from_codex_cli_in_place, probe_codex_bootstrap_from_cli,
    };
}

pub mod auth_sync {
    pub use super::auth_sync_impl::{
        SyncCodexAuthFromCodexOptions, SyncCodexAuthFromCodexReport, sync_codex_auth_from_codex_cli,
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

pub fn model_routing_warnings(cfg: &ProxyConfig, service_name: &str) -> Vec<String> {
    use crate::model_routing::match_wildcard;

    fn validate_upstream(name: &str, upstream: &UpstreamConfig) -> Vec<String> {
        let mut out = Vec::new();

        if upstream.supported_models.is_empty() && upstream.model_mapping.is_empty() {
            out.push(format!(
                "[{name}] 未配置 supported_models 或 model_mapping，将假设支持所有模型（可能导致降级失败）"
            ));
            return out;
        }

        if !upstream.model_mapping.is_empty() && upstream.supported_models.is_empty() {
            out.push(format!(
                "[{name}] 配置了 model_mapping 但未配置 supported_models，映射目标将不做校验，请确认目标模型在供应商处可用"
            ));
        }

        if upstream.model_mapping.is_empty() || upstream.supported_models.is_empty() {
            return out;
        }

        for (external_model, internal_model) in upstream.model_mapping.iter() {
            if internal_model.contains('*') {
                continue;
            }
            let supported = if upstream
                .supported_models
                .get(internal_model)
                .copied()
                .unwrap_or(false)
            {
                true
            } else {
                upstream
                    .supported_models
                    .keys()
                    .any(|p| match_wildcard(p, internal_model))
            };
            if !supported {
                out.push(format!(
                    "[{name}] 模型映射无效：'{external_model}' -> '{internal_model}'，目标模型不在 supported_models 中"
                ));
            }
        }
        out
    }

    let mgr = match service_name {
        "claude" => &cfg.claude,
        "codex" => &cfg.codex,
        _ => &cfg.codex,
    };

    let mut warnings = Vec::new();
    for (cfg_name, svc) in mgr.stations() {
        for (idx, upstream) in svc.upstreams.iter().enumerate() {
            let name = format!(
                "{service_name}:{cfg_name} upstream[{idx}] ({})",
                upstream.base_url
            );
            warnings.extend(validate_upstream(&name, upstream));
        }
    }
    warnings
}

/// A logical config entry (roughly corresponds to cli_proxy 的一个配置名)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// 配置标识（map key），保持稳定
    #[serde(default)]
    pub name: String,
    /// 可选别名，便于展示/记忆
    #[serde(default)]
    pub alias: Option<String>,
    /// Whether this config is eligible for automatic routing (defaults to true).
    #[serde(default = "default_service_config_enabled")]
    pub enabled: bool,
    /// Priority group (1..=10, lower is higher priority). Default: 1.
    #[serde(default = "default_service_config_level")]
    pub level: u8,
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,
}

fn default_service_config_enabled() -> bool {
    true
}

fn is_default_service_config_enabled(value: &bool) -> bool {
    *value == default_service_config_enabled()
}

fn default_service_config_level() -> u8 {
    1
}

fn default_provider_endpoint_priority() -> u32 {
    0
}

fn is_default_provider_endpoint_priority(value: &u32) -> bool {
    *value == default_provider_endpoint_priority()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceConfigManager {
    /// 当前激活配置名
    #[serde(default)]
    pub active: Option<String>,
    /// 新会话默认使用的控制模板名（Phase 1: 仅加载与展示，不自动绑定）。
    #[serde(default)]
    pub default_profile: Option<String>,
    /// 可复用控制模板。
    #[serde(default)]
    pub profiles: BTreeMap<String, ServiceControlProfile>,
    /// 站点集合。公共序列化使用 `stations`，仍兼容读取 legacy `configs`。
    #[serde(default, rename = "stations", alias = "configs")]
    pub configs: HashMap<String, ServiceConfig>,
}

impl ServiceConfigManager {
    pub fn stations(&self) -> &HashMap<String, ServiceConfig> {
        &self.configs
    }

    pub fn stations_mut(&mut self) -> &mut HashMap<String, ServiceConfig> {
        &mut self.configs
    }

    pub fn station(&self, name: &str) -> Option<&ServiceConfig> {
        self.stations().get(name)
    }

    pub fn station_mut(&mut self, name: &str) -> Option<&mut ServiceConfig> {
        self.stations_mut().get_mut(name)
    }

    pub fn contains_station(&self, name: &str) -> bool {
        self.station(name).is_some()
    }

    pub fn station_count(&self) -> usize {
        self.stations().len()
    }

    pub fn has_stations(&self) -> bool {
        !self.stations().is_empty()
    }

    pub fn active_station(&self) -> Option<&ServiceConfig> {
        self.active
            .as_ref()
            .and_then(|name| self.station(name))
            // HashMap 的 values().next() 是非确定性的；这里用 key 排序后的最小项作为稳定兜底。
            .or_else(|| {
                self.stations()
                    .iter()
                    .min_by_key(|(k, _)| *k)
                    .map(|(_, v)| v)
            })
    }

    pub fn profile(&self, name: &str) -> Option<&ServiceControlProfile> {
        self.profiles.get(name)
    }

    pub fn default_profile_ref(&self) -> Option<(&str, &ServiceControlProfile)> {
        let name = self.default_profile.as_deref()?;
        self.profile(name).map(|profile| (name, profile))
    }
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
    /// How far back to look in proxy recent-finished list when matching a thread-id.
    pub recent_search_window_ms: u64,
    /// Timeout for calling proxy `status/recent` endpoint.
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxyConfig {
    /// Optional config schema version for future migrations
    #[serde(default)]
    pub version: Option<u32>,
    /// Codex 服务配置
    #[serde(default)]
    pub codex: ServiceConfigManager,
    /// Claude Code 等其他服务配置，后续扩展
    #[serde(default)]
    pub claude: ServiceConfigManager,
    /// Global retry policy (proxy-side).
    #[serde(default)]
    pub retry: RetryConfig,
    /// Notify integration settings (used by `codex-helper notify ...`).
    #[serde(default)]
    pub notify: NotifyConfig,
    /// 默认目标服务（用于 CLI 默认选择 codex/claude）
    #[serde(default)]
    pub default_service: Option<ServiceKind>,
    /// UI settings (mainly for the built-in TUI).
    #[serde(default)]
    pub ui: UiConfig,
}

fn default_proxy_config_v2_version() -> u32 {
    2
}

pub const LEGACY_ROUTE_GRAPH_CONFIG_VERSION: u32 = 4;
pub const CURRENT_ROUTE_GRAPH_CONFIG_VERSION: u32 = 5;

pub fn is_supported_route_graph_config_version(version: u32) -> bool {
    matches!(
        version,
        LEGACY_ROUTE_GRAPH_CONFIG_VERSION | CURRENT_ROUTE_GRAPH_CONFIG_VERSION
    )
}

fn default_proxy_config_v4_version() -> u32 {
    CURRENT_ROUTE_GRAPH_CONFIG_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfigV2 {
    #[serde(default = "default_proxy_config_v2_version")]
    pub version: u32,
    #[serde(default)]
    pub codex: ServiceViewV2,
    #[serde(default)]
    pub claude: ServiceViewV2,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub notify: NotifyConfig,
    #[serde(default)]
    pub default_service: Option<ServiceKind>,
    #[serde(default)]
    pub ui: UiConfig,
}

impl Default for ProxyConfigV2 {
    fn default() -> Self {
        Self {
            version: default_proxy_config_v2_version(),
            codex: ServiceViewV2::default(),
            claude: ServiceViewV2::default(),
            retry: RetryConfig::default(),
            notify: NotifyConfig::default(),
            default_service: None,
            ui: UiConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfigV4 {
    #[serde(default = "default_proxy_config_v4_version")]
    pub version: u32,
    #[serde(default)]
    pub codex: ServiceViewV4,
    #[serde(default)]
    pub claude: ServiceViewV4,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub notify: NotifyConfig,
    #[serde(default)]
    pub default_service: Option<ServiceKind>,
    #[serde(default)]
    pub ui: UiConfig,
}

impl Default for ProxyConfigV4 {
    fn default() -> Self {
        Self {
            version: default_proxy_config_v4_version(),
            codex: ServiceViewV4::default(),
            claude: ServiceViewV4::default(),
            retry: RetryConfig::default(),
            notify: NotifyConfig::default(),
            default_service: None,
            ui: UiConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceViewV4 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ServiceControlProfile>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderConfigV4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingConfigV4>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfigV4 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(
        default = "default_service_config_enabled",
        skip_serializing_if = "is_default_service_config_enabled"
    )]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub endpoints: BTreeMap<String, ProviderEndpointV4>,
}

impl Default for ProviderConfigV4 {
    fn default() -> Self {
        Self {
            alias: None,
            enabled: default_service_config_enabled(),
            base_url: None,
            auth: UpstreamAuth::default(),
            inline_auth: UpstreamAuth::default(),
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            endpoints: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEndpointV4 {
    pub base_url: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfigV4 {
    #[serde(default = "default_routing_entry_v4")]
    pub entry: String,
    #[serde(
        default = "default_routing_affinity_policy_v5",
        skip_serializing_if = "is_default_routing_affinity_policy_v5"
    )]
    pub affinity_policy: RoutingAffinityPolicyV5,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reprobe_preferred_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routes: BTreeMap<String, RoutingNodeV4>,
    #[serde(skip, default = "default_routing_policy_v4")]
    pub policy: RoutingPolicyV4,
    #[serde(skip)]
    pub order: Vec<String>,
    #[serde(skip)]
    pub target: Option<String>,
    #[serde(skip)]
    pub prefer_tags: Vec<BTreeMap<String, String>>,
    #[serde(skip)]
    pub chain: Vec<String>,
    #[serde(skip)]
    pub pools: BTreeMap<String, RoutingPoolV4>,
    #[serde(skip, default = "default_routing_on_exhausted_v4")]
    pub on_exhausted: RoutingExhaustedActionV4,
}

impl Default for RoutingConfigV4 {
    fn default() -> Self {
        Self {
            entry: default_routing_entry_v4(),
            affinity_policy: default_routing_affinity_policy_v5(),
            fallback_ttl_ms: None,
            reprobe_preferred_after_ms: None,
            routes: BTreeMap::new(),
            policy: default_routing_policy_v4(),
            order: Vec::new(),
            target: None,
            prefer_tags: Vec::new(),
            chain: Vec::new(),
            pools: BTreeMap::new(),
            on_exhausted: default_routing_on_exhausted_v4(),
        }
    }
}

impl ProxyConfigV4 {
    pub fn sync_routing_compat_from_graph(&mut self) {
        if let Some(routing) = self.codex.routing.as_mut() {
            routing.sync_compat_from_graph();
        }
        if let Some(routing) = self.claude.routing.as_mut() {
            routing.sync_compat_from_graph();
        }
    }
}

impl RoutingConfigV4 {
    pub fn ordered_failover(children: Vec<String>) -> Self {
        Self::single_entry_node(RoutingNodeV4 {
            strategy: RoutingPolicyV4::OrderedFailover,
            children,
            ..RoutingNodeV4::default()
        })
    }

    pub fn manual_sticky(target: String, children: Vec<String>) -> Self {
        Self::single_entry_node(RoutingNodeV4 {
            strategy: RoutingPolicyV4::ManualSticky,
            target: Some(target),
            children,
            ..RoutingNodeV4::default()
        })
    }

    pub fn tag_preferred(
        children: Vec<String>,
        prefer_tags: Vec<BTreeMap<String, String>>,
        on_exhausted: RoutingExhaustedActionV4,
    ) -> Self {
        Self::single_entry_node(RoutingNodeV4 {
            strategy: RoutingPolicyV4::TagPreferred,
            children,
            prefer_tags,
            on_exhausted,
            ..RoutingNodeV4::default()
        })
    }

    pub fn single_entry_node(node: RoutingNodeV4) -> Self {
        let entry = non_conflicting_default_route_entry(&node);
        let mut out = Self {
            routes: BTreeMap::from([(entry.clone(), node)]),
            entry,
            affinity_policy: default_routing_affinity_policy_v5(),
            fallback_ttl_ms: None,
            reprobe_preferred_after_ms: None,
            policy: default_routing_policy_v4(),
            order: Vec::new(),
            target: None,
            prefer_tags: Vec::new(),
            chain: Vec::new(),
            pools: BTreeMap::new(),
            on_exhausted: default_routing_on_exhausted_v4(),
        };
        out.sync_compat_from_graph();
        out
    }

    pub fn has_compat_authoring_fields(&self) -> bool {
        self.policy != default_routing_policy_v4()
            || !self.order.is_empty()
            || self.target.is_some()
            || !self.prefer_tags.is_empty()
            || !self.chain.is_empty()
            || !self.pools.is_empty()
            || self.on_exhausted != default_routing_on_exhausted_v4()
    }

    pub fn entry_node(&self) -> Option<&RoutingNodeV4> {
        self.routes.get(self.entry.as_str())
    }

    pub fn entry_node_mut(&mut self) -> Option<&mut RoutingNodeV4> {
        self.routes.get_mut(self.entry.as_str())
    }

    pub fn sync_compat_from_graph(&mut self) {
        let Some(node) = self.entry_node().cloned() else {
            self.policy = default_routing_policy_v4();
            self.order.clear();
            self.target = None;
            self.prefer_tags.clear();
            self.chain.clear();
            self.pools.clear();
            self.on_exhausted = default_routing_on_exhausted_v4();
            return;
        };

        self.policy = node.strategy;
        self.target = node.target.clone();
        self.prefer_tags = node.prefer_tags.clone();
        self.on_exhausted = node.on_exhausted;
        self.order = node.children.clone();
    }

    pub fn sync_graph_from_compat(&mut self) {
        if !self.has_compat_authoring_fields() {
            return;
        }

        if self.routes.is_empty() {
            self.entry =
                non_conflicting_default_route_entry_from_refs(&self.order, self.target.as_deref());
            self.routes
                .insert(self.entry.clone(), RoutingNodeV4::default());
        }

        let entry = self.entry.clone();
        let node = self.routes.entry(entry).or_default();
        node.strategy = self.policy;
        node.target = self.target.clone();
        node.prefer_tags = self.prefer_tags.clone();
        node.on_exhausted = self.on_exhausted;
        if !self.order.is_empty() {
            node.children = self.order.clone();
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
        self.sync_compat_from_graph();
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
        self.sync_compat_from_graph();
        Ok(())
    }
}

fn default_routing_entry_v4() -> String {
    "main".to_string()
}

fn non_conflicting_default_route_entry(node: &RoutingNodeV4) -> String {
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
    let base = default_routing_entry_v4();
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

fn rewrite_route_node_refs(node: &mut RoutingNodeV4, old: &str, new: &str) {
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
pub struct RoutingNodeV4 {
    #[serde(default = "default_routing_policy_v4")]
    pub strategy: RoutingPolicyV4,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer_tags: Vec<BTreeMap<String, String>>,
    #[serde(default = "default_routing_on_exhausted_v4")]
    pub on_exhausted: RoutingExhaustedActionV4,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<RoutingConditionV4>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub then: Option<String>,
    #[serde(default, rename = "default", skip_serializing_if = "Option::is_none")]
    pub default_route: Option<String>,
}

impl Default for RoutingNodeV4 {
    fn default() -> Self {
        Self {
            strategy: default_routing_policy_v4(),
            children: Vec::new(),
            target: None,
            prefer_tags: Vec::new(),
            on_exhausted: default_routing_on_exhausted_v4(),
            metadata: BTreeMap::new(),
            when: None,
            then: None,
            default_route: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, Hash)]
pub struct RoutingConditionV4 {
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

impl RoutingConditionV4 {
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
pub enum RoutingPolicyV4 {
    ManualSticky,
    OrderedFailover,
    TagPreferred,
    Conditional,
}

fn default_routing_policy_v4() -> RoutingPolicyV4 {
    RoutingPolicyV4::OrderedFailover
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingExhaustedActionV4 {
    Continue,
    Stop,
}

fn default_routing_on_exhausted_v4() -> RoutingExhaustedActionV4 {
    RoutingExhaustedActionV4::Continue
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingAffinityPolicyV5 {
    Off,
    PreferredGroup,
    FallbackSticky,
    Hard,
}

fn default_routing_affinity_policy_v5() -> RoutingAffinityPolicyV5 {
    RoutingAffinityPolicyV5::PreferredGroup
}

fn is_default_routing_affinity_policy_v5(policy: &RoutingAffinityPolicyV5) -> bool {
    *policy == default_routing_affinity_policy_v5()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct RoutingPoolV4 {
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
    #[serde(default = "default_routing_affinity_policy_v5")]
    pub affinity_policy: RoutingAffinityPolicyV5,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reprobe_preferred_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routes: BTreeMap<String, RoutingNodeV4>,
    #[serde(default = "default_routing_policy_v4")]
    pub policy: RoutingPolicyV4,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer_tags: Vec<BTreeMap<String, String>>,
    #[serde(default = "default_routing_on_exhausted_v4")]
    pub on_exhausted: RoutingExhaustedActionV4,
    #[serde(default = "default_routing_policy_v4")]
    pub entry_strategy: RoutingPolicyV4,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceViewV2 {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "active_station",
        alias = "active_group"
    )]
    pub active_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ServiceControlProfile>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderConfigV2>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        rename = "stations",
        alias = "groups"
    )]
    pub groups: BTreeMap<String, GroupConfigV2>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfigV2 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default = "default_service_config_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub auth: UpstreamAuth,
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub endpoints: BTreeMap<String, ProviderEndpointV2>,
}

impl Default for ProviderConfigV2 {
    fn default() -> Self {
        Self {
            alias: None,
            enabled: default_service_config_enabled(),
            auth: UpstreamAuth::default(),
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            endpoints: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEndpointV2 {
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupConfigV2 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default = "default_service_config_enabled")]
    pub enabled: bool,
    #[serde(default = "default_service_config_level")]
    pub level: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<GroupMemberRefV2>,
}

impl Default for GroupConfigV2 {
    fn default() -> Self {
        Self {
            alias: None,
            enabled: default_service_config_enabled(),
            level: default_service_config_level(),
            members: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct GroupMemberRefV2 {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "endpoints")]
    pub endpoint_names: Vec<String>,
    #[serde(default)]
    pub preferred: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PersistedStationProviderEndpointRef {
    pub name: String,
    pub base_url: String,
    #[serde(default = "default_service_config_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PersistedStationProviderRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default = "default_service_config_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<PersistedStationProviderEndpointRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PersistedStationSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default = "default_service_config_enabled")]
    pub enabled: bool,
    #[serde(default = "default_service_config_level")]
    pub level: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<GroupMemberRefV2>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PersistedStationsCatalog {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stations: Vec<PersistedStationSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<PersistedStationProviderRef>,
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
