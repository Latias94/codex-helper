use std::collections::{BTreeMap, HashMap};
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

#[path = "config_v3.rs"]
mod v3_impl;

pub use auth_sync_impl::{
    SyncCodexAuthFromCodexOptions, SyncCodexAuthFromCodexReport, sync_codex_auth_from_codex_cli,
};
pub(crate) use auth_sync_impl::{infer_env_key_from_auth_json, read_file_if_exists};
pub use bootstrap_impl::{
    import_codex_config_from_codex_cli, load_or_bootstrap_for_service,
    load_or_bootstrap_from_claude, load_or_bootstrap_from_codex,
    overwrite_codex_config_from_codex_cli_in_place, probe_codex_bootstrap_from_cli,
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
    config_file_path, init_config_toml, load_config, save_config, save_config_v2, save_config_v3,
};
pub use v2_impl::{
    build_persisted_provider_catalog, build_persisted_station_catalog, compact_v2_config,
    compile_v2_to_runtime, migrate_legacy_to_v2,
};
pub(crate) use v3_impl::compact_v3_config_for_write;
pub use v3_impl::{
    ConfigV3MigrationReport, compile_v3_to_runtime, compile_v3_to_v2, migrate_legacy_to_v3,
    migrate_legacy_to_v3_with_report, migrate_v2_to_v3, migrate_v2_to_v3_with_report,
};

#[cfg(test)]
use bootstrap_impl::bootstrap_from_codex;

pub mod storage {
    pub use super::storage_impl::{
        config_file_path, init_config_toml, load_config, save_config, save_config_v2,
        save_config_v3,
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

fn default_proxy_config_v3_version() -> u32 {
    3
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
pub struct ProxyConfigV3 {
    #[serde(default = "default_proxy_config_v3_version")]
    pub version: u32,
    #[serde(default)]
    pub codex: ServiceViewV3,
    #[serde(default)]
    pub claude: ServiceViewV3,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub notify: NotifyConfig,
    #[serde(default)]
    pub default_service: Option<ServiceKind>,
    #[serde(default)]
    pub ui: UiConfig,
}

impl Default for ProxyConfigV3 {
    fn default() -> Self {
        Self {
            version: default_proxy_config_v3_version(),
            codex: ServiceViewV3::default(),
            claude: ServiceViewV3::default(),
            retry: RetryConfig::default(),
            notify: NotifyConfig::default(),
            default_service: None,
            ui: UiConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceViewV3 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ServiceControlProfile>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderConfigV3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingConfigV3>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfigV3 {
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
    pub endpoints: BTreeMap<String, ProviderEndpointV3>,
}

impl Default for ProviderConfigV3 {
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
pub struct ProviderEndpointV3 {
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
pub struct RoutingConfigV3 {
    #[serde(default = "default_routing_policy_v3")]
    pub policy: RoutingPolicyV3,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer_tags: Vec<BTreeMap<String, String>>,
    #[serde(default = "default_routing_on_exhausted_v3")]
    pub on_exhausted: RoutingExhaustedActionV3,
}

impl Default for RoutingConfigV3 {
    fn default() -> Self {
        Self {
            policy: default_routing_policy_v3(),
            order: Vec::new(),
            target: None,
            prefer_tags: Vec::new(),
            on_exhausted: default_routing_on_exhausted_v3(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingPolicyV3 {
    ManualSticky,
    OrderedFailover,
    TagPreferred,
}

fn default_routing_policy_v3() -> RoutingPolicyV3 {
    RoutingPolicyV3::OrderedFailover
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingExhaustedActionV3 {
    Continue,
    Stop,
}

fn default_routing_on_exhausted_v3() -> RoutingExhaustedActionV3 {
    RoutingExhaustedActionV3::Continue
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
    #[serde(default = "default_routing_policy_v3")]
    pub policy: RoutingPolicyV3,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer_tags: Vec<BTreeMap<String, String>>,
    #[serde(default = "default_routing_on_exhausted_v3")]
    pub on_exhausted: RoutingExhaustedActionV3,
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
