use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs as stdfs;
use std::path::{Path, PathBuf};

use crate::client_config::{
    codex_home, is_claude_absent_backup_sentinel, is_codex_absent_backup_sentinel,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::fs;
use toml::Value as TomlValue;
use tracing::{info, warn};

pub use crate::client_config::{
    claude_settings_backup_path, claude_settings_path, codex_auth_path, codex_backup_config_path,
    codex_config_path,
};

#[path = "config_storage.rs"]
mod storage_impl;

#[path = "config_bootstrap.rs"]
mod bootstrap_impl;

#[path = "config_auth_sync.rs"]
mod auth_sync_impl;

pub use auth_sync_impl::{
    SyncCodexAuthFromCodexOptions, SyncCodexAuthFromCodexReport, sync_codex_auth_from_codex_cli,
};
pub(crate) use auth_sync_impl::{infer_env_key_from_auth_json, read_file_if_exists};
pub use bootstrap_impl::{
    import_codex_config_from_codex_cli, load_or_bootstrap_for_service,
    load_or_bootstrap_from_claude, load_or_bootstrap_from_codex,
    overwrite_codex_config_from_codex_cli_in_place, probe_codex_bootstrap_from_cli,
};
pub use storage_impl::{
    config_file_path, init_config_toml, load_config, save_config, save_config_v2,
};

#[cfg(test)]
use bootstrap_impl::bootstrap_from_codex;

pub mod storage {
    pub use super::storage_impl::{
        config_file_path, init_config_toml, load_config, save_config, save_config_v2,
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
    for (cfg_name, svc) in mgr.configs.iter() {
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

fn default_service_config_level() -> u8 {
    1
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
    /// 配置集合
    #[serde(default)]
    pub configs: HashMap<String, ServiceConfig>,
}

impl ServiceConfigManager {
    pub fn active_config(&self) -> Option<&ServiceConfig> {
        self.active
            .as_ref()
            .and_then(|name| self.configs.get(name))
            // HashMap 的 values().next() 是非确定性的；这里用 key 排序后的最小项作为稳定兜底。
            .or_else(|| self.configs.iter().min_by_key(|(k, _)| *k).map(|(_, v)| v))
    }

    pub fn profile(&self, name: &str) -> Option<&ServiceControlProfile> {
        self.profiles.get(name)
    }

    pub fn default_profile_ref(&self) -> Option<(&str, &ServiceControlProfile)> {
        let name = self.default_profile.as_deref()?;
        self.profile(name).map(|profile| (name, profile))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ServiceControlProfile {
    /// Phase 1 keeps legacy runtime terminology underneath, so `station` currently points to
    /// a legacy config/group name.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "config")]
    pub station: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExplicitCapabilitySupport {
    Unknown,
    Supported,
    Unsupported,
}

fn parse_boolish_capability_tag(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" | "supported" => Some(true),
        "0" | "false" | "no" | "n" | "off" | "unsupported" => Some(false),
        _ => None,
    }
}

fn explicit_capability_support_for_upstreams(
    upstreams: &[UpstreamConfig],
    tag_keys: &[&str],
) -> ExplicitCapabilitySupport {
    let mut saw_supported = false;
    let mut saw_explicit_unsupported = false;
    let mut saw_unknown = false;

    for upstream in upstreams {
        match tag_keys
            .iter()
            .find_map(|key| upstream.tags.get(*key))
            .and_then(|value| parse_boolish_capability_tag(value))
        {
            Some(true) => saw_supported = true,
            Some(false) => saw_explicit_unsupported = true,
            None => saw_unknown = true,
        }
    }

    if saw_supported {
        ExplicitCapabilitySupport::Supported
    } else if saw_explicit_unsupported && !saw_unknown {
        ExplicitCapabilitySupport::Unsupported
    } else {
        ExplicitCapabilitySupport::Unknown
    }
}

pub fn validate_profile_station_compatibility(
    service_name: &str,
    mgr: &ServiceConfigManager,
    profile_name: &str,
    profile: &ServiceControlProfile,
) -> Result<()> {
    let Some(station) = profile
        .station
        .as_deref()
        .map(str::trim)
        .filter(|station| !station.is_empty())
    else {
        return Ok(());
    };

    let Some(config) = mgr.configs.get(station) else {
        anyhow::bail!(
            "[{service_name}] profile '{}' references missing station/config '{}'",
            profile_name,
            station
        );
    };

    if let Some(model) = profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        let supported = config.upstreams.is_empty()
            || config.upstreams.iter().any(|upstream| {
                crate::model_routing::is_model_supported(
                    &upstream.supported_models,
                    &upstream.model_mapping,
                    model,
                )
            });
        if !supported {
            anyhow::bail!(
                "[{service_name}] profile '{}' model '{}' is not supported by station/config '{}'",
                profile_name,
                model,
                station
            );
        }
    }

    if let Some(service_tier) = profile
        .service_tier
        .as_deref()
        .map(str::trim)
        .filter(|service_tier| !service_tier.is_empty())
        && explicit_capability_support_for_upstreams(
            &config.upstreams,
            &[
                "supports_service_tier",
                "supports_service_tiers",
                "supports_fast_mode",
                "supports_fast",
            ],
        ) == ExplicitCapabilitySupport::Unsupported
    {
        anyhow::bail!(
            "[{service_name}] profile '{}' requires service_tier '{}' but station/config '{}' explicitly disables fast/service-tier support",
            profile_name,
            service_tier,
            station
        );
    }

    if let Some(reasoning_effort) = profile
        .reasoning_effort
        .as_deref()
        .map(str::trim)
        .filter(|reasoning_effort| !reasoning_effort.is_empty())
        && explicit_capability_support_for_upstreams(
            &config.upstreams,
            &["supports_reasoning_effort", "supports_reasoning"],
        ) == ExplicitCapabilitySupport::Unsupported
    {
        anyhow::bail!(
            "[{service_name}] profile '{}' requires reasoning_effort '{}' but station/config '{}' explicitly disables reasoning support",
            profile_name,
            reasoning_effort,
            station
        );
    }

    Ok(())
}

fn validate_service_profiles(service_name: &str, mgr: &ServiceConfigManager) -> Result<()> {
    if let Some(default_profile) = mgr.default_profile.as_deref()
        && !mgr.profiles.contains_key(default_profile)
    {
        anyhow::bail!(
            "[{service_name}] default_profile '{}' does not exist in profiles",
            default_profile
        );
    }

    for (profile_name, profile) in &mgr.profiles {
        validate_profile_station_compatibility(service_name, mgr, profile_name, profile)?;
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RetryProfileName {
    Balanced,
    SameUpstream,
    AggressiveFailover,
    CostPrimary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedRetryLayerConfig {
    pub max_attempts: u32,
    pub backoff_ms: u64,
    pub backoff_max_ms: u64,
    pub jitter_ms: u64,
    pub on_status: String,
    pub on_class: Vec<String>,
    pub strategy: RetryStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedRetryConfig {
    pub upstream: ResolvedRetryLayerConfig,
    pub provider: ResolvedRetryLayerConfig,
    pub never_on_status: String,
    pub never_on_class: Vec<String>,
    pub cloudflare_challenge_cooldown_secs: u64,
    pub cloudflare_timeout_cooldown_secs: u64,
    pub transport_cooldown_secs: u64,
    pub cooldown_backoff_factor: u64,
    pub cooldown_backoff_max_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RetryLayerConfig {
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub backoff_ms: Option<u64>,
    #[serde(default)]
    pub backoff_max_ms: Option<u64>,
    #[serde(default)]
    pub jitter_ms: Option<u64>,
    #[serde(default)]
    pub on_status: Option<String>,
    #[serde(default)]
    pub on_class: Option<Vec<String>>,
    #[serde(default)]
    pub strategy: Option<RetryStrategy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Curated retry policy preset. When set, codex-helper starts from the profile defaults,
    /// then applies any explicitly configured fields below as overrides.
    #[serde(default)]
    pub profile: Option<RetryProfileName>,
    // Legacy (pre-v0.10.0) flat retry fields (kept for backward compatibility).
    // Prefer the nested `upstream` / `provider` blocks for new configs.
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub backoff_ms: Option<u64>,
    #[serde(default)]
    pub backoff_max_ms: Option<u64>,
    #[serde(default)]
    pub jitter_ms: Option<u64>,
    #[serde(default)]
    pub on_status: Option<String>,
    #[serde(default)]
    pub on_class: Option<Vec<String>>,
    #[serde(default)]
    pub strategy: Option<RetryStrategy>,
    #[serde(default)]
    pub upstream: Option<RetryLayerConfig>,
    #[serde(default)]
    pub provider: Option<RetryLayerConfig>,
    #[serde(default)]
    pub never_on_status: Option<String>,
    #[serde(default)]
    pub never_on_class: Option<Vec<String>>,
    #[serde(default)]
    pub cloudflare_challenge_cooldown_secs: Option<u64>,
    #[serde(default)]
    pub cloudflare_timeout_cooldown_secs: Option<u64>,
    #[serde(default)]
    pub transport_cooldown_secs: Option<u64>,
    /// Optional exponential backoff for cooldown penalties.
    /// When factor > 1, repeated penalties will increase cooldown up to max_secs.
    #[serde(default)]
    pub cooldown_backoff_factor: Option<u64>,
    #[serde(default)]
    pub cooldown_backoff_max_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetryStrategy {
    /// Prefer switching to another upstream on retry (default).
    #[default]
    Failover,
    /// Prefer retrying the same upstream (opt-in).
    SameUpstream,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            profile: Some(RetryProfileName::Balanced),
            max_attempts: None,
            backoff_ms: None,
            backoff_max_ms: None,
            jitter_ms: None,
            on_status: None,
            on_class: None,
            strategy: None,
            upstream: None,
            provider: None,
            never_on_status: None,
            never_on_class: None,
            cloudflare_challenge_cooldown_secs: None,
            cloudflare_timeout_cooldown_secs: None,
            transport_cooldown_secs: None,
            cooldown_backoff_factor: None,
            cooldown_backoff_max_secs: None,
        }
    }
}

impl RetryProfileName {
    pub fn defaults(self) -> ResolvedRetryConfig {
        match self {
            RetryProfileName::Balanced => ResolvedRetryConfig {
                upstream: ResolvedRetryLayerConfig {
                    max_attempts: 2,
                    backoff_ms: 200,
                    backoff_max_ms: 2_000,
                    jitter_ms: 100,
                    on_status: "429,500-599,524".to_string(),
                    on_class: vec![
                        "upstream_transport_error".to_string(),
                        "cloudflare_timeout".to_string(),
                        "cloudflare_challenge".to_string(),
                    ],
                    strategy: RetryStrategy::SameUpstream,
                },
                provider: ResolvedRetryLayerConfig {
                    max_attempts: 2,
                    backoff_ms: 0,
                    backoff_max_ms: 0,
                    jitter_ms: 0,
                    on_status: "401,403,404,408,429,500-599,524".to_string(),
                    on_class: vec!["upstream_transport_error".to_string()],
                    strategy: RetryStrategy::Failover,
                },
                never_on_status: "413,415,422".to_string(),
                never_on_class: vec!["client_error_non_retryable".to_string()],
                cloudflare_challenge_cooldown_secs: 300,
                cloudflare_timeout_cooldown_secs: 60,
                transport_cooldown_secs: 30,
                cooldown_backoff_factor: 1,
                cooldown_backoff_max_secs: 600,
            },
            RetryProfileName::SameUpstream => ResolvedRetryConfig {
                upstream: ResolvedRetryLayerConfig {
                    max_attempts: 3,
                    ..RetryProfileName::Balanced.defaults().upstream
                },
                provider: ResolvedRetryLayerConfig {
                    max_attempts: 1,
                    ..RetryProfileName::Balanced.defaults().provider
                },
                ..RetryProfileName::Balanced.defaults()
            },
            RetryProfileName::AggressiveFailover => ResolvedRetryConfig {
                upstream: ResolvedRetryLayerConfig {
                    max_attempts: 2,
                    backoff_ms: 200,
                    backoff_max_ms: 2_500,
                    jitter_ms: 150,
                    on_status: "429,500-599,524".to_string(),
                    on_class: vec![
                        "upstream_transport_error".to_string(),
                        "cloudflare_timeout".to_string(),
                        "cloudflare_challenge".to_string(),
                    ],
                    strategy: RetryStrategy::SameUpstream,
                },
                provider: ResolvedRetryLayerConfig {
                    max_attempts: 3,
                    backoff_ms: 0,
                    backoff_max_ms: 0,
                    jitter_ms: 0,
                    on_status: "401,403,404,408,429,500-599,524".to_string(),
                    on_class: vec!["upstream_transport_error".to_string()],
                    strategy: RetryStrategy::Failover,
                },
                ..RetryProfileName::Balanced.defaults()
            },
            RetryProfileName::CostPrimary => ResolvedRetryConfig {
                provider: ResolvedRetryLayerConfig {
                    max_attempts: 2,
                    ..RetryProfileName::Balanced.defaults().provider
                },
                transport_cooldown_secs: 30,
                cooldown_backoff_factor: 2,
                cooldown_backoff_max_secs: 900,
                ..RetryProfileName::Balanced.defaults()
            },
        }
    }
}

impl RetryConfig {
    pub fn resolve(&self) -> ResolvedRetryConfig {
        let mut out = self
            .profile
            .unwrap_or(RetryProfileName::Balanced)
            .defaults();

        // Legacy flat fields map to the upstream layer by default, so existing configs that only
        // tuned `max_attempts` / `on_status` keep a similar "retry the current upstream" behavior.
        // If `upstream` is explicitly configured, it always takes precedence.
        if self.upstream.is_none() {
            if let Some(v) = self.max_attempts {
                out.upstream.max_attempts = v;
            }
            if let Some(v) = self.backoff_ms {
                out.upstream.backoff_ms = v;
            }
            if let Some(v) = self.backoff_max_ms {
                out.upstream.backoff_max_ms = v;
            }
            if let Some(v) = self.jitter_ms {
                out.upstream.jitter_ms = v;
            }
            if let Some(v) = self.on_status.as_deref() {
                out.upstream.on_status = v.to_string();
            }
            if let Some(v) = self.on_class.as_ref() {
                out.upstream.on_class = v.clone();
            }
            if let Some(v) = self.strategy {
                out.upstream.strategy = v;
            }
        }

        if let Some(layer) = self.upstream.as_ref() {
            if let Some(v) = layer.max_attempts {
                out.upstream.max_attempts = v;
            }
            if let Some(v) = layer.backoff_ms {
                out.upstream.backoff_ms = v;
            }
            if let Some(v) = layer.backoff_max_ms {
                out.upstream.backoff_max_ms = v;
            }
            if let Some(v) = layer.jitter_ms {
                out.upstream.jitter_ms = v;
            }
            if let Some(v) = layer.on_status.as_deref() {
                out.upstream.on_status = v.to_string();
            }
            if let Some(v) = layer.on_class.as_ref() {
                out.upstream.on_class = v.clone();
            }
            if let Some(v) = layer.strategy {
                out.upstream.strategy = v;
            }
        }
        if let Some(layer) = self.provider.as_ref() {
            if let Some(v) = layer.max_attempts {
                out.provider.max_attempts = v;
            }
            if let Some(v) = layer.backoff_ms {
                out.provider.backoff_ms = v;
            }
            if let Some(v) = layer.backoff_max_ms {
                out.provider.backoff_max_ms = v;
            }
            if let Some(v) = layer.jitter_ms {
                out.provider.jitter_ms = v;
            }
            if let Some(v) = layer.on_status.as_deref() {
                out.provider.on_status = v.to_string();
            }
            if let Some(v) = layer.on_class.as_ref() {
                out.provider.on_class = v.clone();
            }
            if let Some(v) = layer.strategy {
                out.provider.strategy = v;
            }
        }
        if let Some(v) = self.never_on_status.as_deref() {
            out.never_on_status = v.to_string();
        }
        if let Some(v) = self.never_on_class.as_ref() {
            out.never_on_class = v.clone();
        }
        if let Some(v) = self.cloudflare_challenge_cooldown_secs {
            out.cloudflare_challenge_cooldown_secs = v;
        }
        if let Some(v) = self.cloudflare_timeout_cooldown_secs {
            out.cloudflare_timeout_cooldown_secs = v;
        }
        if let Some(v) = self.transport_cooldown_secs {
            out.transport_cooldown_secs = v;
        }
        if let Some(v) = self.cooldown_backoff_factor {
            out.cooldown_backoff_factor = v;
        }
        if let Some(v) = self.cooldown_backoff_max_secs {
            out.cooldown_backoff_max_secs = v;
        }

        out
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceViewV2 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ServiceControlProfile>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderConfigV2>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroupMemberRefV2 {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "endpoints")]
    pub endpoint_names: Vec<String>,
    #[serde(default)]
    pub preferred: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoutingCandidate {
    pub name: String,
    pub alias: Option<String>,
    pub level: u8,
    pub enabled: bool,
    pub active: bool,
    pub upstreams: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceRoutingExplanation {
    pub active_config: Option<String>,
    pub mode: &'static str,
    pub eligible_configs: Vec<RoutingCandidate>,
    pub fallback_config: Option<RoutingCandidate>,
}

fn merge_string_maps(
    provider_values: &BTreeMap<String, String>,
    endpoint_values: &BTreeMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = provider_values
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<HashMap<_, _>>();
    for (key, value) in endpoint_values {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

fn merge_bool_maps(
    provider_values: &BTreeMap<String, bool>,
    endpoint_values: &BTreeMap<String, bool>,
) -> HashMap<String, bool> {
    let mut merged = provider_values
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect::<HashMap<_, _>>();
    for (key, value) in endpoint_values {
        merged.insert(key.clone(), *value);
    }
    merged
}

fn compile_service_view_v2(
    service_name: &str,
    view: &ServiceViewV2,
) -> Result<ServiceConfigManager> {
    if let Some(active_group) = view.active_group.as_deref()
        && !view.groups.contains_key(active_group)
    {
        anyhow::bail!(
            "[{service_name}] active_group '{}' does not exist in groups",
            active_group
        );
    }

    let mut configs = HashMap::new();
    for (group_name, group) in &view.groups {
        let mut members = group.members.iter().enumerate().collect::<Vec<_>>();
        members.sort_by_key(|(idx, member)| (!member.preferred, *idx));

        let mut upstreams = Vec::new();
        for (_, member) in members {
            let provider = view.providers.get(&member.provider).with_context(|| {
                format!(
                    "[{service_name}] group '{}' references missing provider '{}'",
                    group_name, member.provider
                )
            })?;

            if !provider.enabled {
                continue;
            }
            if provider.endpoints.is_empty() {
                anyhow::bail!(
                    "[{service_name}] provider '{}' has no endpoints",
                    member.provider
                );
            }

            let endpoint_names = if member.endpoint_names.is_empty() {
                provider.endpoints.keys().cloned().collect::<Vec<_>>()
            } else {
                member.endpoint_names.clone()
            };

            for endpoint_name in endpoint_names {
                let endpoint = provider.endpoints.get(&endpoint_name).with_context(|| {
                    format!(
                        "[{service_name}] group '{}' references missing endpoint '{}.{}'",
                        group_name, member.provider, endpoint_name
                    )
                })?;
                if !endpoint.enabled {
                    continue;
                }

                upstreams.push(UpstreamConfig {
                    base_url: endpoint.base_url.clone(),
                    auth: provider.auth.clone(),
                    tags: merge_string_maps(&provider.tags, &endpoint.tags),
                    supported_models: merge_bool_maps(
                        &provider.supported_models,
                        &endpoint.supported_models,
                    ),
                    model_mapping: merge_string_maps(
                        &provider.model_mapping,
                        &endpoint.model_mapping,
                    ),
                });
            }
        }

        configs.insert(
            group_name.clone(),
            ServiceConfig {
                name: group_name.clone(),
                alias: group.alias.clone(),
                enabled: group.enabled,
                level: group.level.clamp(1, 10),
                upstreams,
            },
        );
    }

    let mgr = ServiceConfigManager {
        active: view.active_group.clone(),
        default_profile: view.default_profile.clone(),
        profiles: view.profiles.clone(),
        configs,
    };
    validate_service_profiles(service_name, &mgr)?;
    Ok(mgr)
}

pub fn compile_v2_to_runtime(v2: &ProxyConfigV2) -> Result<ProxyConfig> {
    if v2.version != 2 {
        anyhow::bail!("unsupported v2 config version: {}", v2.version);
    }

    Ok(ProxyConfig {
        version: Some(v2.version),
        codex: compile_service_view_v2("codex", &v2.codex)?,
        claude: compile_service_view_v2("claude", &v2.claude)?,
        retry: v2.retry.clone(),
        notify: v2.notify.clone(),
        default_service: v2.default_service,
        ui: v2.ui.clone(),
    })
}

fn migrate_service_manager_to_v2(mgr: &ServiceConfigManager) -> ServiceViewV2 {
    let mut providers = BTreeMap::new();
    let mut groups = BTreeMap::new();

    let mut group_names = mgr.configs.keys().cloned().collect::<Vec<_>>();
    group_names.sort();

    for group_name in group_names {
        let Some(svc) = mgr.configs.get(&group_name) else {
            continue;
        };

        let mut members: Vec<GroupMemberRefV2> = Vec::new();
        for (idx, upstream) in svc.upstreams.iter().enumerate() {
            let provider_name = format!("{}__u{:02}", group_name, idx + 1);
            let endpoint_name = "default".to_string();

            let mut endpoints = BTreeMap::new();
            endpoints.insert(
                endpoint_name.clone(),
                ProviderEndpointV2 {
                    base_url: upstream.base_url.clone(),
                    enabled: true,
                    tags: upstream
                        .tags
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                    supported_models: upstream
                        .supported_models
                        .iter()
                        .map(|(k, v)| (k.clone(), *v))
                        .collect(),
                    model_mapping: upstream
                        .model_mapping
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                },
            );

            providers.insert(
                provider_name.clone(),
                ProviderConfigV2 {
                    alias: upstream.tags.get("provider_id").cloned(),
                    enabled: true,
                    auth: upstream.auth.clone(),
                    tags: BTreeMap::new(),
                    supported_models: BTreeMap::new(),
                    model_mapping: BTreeMap::new(),
                    endpoints,
                },
            );

            members.push(GroupMemberRefV2 {
                provider: provider_name,
                endpoint_names: vec![endpoint_name],
                preferred: false,
            });
        }

        groups.insert(
            group_name.clone(),
            GroupConfigV2 {
                alias: svc.alias.clone(),
                enabled: svc.enabled,
                level: svc.level,
                members,
            },
        );
    }

    ServiceViewV2 {
        active_group: mgr.active.clone(),
        default_profile: mgr.default_profile.clone(),
        profiles: mgr.profiles.clone(),
        providers,
        groups,
    }
}

pub fn migrate_legacy_to_v2(old: &ProxyConfig) -> ProxyConfigV2 {
    ProxyConfigV2 {
        version: 2,
        codex: migrate_service_manager_to_v2(&old.codex),
        claude: migrate_service_manager_to_v2(&old.claude),
        retry: old.retry.clone(),
        notify: old.notify.clone(),
        default_service: old.default_service,
        ui: old.ui.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderBucketKey {
    hint: String,
    auth_token: Option<String>,
    auth_token_env: Option<String>,
    api_key: Option<String>,
    api_key_env: Option<String>,
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct EndpointBucketKey {
    enabled: bool,
    base_url: String,
    tags: Vec<(String, String)>,
    supported_models: Vec<(String, bool)>,
    model_mapping: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct EndpointCompactBuild {
    key: EndpointBucketKey,
    upstream: UpstreamConfig,
    original_names: Vec<String>,
}

#[derive(Debug, Clone)]
struct ProviderCompactBuild {
    alias: Option<String>,
    auth: UpstreamAuth,
    enabled: bool,
    empty_provider_tags: BTreeMap<String, String>,
    empty_provider_supported_models: BTreeMap<String, bool>,
    empty_provider_model_mapping: BTreeMap<String, String>,
    endpoints: Vec<EndpointCompactBuild>,
    endpoint_index: HashMap<EndpointBucketKey, usize>,
    endpoint_names: HashMap<EndpointBucketKey, String>,
}

#[derive(Debug, Clone)]
struct GroupOccurrence {
    provider: String,
    endpoint_name: String,
    preferred: bool,
}

fn hash_string_map_to_btree(values: &HashMap<String, String>) -> BTreeMap<String, String> {
    values.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn hash_bool_map_to_btree(values: &HashMap<String, bool>) -> BTreeMap<String, bool> {
    values.iter().map(|(k, v)| (k.clone(), *v)).collect()
}

fn string_map_without_common(
    values: &HashMap<String, String>,
    common: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    values
        .iter()
        .filter(|(key, value)| common.get(*key) != Some(*value))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn bool_map_without_common(
    values: &HashMap<String, bool>,
    common: &BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    values
        .iter()
        .filter(|(key, value)| common.get(*key) != Some(*value))
        .map(|(k, v)| (k.clone(), *v))
        .collect()
}

fn common_string_entries(
    upstreams: &[UpstreamConfig],
    selector: fn(&UpstreamConfig) -> &HashMap<String, String>,
) -> BTreeMap<String, String> {
    let Some(first) = upstreams.first() else {
        return BTreeMap::new();
    };
    let mut common = hash_string_map_to_btree(selector(first));
    common.retain(|key, value| {
        upstreams
            .iter()
            .skip(1)
            .all(|upstream| selector(upstream).get(key) == Some(value))
    });
    common
}

fn common_bool_entries(
    upstreams: &[UpstreamConfig],
    selector: fn(&UpstreamConfig) -> &HashMap<String, bool>,
) -> BTreeMap<String, bool> {
    let Some(first) = upstreams.first() else {
        return BTreeMap::new();
    };
    let mut common = hash_bool_map_to_btree(selector(first));
    common.retain(|key, value| {
        upstreams
            .iter()
            .skip(1)
            .all(|upstream| selector(upstream).get(key) == Some(value))
    });
    common
}

fn sanitize_schema_key(raw: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.trim().chars() {
        let normalized = ch.to_ascii_lowercase();
        if normalized.is_ascii_alphanumeric() {
            out.push(normalized);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

fn looks_generated_provider_name(name: &str) -> bool {
    if let Some((prefix, suffix)) = name.rsplit_once("__u") {
        !prefix.is_empty() && suffix.len() == 2 && suffix.chars().all(|ch| ch.is_ascii_digit())
    } else {
        false
    }
}

fn looks_default_endpoint_name(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    if lower == "default" {
        return true;
    }
    lower
        .strip_prefix("default-")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()))
}

fn env_provider_hint(auth: &UpstreamAuth) -> Option<String> {
    let raw = auth
        .auth_token_env
        .as_deref()
        .or(auth.api_key_env.as_deref())?
        .trim()
        .to_ascii_lowercase();
    let mut hint = raw;
    for suffix in ["_auth_token", "_api_key", "_token", "_key"] {
        if let Some(stripped) = hint.strip_suffix(suffix) {
            hint = stripped.to_string();
            break;
        }
    }
    Some(sanitize_schema_key(&hint, "provider"))
}

fn host_provider_hint(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    let labels = host.split('.').collect::<Vec<_>>();
    let raw = if labels.len() >= 2 {
        labels[labels.len() - 2]
    } else {
        host
    };
    Some(sanitize_schema_key(raw, "provider"))
}

fn subdomain_or_host_hint(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let host = url.host_str()?;
    let labels = host.split('.').collect::<Vec<_>>();
    if labels.len() >= 3 {
        let first = labels[0].to_ascii_lowercase();
        if !matches!(first.as_str(), "api" | "www" | "gateway") {
            return Some(sanitize_schema_key(&first, "endpoint"));
        }
    }
    host_provider_hint(base_url).map(|hint| sanitize_schema_key(&hint, "endpoint"))
}

fn path_endpoint_hint(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let segment = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .filter(|segment| !matches!(*segment, "v1" | "v2" | "api"))
        .next_back()?;
    Some(sanitize_schema_key(segment, "endpoint"))
}

fn allocate_unique_name(base: &str, counters: &mut HashMap<String, usize>) -> String {
    let entry = counters.entry(base.to_string()).or_insert(0);
    *entry += 1;
    if *entry == 1 {
        base.to_string()
    } else {
        format!("{base}-{}", *entry)
    }
}

fn provider_name_hint(
    original_name: &str,
    provider: &ProviderConfigV2,
) -> (String, Option<String>) {
    let mut raw_hint = None;
    if !original_name.trim().is_empty() && !looks_generated_provider_name(original_name) {
        raw_hint = Some(original_name.trim().to_string());
    }
    if raw_hint.is_none() {
        raw_hint = provider
            .alias
            .clone()
            .filter(|alias| !alias.trim().is_empty());
    }
    if raw_hint.is_none() {
        raw_hint = provider
            .tags
            .get("provider_id")
            .cloned()
            .filter(|value| !value.trim().is_empty());
    }
    if raw_hint.is_none() {
        raw_hint = provider
            .endpoints
            .values()
            .find_map(|endpoint| endpoint.tags.get("provider_id").cloned())
            .filter(|value| !value.trim().is_empty());
    }
    if raw_hint.is_none() {
        raw_hint = env_provider_hint(&provider.auth);
    }
    if raw_hint.is_none() {
        raw_hint = provider
            .endpoints
            .values()
            .find_map(|endpoint| host_provider_hint(&endpoint.base_url));
    }

    let raw_hint = raw_hint.unwrap_or_else(|| "provider".to_string());
    let slug = sanitize_schema_key(&raw_hint, "provider");
    let alias = if raw_hint == slug {
        None
    } else {
        Some(raw_hint)
    };
    (slug, alias)
}

fn endpoint_name_hint(endpoint: &EndpointCompactBuild, total: usize) -> String {
    if total == 1 {
        return "default".to_string();
    }

    if let Some(name) = endpoint
        .original_names
        .iter()
        .find(|name| !name.trim().is_empty() && !looks_default_endpoint_name(name))
    {
        return sanitize_schema_key(name, "endpoint");
    }
    if let Some(region) = endpoint.upstream.tags.get("region") {
        return sanitize_schema_key(region, "endpoint");
    }
    if let Some(hint) = subdomain_or_host_hint(&endpoint.upstream.base_url) {
        return hint;
    }
    if let Some(hint) = path_endpoint_hint(&endpoint.upstream.base_url) {
        return hint;
    }
    "endpoint".to_string()
}

fn endpoint_bucket_key(
    endpoint: &ProviderEndpointV2,
    effective: &UpstreamConfig,
) -> EndpointBucketKey {
    let mut tags = effective
        .tags
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Vec<_>>();
    tags.sort();
    let mut supported_models = effective
        .supported_models
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect::<Vec<_>>();
    supported_models.sort_by(|a, b| a.0.cmp(&b.0));
    let mut model_mapping = effective
        .model_mapping
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Vec<_>>();
    model_mapping.sort_by(|a, b| a.0.cmp(&b.0));

    EndpointBucketKey {
        enabled: endpoint.enabled,
        base_url: effective.base_url.clone(),
        tags,
        supported_models,
        model_mapping,
    }
}

fn effective_upstream_for_endpoint(
    provider: &ProviderConfigV2,
    endpoint: &ProviderEndpointV2,
) -> UpstreamConfig {
    UpstreamConfig {
        base_url: endpoint.base_url.clone(),
        auth: provider.auth.clone(),
        tags: merge_string_maps(&provider.tags, &endpoint.tags),
        supported_models: merge_bool_maps(&provider.supported_models, &endpoint.supported_models),
        model_mapping: merge_string_maps(&provider.model_mapping, &endpoint.model_mapping),
    }
}

fn compact_service_view_v2(view: &ServiceViewV2) -> Result<ServiceViewV2> {
    let mut provider_name_counters = HashMap::new();
    let mut bucket_lookup = HashMap::<ProviderBucketKey, String>::new();
    let mut provider_lookup = HashMap::<String, String>::new();
    let mut endpoint_lookup = HashMap::<(String, String), (String, EndpointBucketKey)>::new();
    let mut builds = BTreeMap::<String, ProviderCompactBuild>::new();

    for (original_provider_name, provider) in &view.providers {
        let (hint, alias) = provider_name_hint(original_provider_name, provider);
        let bucket_key = ProviderBucketKey {
            hint: hint.clone(),
            auth_token: provider.auth.auth_token.clone(),
            auth_token_env: provider.auth.auth_token_env.clone(),
            api_key: provider.auth.api_key.clone(),
            api_key_env: provider.auth.api_key_env.clone(),
            enabled: provider.enabled,
        };

        let canonical_provider_name = if let Some(existing) = bucket_lookup.get(&bucket_key) {
            existing.clone()
        } else {
            let allocated = allocate_unique_name(&hint, &mut provider_name_counters);
            bucket_lookup.insert(bucket_key, allocated.clone());
            allocated
        };
        provider_lookup.insert(
            original_provider_name.clone(),
            canonical_provider_name.clone(),
        );

        let build = builds
            .entry(canonical_provider_name.clone())
            .or_insert_with(|| ProviderCompactBuild {
                alias: alias.clone(),
                auth: provider.auth.clone(),
                enabled: provider.enabled,
                empty_provider_tags: provider.tags.clone(),
                empty_provider_supported_models: provider.supported_models.clone(),
                empty_provider_model_mapping: provider.model_mapping.clone(),
                endpoints: Vec::new(),
                endpoint_index: HashMap::new(),
                endpoint_names: HashMap::new(),
            });
        if build.alias.is_none() {
            build.alias = alias;
        }

        if provider.endpoints.is_empty() {
            continue;
        }

        for (original_endpoint_name, endpoint) in &provider.endpoints {
            let effective = effective_upstream_for_endpoint(provider, endpoint);
            let key = endpoint_bucket_key(endpoint, &effective);
            let index = if let Some(index) = build.endpoint_index.get(&key) {
                *index
            } else {
                let index = build.endpoints.len();
                build.endpoints.push(EndpointCompactBuild {
                    key: key.clone(),
                    upstream: effective.clone(),
                    original_names: Vec::new(),
                });
                build.endpoint_index.insert(key.clone(), index);
                index
            };
            build.endpoints[index]
                .original_names
                .push(original_endpoint_name.clone());
            endpoint_lookup.insert(
                (
                    original_provider_name.clone(),
                    original_endpoint_name.clone(),
                ),
                (canonical_provider_name.clone(), key),
            );
        }
    }

    for build in builds.values_mut() {
        let mut counters = HashMap::new();
        let total = build.endpoints.len();
        for endpoint in &build.endpoints {
            let base = endpoint_name_hint(endpoint, total);
            let name = allocate_unique_name(&base, &mut counters);
            build.endpoint_names.insert(endpoint.key.clone(), name);
        }
    }

    let mut providers = BTreeMap::new();
    for (provider_name, build) in &builds {
        if build.endpoints.is_empty() {
            providers.insert(
                provider_name.clone(),
                ProviderConfigV2 {
                    alias: build
                        .alias
                        .clone()
                        .filter(|alias| sanitize_schema_key(alias, "provider") != *provider_name),
                    enabled: build.enabled,
                    auth: build.auth.clone(),
                    tags: build.empty_provider_tags.clone(),
                    supported_models: build.empty_provider_supported_models.clone(),
                    model_mapping: build.empty_provider_model_mapping.clone(),
                    endpoints: BTreeMap::new(),
                },
            );
            continue;
        }

        let upstreams = build
            .endpoints
            .iter()
            .map(|endpoint| endpoint.upstream.clone())
            .collect::<Vec<_>>();
        let common_tags = common_string_entries(&upstreams, |upstream| &upstream.tags);
        let common_supported_models =
            common_bool_entries(&upstreams, |upstream| &upstream.supported_models);
        let common_model_mapping =
            common_string_entries(&upstreams, |upstream| &upstream.model_mapping);

        let mut endpoints = BTreeMap::new();
        for endpoint in &build.endpoints {
            let endpoint_name = build
                .endpoint_names
                .get(&endpoint.key)
                .expect("endpoint name should exist")
                .clone();
            endpoints.insert(
                endpoint_name,
                ProviderEndpointV2 {
                    base_url: endpoint.upstream.base_url.clone(),
                    enabled: endpoint.key.enabled,
                    tags: string_map_without_common(&endpoint.upstream.tags, &common_tags),
                    supported_models: bool_map_without_common(
                        &endpoint.upstream.supported_models,
                        &common_supported_models,
                    ),
                    model_mapping: string_map_without_common(
                        &endpoint.upstream.model_mapping,
                        &common_model_mapping,
                    ),
                },
            );
        }

        providers.insert(
            provider_name.clone(),
            ProviderConfigV2 {
                alias: build
                    .alias
                    .clone()
                    .filter(|alias| sanitize_schema_key(alias, "provider") != *provider_name),
                enabled: build.enabled,
                auth: build.auth.clone(),
                tags: common_tags,
                supported_models: common_supported_models,
                model_mapping: common_model_mapping,
                endpoints,
            },
        );
    }

    let mut groups = BTreeMap::new();
    for (group_name, group) in &view.groups {
        let mut occurrences = Vec::new();
        for member in &group.members {
            let provider = view.providers.get(&member.provider).with_context(|| {
                format!(
                    "group '{}' references missing provider '{}'",
                    group_name, member.provider
                )
            })?;
            let endpoint_names = if member.endpoint_names.is_empty() {
                provider.endpoints.keys().cloned().collect::<Vec<_>>()
            } else {
                member.endpoint_names.clone()
            };

            for endpoint_name in endpoint_names {
                let (canonical_provider, endpoint_key) = endpoint_lookup
                    .get(&(member.provider.clone(), endpoint_name.clone()))
                    .with_context(|| {
                        format!(
                            "group '{}' references missing endpoint '{}.{}'",
                            group_name, member.provider, endpoint_name
                        )
                    })?;
                let mapped_endpoint_name = builds
                    .get(canonical_provider)
                    .and_then(|build| build.endpoint_names.get(endpoint_key))
                    .cloned()
                    .with_context(|| {
                        format!(
                            "group '{}' cannot map endpoint '{}.{}'",
                            group_name, member.provider, endpoint_name
                        )
                    })?;
                occurrences.push(GroupOccurrence {
                    provider: canonical_provider.clone(),
                    endpoint_name: mapped_endpoint_name,
                    preferred: member.preferred,
                });
            }
        }

        let mut members: Vec<GroupMemberRefV2> = Vec::new();
        for occurrence in occurrences {
            if let Some(last) = members.last_mut()
                && last.provider == occurrence.provider
                && last.preferred == occurrence.preferred
            {
                last.endpoint_names.push(occurrence.endpoint_name);
            } else {
                members.push(GroupMemberRefV2 {
                    provider: occurrence.provider,
                    endpoint_names: vec![occurrence.endpoint_name],
                    preferred: occurrence.preferred,
                });
            }
        }

        groups.insert(
            group_name.clone(),
            GroupConfigV2 {
                alias: group.alias.clone(),
                enabled: group.enabled,
                level: group.level,
                members,
            },
        );
    }

    Ok(ServiceViewV2 {
        active_group: view.active_group.clone(),
        default_profile: view.default_profile.clone(),
        profiles: view.profiles.clone(),
        providers,
        groups,
    })
}

pub fn compact_v2_config(v2: &ProxyConfigV2) -> Result<ProxyConfigV2> {
    Ok(ProxyConfigV2 {
        version: 2,
        codex: compact_service_view_v2(&v2.codex)?,
        claude: compact_service_view_v2(&v2.claude)?,
        retry: v2.retry.clone(),
        notify: v2.notify.clone(),
        default_service: v2.default_service,
        ui: v2.ui.clone(),
    })
}

fn routing_candidate(
    name: &str,
    svc: &ServiceConfig,
    active_name: Option<&str>,
) -> RoutingCandidate {
    RoutingCandidate {
        name: name.to_string(),
        alias: svc.alias.clone(),
        level: svc.level.clamp(1, 10),
        enabled: svc.enabled,
        active: active_name.is_some_and(|active| active == name),
        upstreams: svc.upstreams.len(),
    }
}

fn active_or_first_config(mgr: &ServiceConfigManager) -> Option<(String, &ServiceConfig)> {
    if let Some(active_name) = mgr.active.as_deref()
        && let Some(svc) = mgr.configs.get(active_name)
    {
        return Some((active_name.to_string(), svc));
    }

    mgr.configs
        .iter()
        .min_by_key(|(name, _)| *name)
        .map(|(name, svc)| (name.clone(), svc))
}

pub fn explain_service_routing(mgr: &ServiceConfigManager) -> ServiceRoutingExplanation {
    let active_name = mgr.active.as_deref();
    let mut eligible = mgr
        .configs
        .iter()
        .filter(|(name, svc)| {
            !svc.upstreams.is_empty()
                && (svc.enabled || active_name.is_some_and(|active| active == name.as_str()))
        })
        .map(|(name, svc)| routing_candidate(name, svc, active_name))
        .collect::<Vec<_>>();

    let has_multi_level = {
        let mut levels = eligible
            .iter()
            .map(|candidate| candidate.level)
            .collect::<Vec<_>>();
        levels.sort_unstable();
        levels.dedup();
        levels.len() > 1
    };

    if !has_multi_level {
        eligible.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(active) = active_name
            && let Some(pos) = eligible
                .iter()
                .position(|candidate| candidate.name == active)
        {
            let item = eligible.remove(pos);
            eligible.insert(0, item);
        }

        if !eligible.is_empty() {
            return ServiceRoutingExplanation {
                active_config: mgr.active.clone(),
                mode: "single_level_multi",
                eligible_configs: eligible,
                fallback_config: None,
            };
        }

        return ServiceRoutingExplanation {
            active_config: mgr.active.clone(),
            mode: if active_or_first_config(mgr).is_some() {
                "single_level_fallback_active_config"
            } else {
                "single_level_empty"
            },
            eligible_configs: Vec::new(),
            fallback_config: active_or_first_config(mgr)
                .map(|(name, svc)| routing_candidate(&name, svc, active_name)),
        };
    }

    eligible.sort_by(|a, b| {
        a.level
            .cmp(&b.level)
            .then_with(|| b.active.cmp(&a.active))
            .then_with(|| a.name.cmp(&b.name))
    });

    if !eligible.is_empty() {
        return ServiceRoutingExplanation {
            active_config: mgr.active.clone(),
            mode: "multi_level",
            eligible_configs: eligible,
            fallback_config: None,
        };
    }

    ServiceRoutingExplanation {
        active_config: mgr.active.clone(),
        mode: if active_or_first_config(mgr).is_some() {
            "multi_level_fallback_active_config"
        } else {
            "multi_level_empty"
        },
        eligible_configs: Vec::new(),
        fallback_config: active_or_first_config(mgr)
            .map(|(name, svc)| routing_candidate(&name, svc, active_name)),
    }
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
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    #[test]
    fn infer_env_key_from_auth_json_single_key() {
        let json = serde_json::json!({
            "OPENAI_API_KEY": "sk-test-123",
            "tokens": null
        });
        let auth = Some(json);
        let inferred = infer_env_key_from_auth_json(&auth);
        assert!(inferred.is_some());
        let (key, value) = inferred.unwrap();
        assert_eq!(key, "OPENAI_API_KEY");
        assert_eq!(value, "sk-test-123");
    }

    #[test]
    fn infer_env_key_from_auth_json_multiple_keys() {
        let json = serde_json::json!({
            "OPENAI_API_KEY": "sk-test-1",
            "MISTRAL_API_KEY": "sk-test-2"
        });
        let auth = Some(json);
        let inferred = infer_env_key_from_auth_json(&auth);
        assert!(inferred.is_none());
    }

    #[test]
    fn infer_env_key_from_auth_json_none() {
        let json = serde_json::json!({
            "tokens": {
                "id_token": "xxx"
            }
        });
        let auth = Some(json);
        let inferred = infer_env_key_from_auth_json(&auth);
        assert!(inferred.is_none());
    }

    struct ScopedEnv {
        saved: Vec<(String, Option<String>)>,
    }

    impl ScopedEnv {
        fn new() -> Self {
            Self { saved: Vec::new() }
        }

        unsafe fn set(&mut self, key: &str, value: &Path) {
            self.saved.push((key.to_string(), std::env::var(key).ok()));
            unsafe { std::env::set_var(key, value) };
        }

        unsafe fn set_str(&mut self, key: &str, value: &str) {
            self.saved.push((key.to_string(), std::env::var(key).ok()));
            unsafe { std::env::set_var(key, value) };
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            for (key, old) in self.saved.drain(..).rev() {
                unsafe {
                    match old {
                        Some(v) => std::env::set_var(&key, v),
                        None => std::env::remove_var(&key),
                    }
                }
            }
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        match LOCK.get_or_init(|| Mutex::new(())).lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        }
    }

    struct TestEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        _env: ScopedEnv,
        home: PathBuf,
    }

    fn setup_temp_codex_home() -> TestEnv {
        let lock = env_lock();
        let mut dir = std::env::temp_dir();
        let suffix = format!("codex-helper-test-{}", uuid::Uuid::new_v4());
        dir.push(suffix);
        std::fs::create_dir_all(&dir).expect("create temp codex home");
        let mut scoped = ScopedEnv::new();
        let proxy_home = dir.join(".codex-helper");
        std::fs::create_dir_all(&proxy_home).expect("create temp proxy home");
        unsafe {
            scoped.set("CODEX_HELPER_HOME", &proxy_home);
            scoped.set("CODEX_HOME", &dir);
            // 将 HOME 也指向该目录，确保 proxy_home_dir()/config.json 也被隔离在测试目录中。
            scoped.set("HOME", &dir);
            // Windows: dirs::home_dir() prefers USERPROFILE.
            scoped.set("USERPROFILE", &dir);
            // 避免本机真实环境变量（例如 OPENAI_API_KEY）影响测试断言。
            scoped.set_str("OPENAI_API_KEY", "");
            scoped.set_str("MISTRAL_API_KEY", "");
            scoped.set_str("RIGHTCODE_API_KEY", "");
            scoped.set_str("PACKYAPI_API_KEY", "");
        }
        TestEnv {
            _lock: lock,
            _env: scoped,
            home: dir,
        }
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(path, content).expect("write test file");
    }

    #[test]
    fn load_config_prefers_toml_over_json() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async move {
            let dir = super::proxy_home_dir();
            let json_path = dir.join("config.json");
            let toml_path = dir.join("config.toml");

            // JSON sets notify.enabled=false
            write_file(&json_path, r#"{"version":1,"notify":{"enabled":false}}"#);

            // TOML overrides notify.enabled=true
            write_file(
                &toml_path,
                r#"
version = 1

[notify]
enabled = true
"#,
            );

            let cfg = super::load_config().await.expect("load_config");
            assert!(
                cfg.notify.enabled,
                "expected config.toml to take precedence over config.json (home={:?})",
                home
            );
        });
    }

    #[test]
    fn load_config_toml_allows_missing_service_name_and_infers_from_key() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async move {
            let dir = super::proxy_home_dir();
            let toml_path = dir.join("config.toml");
            write_file(
                &toml_path,
                r#"
version = 1

[codex]
active = "right"

[codex.configs.right]
# name omitted on purpose

[[codex.configs.right.upstreams]]
base_url = "https://www.right.codes/codex/v1"
[codex.configs.right.upstreams.auth]
auth_token_env = "RIGHTCODE_API_KEY"
"#,
            );

            let cfg = super::load_config().await.expect("load_config");
            let svc = cfg
                .codex
                .configs
                .get("right")
                .expect("codex config 'right'");
            assert_eq!(
                svc.name, "right",
                "expected ServiceConfig.name to default to the map key (home={:?})",
                home
            );
        });
    }

    #[test]
    fn save_config_overwrites_existing_toml_and_updates_backup() {
        let _env = setup_temp_codex_home();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async move {
            let dir = super::proxy_home_dir();
            let toml_path = dir.join("config.toml");
            let backup_path = dir.join("config.toml.bak");

            let mut cfg = ProxyConfig::default();
            cfg.notify.enabled = true;
            super::save_config(&cfg).await.expect("first save_config");

            let first_text = std::fs::read_to_string(&toml_path).expect("read first config.toml");
            assert!(first_text.contains("enabled = true"));
            assert!(
                !backup_path.exists(),
                "first save should not create backup without an existing file"
            );

            cfg.notify.enabled = false;
            super::save_config(&cfg).await.expect("second save_config");

            let second_text = std::fs::read_to_string(&toml_path).expect("read second config.toml");
            assert!(second_text.contains("enabled = false"));

            let backup_text = std::fs::read_to_string(&backup_path).expect("read config.toml.bak");
            assert!(
                backup_text.contains("enabled = true"),
                "backup should preserve the previous config contents"
            );
        });
    }

    #[test]
    fn init_config_toml_inserts_codex_bootstrap_when_available() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();

        // Provide a minimal Codex config that bootstrap_from_codex can parse.
        write_file(
            &home.join("config.toml"),
            r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#,
        );
        write_file(
            &home.join("auth.json"),
            r#"{ "RIGHTCODE_API_KEY": "sk-test-123" }"#,
        );

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async move {
            let path = super::init_config_toml(true, true)
                .await
                .expect("init_config_toml");
            let text = std::fs::read_to_string(&path).expect("read config.toml");
            assert!(
                text.contains("\n[codex]\n"),
                "expected init to insert a real [codex] block (path={:?})",
                path
            );
            assert!(
                text.contains("active = \"right\""),
                "expected imported active config to be present"
            );
            assert!(
                text.contains("\n[retry]\n") && text.contains("profile = \"balanced\""),
                "expected retry.profile default to be visible"
            );
        });
    }

    #[test]
    fn init_config_toml_can_skip_codex_bootstrap_with_no_import() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();

        // Even if Codex config exists, no_import should not insert the real [codex] block.
        write_file(
            &home.join("config.toml"),
            r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#,
        );
        write_file(
            &home.join("auth.json"),
            r#"{ "RIGHTCODE_API_KEY": "sk-test-123" }"#,
        );

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async move {
            let path = super::init_config_toml(true, false)
                .await
                .expect("init_config_toml");
            let text = std::fs::read_to_string(&path).expect("read config.toml");
            assert!(
                !text.contains("\n[codex]\n"),
                "expected no_import to skip inserting a real [codex] block"
            );
            // But the template still contains the commented example.
            assert!(text.contains("# [codex]"));
        });
    }

    #[test]
    fn retry_profile_defaults_to_balanced_when_unset() {
        let cfg = RetryConfig::default();
        let resolved = cfg.resolve();
        assert_eq!(resolved.upstream.strategy, RetryStrategy::SameUpstream);
        assert_eq!(resolved.upstream.max_attempts, 2);
        assert_eq!(resolved.upstream.backoff_ms, 200);
        assert_eq!(resolved.upstream.backoff_max_ms, 2_000);
        assert_eq!(resolved.upstream.jitter_ms, 100);
        assert_eq!(resolved.upstream.on_status, "429,500-599,524");
        assert!(
            resolved
                .upstream
                .on_class
                .iter()
                .any(|c| c == "upstream_transport_error")
        );

        assert_eq!(resolved.provider.strategy, RetryStrategy::Failover);
        assert_eq!(resolved.provider.max_attempts, 2);
        assert_eq!(
            resolved.provider.on_status,
            "401,403,404,408,429,500-599,524"
        );
        assert_eq!(resolved.never_on_status, "413,415,422");
        assert!(
            resolved
                .never_on_class
                .iter()
                .any(|c| c == "client_error_non_retryable")
        );
        assert_eq!(resolved.cloudflare_challenge_cooldown_secs, 300);
        assert_eq!(resolved.cloudflare_timeout_cooldown_secs, 60);
        assert_eq!(resolved.transport_cooldown_secs, 30);
        assert_eq!(resolved.cooldown_backoff_factor, 1);
        assert_eq!(resolved.cooldown_backoff_max_secs, 600);
    }

    #[test]
    fn retry_profile_cost_primary_sets_probe_back_defaults() {
        let cfg = RetryConfig {
            profile: Some(RetryProfileName::CostPrimary),
            ..RetryConfig::default()
        };
        let resolved = cfg.resolve();
        assert_eq!(resolved.provider.strategy, RetryStrategy::Failover);
        assert_eq!(resolved.cooldown_backoff_factor, 2);
        assert_eq!(resolved.cooldown_backoff_max_secs, 900);
        assert_eq!(resolved.transport_cooldown_secs, 30);
    }

    #[test]
    fn retry_profile_aggressive_failover_enables_broader_failover_with_guardrails() {
        let cfg = RetryConfig {
            profile: Some(RetryProfileName::AggressiveFailover),
            ..RetryConfig::default()
        };
        let resolved = cfg.resolve();
        assert_eq!(resolved.provider.max_attempts, 3);
        assert_eq!(resolved.provider.strategy, RetryStrategy::Failover);
        assert_eq!(
            resolved.provider.on_status,
            "401,403,404,408,429,500-599,524"
        );
        assert_eq!(resolved.never_on_status, "413,415,422");
        assert!(
            resolved
                .never_on_class
                .iter()
                .any(|c| c == "client_error_non_retryable")
        );
    }

    #[test]
    fn retry_profile_allows_explicit_overrides() {
        let cfg = RetryConfig {
            profile: Some(RetryProfileName::SameUpstream),
            // Override profile defaults:
            max_attempts: Some(5),
            strategy: Some(RetryStrategy::Failover),
            ..RetryConfig::default()
        };
        let resolved = cfg.resolve();
        assert_eq!(resolved.upstream.max_attempts, 5);
        assert_eq!(resolved.upstream.strategy, RetryStrategy::Failover);
    }

    #[test]
    fn retry_profile_parses_from_toml_kebab_case() {
        let text = r#"
version = 1

[retry]
profile = "cost-primary"
"#;
        let cfg = toml::from_str::<ProxyConfig>(text).expect("toml parse");
        assert_eq!(cfg.retry.profile, Some(RetryProfileName::CostPrimary));
    }

    #[test]
    fn bootstrap_from_codex_with_env_key_and_auth_json() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();
        // Write config.toml with explicit env_key
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#;
        write_file(&cfg_path, config_text);

        // Write auth.json with matching RIGHTCODE_API_KEY
        let auth_path = home.join("auth.json");
        let auth_text = r#"{ "RIGHTCODE_API_KEY": "sk-test-123" }"#;
        write_file(&auth_path, auth_text);

        let mut cfg = ProxyConfig::default();
        bootstrap_from_codex(&mut cfg).expect("bootstrap_from_codex should succeed");

        assert!(!cfg.codex.configs.is_empty());
        let svc = cfg.codex.active_config().expect("active codex config");
        assert_eq!(svc.name, "right");
        assert_eq!(svc.upstreams.len(), 1);
        let up = &svc.upstreams[0];
        assert_eq!(up.base_url, "https://www.right.codes/codex/v1");
        assert!(up.auth.auth_token.is_none());
        assert_eq!(up.auth.auth_token_env.as_deref(), Some("RIGHTCODE_API_KEY"));
    }

    #[test]
    fn bootstrap_from_codex_infers_env_key_from_auth_json_when_missing() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();
        // config.toml without env_key
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
"#;
        write_file(&cfg_path, config_text);

        // auth.json with a single *_API_KEY field
        let auth_path = home.join("auth.json");
        let auth_text = r#"{ "RIGHTCODE_API_KEY": "sk-test-456" }"#;
        write_file(&auth_path, auth_text);

        let mut cfg = ProxyConfig::default();
        bootstrap_from_codex(&mut cfg).expect("bootstrap_from_codex should infer env_key");

        let svc = cfg.codex.active_config().expect("active codex config");
        assert_eq!(svc.name, "right");
        let up = &svc.upstreams[0];
        assert!(up.auth.auth_token.is_none());
        assert_eq!(up.auth.auth_token_env.as_deref(), Some("RIGHTCODE_API_KEY"));
    }

    #[test]
    fn bootstrap_from_codex_fails_when_multiple_api_keys_without_env_key() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();
        // config.toml still without env_key
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
"#;
        write_file(&cfg_path, config_text);

        // auth.json with multiple *_API_KEY fields
        let auth_path = home.join("auth.json");
        let auth_text = r#"
{
  "RIGHTCODE_API_KEY": "sk-test-1",
  "PACKYAPI_API_KEY": "sk-test-2"
}
"#;
        write_file(&auth_path, auth_text);

        let mut cfg = ProxyConfig::default();
        let err = bootstrap_from_codex(&mut cfg).expect_err("should fail to infer unique token");
        let msg = err.to_string();
        assert!(
            msg.contains("无法从 ~/.codex/auth.json 推断唯一的 `*_API_KEY` 字段"),
            "unexpected error message: {}",
            msg
        );
    }

    #[test]
    fn load_or_bootstrap_for_service_writes_proxy_config() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async move {
            // Prepare Codex CLI config and auth under CODEX_HOME/HOME
            let cfg_path = home.join("config.toml");
            let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#;
            write_file(&cfg_path, config_text);

            let auth_path = home.join("auth.json");
            let auth_text = r#"{ "RIGHTCODE_API_KEY": "sk-test-789" }"#;
            write_file(&auth_path, auth_text);

            // 确保 proxy 配置文件起始不存在
            let proxy_cfg_path = super::proxy_home_dir().join("config.json");
            let proxy_cfg_toml_path = super::proxy_home_dir().join("config.toml");
            let _ = std::fs::remove_file(&proxy_cfg_path);
            let _ = std::fs::remove_file(&proxy_cfg_toml_path);

            let cfg = super::load_or_bootstrap_for_service(ServiceKind::Codex)
                .await
                .expect("load_or_bootstrap_for_service should succeed");

            // 内存中的配置应包含 right upstream 与正确的 token
            let svc = cfg.codex.active_config().expect("active codex config");
            assert_eq!(svc.name, "right");
            assert_eq!(svc.upstreams.len(), 1);
            assert!(svc.upstreams[0].auth.auth_token.is_none());
            assert_eq!(
                svc.upstreams[0].auth.auth_token_env.as_deref(),
                Some("RIGHTCODE_API_KEY")
            );

            // 并且应已将配置写入到 proxy_home_dir()/config.toml（fresh install defaults to TOML）
            let text = std::fs::read_to_string(&proxy_cfg_toml_path)
                .expect("config.toml should be written by load_or_bootstrap");
            let text = text
                .lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n");
            let loaded: ProxyConfig =
                toml::from_str(&text).expect("config.toml should be valid ProxyConfig");
            let svc2 = loaded.codex.active_config().expect("active codex config");
            assert_eq!(svc2.name, "right");
            assert!(svc2.upstreams[0].auth.auth_token.is_none());
            assert_eq!(
                svc2.upstreams[0].auth.auth_token_env.as_deref(),
                Some("RIGHTCODE_API_KEY")
            );
        });
    }

    #[test]
    fn bootstrap_from_codex_openai_defaults_to_requires_openai_auth_and_allows_missing_token() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "openai"

[model_providers.openai]
name = "openai"
base_url = "https://api.openai.com/v1"
"#;
        write_file(&cfg_path, config_text);

        let mut cfg = ProxyConfig::default();
        bootstrap_from_codex(&mut cfg).expect("bootstrap_from_codex should succeed");

        let svc = cfg.codex.active_config().expect("active codex config");
        assert_eq!(svc.name, "openai");
        let up = &svc.upstreams[0];
        assert_eq!(up.base_url, "https://api.openai.com/v1");
        assert!(
            up.auth.auth_token.is_none(),
            "openai default requires_openai_auth=true should not force a stored token"
        );
        assert_eq!(
            up.tags.get("requires_openai_auth").map(|s| s.as_str()),
            Some("true")
        );
    }

    #[test]
    fn bootstrap_from_codex_allows_requires_openai_auth_true_for_custom_provider() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "packycode"

[model_providers.packycode]
name = "packycode"
base_url = "https://codex-api.packycode.com/v1"
requires_openai_auth = true
wire_api = "responses"
"#;
        write_file(&cfg_path, config_text);

        let mut cfg = ProxyConfig::default();
        bootstrap_from_codex(&mut cfg).expect("bootstrap_from_codex should succeed");

        let svc = cfg.codex.active_config().expect("active codex config");
        assert_eq!(svc.name, "packycode");
        let up = &svc.upstreams[0];
        assert_eq!(up.base_url, "https://codex-api.packycode.com/v1");
        assert!(up.auth.auth_token.is_none());
        assert_eq!(
            up.tags.get("requires_openai_auth").map(|s| s.as_str()),
            Some("true")
        );
    }

    #[test]
    fn probe_codex_bootstrap_detects_codex_proxy_without_backup() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async move {
            let cfg_path = home.join("config.toml");
            let config_text = r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "codex-helper"
base_url = "http://127.0.0.1:3211"
wire_api = "responses"
"#;
            write_file(&cfg_path, config_text);

            // 不写备份文件，模拟“已经被本地代理接管且无原始备份”的场景
            let err = super::probe_codex_bootstrap_from_cli()
                .await
                .expect_err("probe should fail when model_provider is codex_proxy without backup");
            let msg = err.to_string();
            assert!(
                msg.contains("当前 model_provider 指向本地代理 codex-helper，且未找到备份配置"),
                "unexpected error message: {}",
                msg
            );
        });
    }

    #[test]
    fn sync_codex_auth_updates_env_key_without_changing_routing_config() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();

        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#;
        write_file(&cfg_path, config_text);

        let auth_path = home.join("auth.json");
        let auth_text = r#"{ "RIGHTCODE_API_KEY": "sk-test-123" }"#;
        write_file(&auth_path, auth_text);

        let mut cfg = ProxyConfig::default();
        cfg.codex.active = Some("keep-active".to_string());
        cfg.codex.configs.insert(
            "right".to_string(),
            ServiceConfig {
                name: "right".to_string(),
                alias: None,
                enabled: false,
                level: 7,
                upstreams: vec![UpstreamConfig {
                    base_url: "https://www.right.codes/codex/v1".to_string(),
                    auth: UpstreamAuth {
                        auth_token: None,
                        auth_token_env: Some("OLD_KEY".to_string()),
                        api_key: None,
                        api_key_env: None,
                    },
                    tags: {
                        let mut t = HashMap::new();
                        t.insert("provider_id".into(), "right".into());
                        t.insert("source".into(), "codex-config".into());
                        t
                    },
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                }],
            },
        );

        let report = sync_codex_auth_from_codex_cli(
            &mut cfg,
            SyncCodexAuthFromCodexOptions {
                add_missing: false,
                set_active: false,
                force: false,
            },
        )
        .expect("sync should succeed");

        assert_eq!(report.updated, 1);
        assert_eq!(report.added, 0);
        assert!(!report.active_set);

        let svc = cfg.codex.configs.get("right").expect("right config exists");
        assert_eq!(svc.level, 7);
        assert!(!svc.enabled, "enabled should not be changed by sync");
        assert_eq!(
            svc.upstreams[0].auth.auth_token_env.as_deref(),
            Some("RIGHTCODE_API_KEY")
        );
        assert_eq!(
            cfg.codex.active.as_deref(),
            Some("keep-active"),
            "active should not be changed by sync unless set_active is true"
        );
    }

    #[test]
    fn sync_codex_auth_can_add_missing_provider_and_set_active() {
        let env = setup_temp_codex_home();
        let home = env.home.clone();

        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "right"

[model_providers.right]
name = "right"
base_url = "https://www.right.codes/codex/v1"
env_key = "RIGHTCODE_API_KEY"
"#;
        write_file(&cfg_path, config_text);

        let auth_path = home.join("auth.json");
        let auth_text = r#"{ "RIGHTCODE_API_KEY": "sk-test-123" }"#;
        write_file(&auth_path, auth_text);

        let mut cfg = ProxyConfig::default();
        cfg.codex.active = Some("openai".to_string());

        let report = sync_codex_auth_from_codex_cli(
            &mut cfg,
            SyncCodexAuthFromCodexOptions {
                add_missing: true,
                set_active: true,
                force: false,
            },
        )
        .expect("sync should succeed");

        assert_eq!(report.added, 1);
        assert!(report.active_set);
        assert_eq!(cfg.codex.active.as_deref(), Some("right"));

        let svc = cfg
            .codex
            .configs
            .get("right")
            .expect("right config should be added");
        assert!(svc.enabled);
        assert_eq!(svc.level, 1);
        assert_eq!(
            svc.upstreams[0].auth.auth_token_env.as_deref(),
            Some("RIGHTCODE_API_KEY")
        );
        assert_eq!(
            svc.upstreams[0].tags.get("source").map(|s| s.as_str()),
            Some("codex-config")
        );
    }

    #[test]
    fn compile_v2_to_runtime_orders_preferred_members() {
        let mut openai_endpoints = BTreeMap::new();
        openai_endpoints.insert(
            "hk".to_string(),
            ProviderEndpointV2 {
                base_url: "https://hk.example.com/v1".to_string(),
                enabled: true,
                tags: BTreeMap::from([("region".to_string(), "hk".to_string())]),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
            },
        );
        openai_endpoints.insert(
            "us".to_string(),
            ProviderEndpointV2 {
                base_url: "https://us.example.com/v1".to_string(),
                enabled: true,
                tags: BTreeMap::from([("region".to_string(), "us".to_string())]),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
            },
        );

        let mut backup_endpoints = BTreeMap::new();
        backup_endpoints.insert(
            "default".to_string(),
            ProviderEndpointV2 {
                base_url: "https://backup.example.com/v1".to_string(),
                enabled: true,
                tags: BTreeMap::new(),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
            },
        );

        let mut providers = BTreeMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfigV2 {
                alias: Some("OpenAI".to_string()),
                enabled: true,
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: Some("OPENAI_API_KEY".to_string()),
                    api_key: None,
                    api_key_env: None,
                },
                tags: BTreeMap::from([("provider_id".to_string(), "openai".to_string())]),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
                endpoints: openai_endpoints,
            },
        );
        providers.insert(
            "backup".to_string(),
            ProviderConfigV2 {
                alias: Some("Backup".to_string()),
                enabled: true,
                auth: UpstreamAuth::default(),
                tags: BTreeMap::new(),
                supported_models: BTreeMap::new(),
                model_mapping: BTreeMap::new(),
                endpoints: backup_endpoints,
            },
        );

        let v2 = ProxyConfigV2 {
            version: 2,
            codex: ServiceViewV2 {
                active_group: Some("primary".to_string()),
                default_profile: None,
                profiles: BTreeMap::new(),
                providers,
                groups: BTreeMap::from([(
                    "primary".to_string(),
                    GroupConfigV2 {
                        alias: Some("Primary".to_string()),
                        enabled: true,
                        level: 1,
                        members: vec![
                            GroupMemberRefV2 {
                                provider: "backup".to_string(),
                                endpoint_names: vec!["default".to_string()],
                                preferred: false,
                            },
                            GroupMemberRefV2 {
                                provider: "openai".to_string(),
                                endpoint_names: vec!["hk".to_string(), "us".to_string()],
                                preferred: true,
                            },
                        ],
                    },
                )]),
            },
            claude: ServiceViewV2::default(),
            retry: RetryConfig::default(),
            notify: NotifyConfig::default(),
            default_service: Some(ServiceKind::Codex),
            ui: UiConfig::default(),
        };

        let runtime = compile_v2_to_runtime(&v2).expect("compile_v2_to_runtime");
        let svc = runtime
            .codex
            .configs
            .get("primary")
            .expect("compiled primary group");

        assert_eq!(svc.upstreams.len(), 3);
        assert_eq!(svc.upstreams[0].base_url, "https://hk.example.com/v1");
        assert_eq!(svc.upstreams[1].base_url, "https://us.example.com/v1");
        assert_eq!(svc.upstreams[2].base_url, "https://backup.example.com/v1");
        assert_eq!(
            svc.upstreams[0].auth.auth_token_env.as_deref(),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(
            svc.upstreams[0].tags.get("provider_id").map(|s| s.as_str()),
            Some("openai")
        );
        assert_eq!(
            svc.upstreams[0].tags.get("region").map(|s| s.as_str()),
            Some("hk")
        );
    }

    #[test]
    fn migrate_legacy_to_v2_creates_provider_per_upstream() {
        let mut legacy = ProxyConfig::default();
        legacy.codex.active = Some("team".to_string());
        legacy.codex.configs.insert(
            "team".to_string(),
            ServiceConfig {
                name: "team".to_string(),
                alias: Some("Team".to_string()),
                enabled: false,
                level: 3,
                upstreams: vec![
                    UpstreamConfig {
                        base_url: "https://one.example.com/v1".to_string(),
                        auth: UpstreamAuth {
                            auth_token: None,
                            auth_token_env: Some("ONE_KEY".to_string()),
                            api_key: None,
                            api_key_env: None,
                        },
                        tags: HashMap::from([("provider_id".to_string(), "one".to_string())]),
                        supported_models: HashMap::new(),
                        model_mapping: HashMap::new(),
                    },
                    UpstreamConfig {
                        base_url: "https://two.example.com/v1".to_string(),
                        auth: UpstreamAuth::default(),
                        tags: HashMap::new(),
                        supported_models: HashMap::new(),
                        model_mapping: HashMap::new(),
                    },
                ],
            },
        );

        let migrated = migrate_legacy_to_v2(&legacy);
        assert_eq!(migrated.version, 2);
        assert_eq!(migrated.codex.active_group.as_deref(), Some("team"));

        let group = migrated
            .codex
            .groups
            .get("team")
            .expect("team group should exist");
        assert_eq!(group.alias.as_deref(), Some("Team"));
        assert!(!group.enabled);
        assert_eq!(group.level, 3);
        assert_eq!(group.members.len(), 2);
        assert_eq!(group.members[0].provider, "team__u01");
        assert_eq!(group.members[1].provider, "team__u02");

        let provider = migrated
            .codex
            .providers
            .get("team__u01")
            .expect("team__u01 provider should exist");
        assert_eq!(provider.alias.as_deref(), Some("one"));
        assert_eq!(provider.auth.auth_token_env.as_deref(), Some("ONE_KEY"));
        assert_eq!(
            provider
                .endpoints
                .get("default")
                .expect("default endpoint")
                .base_url,
            "https://one.example.com/v1"
        );
    }

    #[test]
    fn compact_v2_config_merges_same_provider_endpoints() {
        let mut legacy = ProxyConfig::default();
        legacy.codex.active = Some("team".to_string());
        legacy.codex.configs.insert(
            "team".to_string(),
            ServiceConfig {
                name: "team".to_string(),
                alias: Some("Team".to_string()),
                enabled: true,
                level: 1,
                upstreams: vec![
                    UpstreamConfig {
                        base_url: "https://hk.example.com/v1".to_string(),
                        auth: UpstreamAuth {
                            auth_token: None,
                            auth_token_env: Some("OPENAI_API_KEY".to_string()),
                            api_key: None,
                            api_key_env: None,
                        },
                        tags: HashMap::from([
                            ("provider_id".to_string(), "openai".to_string()),
                            ("region".to_string(), "hk".to_string()),
                        ]),
                        supported_models: HashMap::new(),
                        model_mapping: HashMap::new(),
                    },
                    UpstreamConfig {
                        base_url: "https://us.example.com/v1".to_string(),
                        auth: UpstreamAuth {
                            auth_token: None,
                            auth_token_env: Some("OPENAI_API_KEY".to_string()),
                            api_key: None,
                            api_key_env: None,
                        },
                        tags: HashMap::from([
                            ("provider_id".to_string(), "openai".to_string()),
                            ("region".to_string(), "us".to_string()),
                        ]),
                        supported_models: HashMap::new(),
                        model_mapping: HashMap::new(),
                    },
                ],
            },
        );

        let migrated = migrate_legacy_to_v2(&legacy);
        let compact = compact_v2_config(&migrated).expect("compact_v2_config");

        assert_eq!(compact.codex.providers.len(), 1);
        let provider = compact
            .codex
            .providers
            .get("openai")
            .expect("openai provider should exist");
        assert_eq!(
            provider.auth.auth_token_env.as_deref(),
            Some("OPENAI_API_KEY")
        );
        assert_eq!(
            provider.tags.get("provider_id").map(|s| s.as_str()),
            Some("openai")
        );
        assert_eq!(provider.endpoints.len(), 2);
        assert!(provider.endpoints.contains_key("hk"));
        assert!(provider.endpoints.contains_key("us"));

        let group = compact
            .codex
            .groups
            .get("team")
            .expect("team group should exist");
        assert_eq!(group.members.len(), 1);
        assert_eq!(group.members[0].provider, "openai");
        assert_eq!(
            group.members[0].endpoint_names,
            vec!["hk".to_string(), "us".to_string()]
        );
    }

    #[test]
    fn load_config_supports_v2_schema() {
        let _env = setup_temp_codex_home();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async move {
            let dir = super::proxy_home_dir();
            let toml_path = dir.join("config.toml");
            write_file(
                &toml_path,
                r#"
version = 2

[codex]
active_group = "primary"
default_profile = "daily"

[codex.profiles.daily]
station = "primary"
reasoning_effort = "medium"
service_tier = "priority"

[codex.providers.openai]
[codex.providers.openai.auth]
auth_token_env = "OPENAI_API_KEY"
[codex.providers.openai.tags]
provider_id = "openai"
[codex.providers.openai.endpoints.hk]
base_url = "https://hk.example.com/v1"
[codex.providers.openai.endpoints.hk.tags]
region = "hk"
[codex.providers.openai.endpoints.us]
base_url = "https://us.example.com/v1"

[codex.groups.primary]
level = 2

[[codex.groups.primary.members]]
provider = "openai"
endpoint_names = ["us"]
preferred = true
"#,
            );

            let cfg = super::load_config().await.expect("load v2 config");
            assert_eq!(cfg.version, Some(2));
            assert_eq!(cfg.codex.active.as_deref(), Some("primary"));
            assert_eq!(cfg.codex.default_profile.as_deref(), Some("daily"));
            assert_eq!(
                cfg.codex
                    .profiles
                    .get("daily")
                    .and_then(|profile| profile.station.as_deref()),
                Some("primary")
            );

            let svc = cfg
                .codex
                .configs
                .get("primary")
                .expect("primary config should exist");
            assert_eq!(svc.level, 2);
            assert_eq!(svc.upstreams.len(), 1);
            assert_eq!(svc.upstreams[0].base_url, "https://us.example.com/v1");
            assert_eq!(
                svc.upstreams[0].auth.auth_token_env.as_deref(),
                Some("OPENAI_API_KEY")
            );
            assert_eq!(
                svc.upstreams[0].tags.get("provider_id").map(|s| s.as_str()),
                Some("openai")
            );
        });
    }

    #[test]
    fn save_config_after_loading_v2_writes_legacy_schema() {
        let _env = setup_temp_codex_home();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async move {
            let dir = super::proxy_home_dir();
            let toml_path = dir.join("config.toml");
            write_file(
                &toml_path,
                r#"
version = 2

[codex]
active_group = "primary"
default_profile = "daily"

[codex.profiles.daily]
station = "primary"
service_tier = "priority"

[codex.providers.openai]
[codex.providers.openai.auth]
auth_token_env = "OPENAI_API_KEY"
[codex.providers.openai.endpoints.default]
base_url = "https://api.example.com/v1"

[codex.groups.primary]
level = 1

[[codex.groups.primary.members]]
provider = "openai"
endpoint_names = ["default"]
"#,
            );

            let cfg = super::load_config().await.expect("load v2 config");
            assert_eq!(cfg.version, Some(2));

            super::save_config(&cfg).await.expect("save legacy config");
            let saved = std::fs::read_to_string(&toml_path).expect("read saved config.toml");
            assert!(saved.contains("version = 1"));
            assert!(saved.contains("[codex.configs.primary]"));
            assert!(saved.contains("default_profile = \"daily\""));
            assert!(saved.contains("[codex.profiles.daily]"));
            assert!(saved.contains("service_tier = \"priority\""));
            assert!(!saved.contains("[codex.groups.primary]"));
        });
    }

    #[test]
    fn load_config_rejects_invalid_default_profile() {
        let _env = setup_temp_codex_home();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async move {
            let dir = super::proxy_home_dir();
            let toml_path = dir.join("config.toml");
            write_file(
                &toml_path,
                r#"
version = 1

[codex]
active = "primary"
default_profile = "missing"

[codex.configs.primary]
name = "primary"

[[codex.configs.primary.upstreams]]
base_url = "https://api.example.com/v1"
"#,
            );

            let err = super::load_config().await.expect_err("load should fail");
            assert!(
                err.to_string().contains("default_profile"),
                "unexpected error: {err}"
            );
        });
    }

    #[test]
    fn load_config_rejects_profile_model_incompatible_with_station_capabilities() {
        let _env = setup_temp_codex_home();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async move {
            let dir = super::proxy_home_dir();
            let toml_path = dir.join("config.toml");
            write_file(
                &toml_path,
                r#"
version = 1

[codex]
active = "primary"
default_profile = "fast"

[codex.profiles.fast]
station = "primary"
model = "gpt-4.1"

[codex.configs.primary]
name = "primary"

[[codex.configs.primary.upstreams]]
base_url = "https://api.example.com/v1"
supported_models = { "gpt-5.4" = true }
"#,
            );

            let err = super::load_config().await.expect_err("load should fail");
            assert!(
                err.to_string().contains("not supported"),
                "unexpected error: {err}"
            );
        });
    }

    #[test]
    fn save_config_v2_writes_v2_schema_and_backup() {
        let _env = setup_temp_codex_home();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async move {
            let dir = super::proxy_home_dir();
            let toml_path = dir.join("config.toml");
            let backup_path = dir.join("config.toml.bak");
            write_file(
                &toml_path,
                r#"
version = 1

[codex]
active = "legacy"

[codex.configs.legacy]
name = "legacy"
level = 1

[[codex.configs.legacy.upstreams]]
base_url = "https://legacy.example.com/v1"
"#,
            );

            let legacy = super::load_config().await.expect("load legacy config");
            let migrated = migrate_legacy_to_v2(&legacy);
            let written_path = super::save_config_v2(&migrated)
                .await
                .expect("save_config_v2 should succeed");

            assert_eq!(written_path, toml_path);
            let saved = std::fs::read_to_string(&toml_path).expect("read v2 config.toml");
            assert!(saved.contains("version = 2"));
            assert!(saved.contains("[codex.providers.legacy__u01]"));
            assert!(saved.contains("[codex.groups.legacy]"));

            let backup = std::fs::read_to_string(&backup_path).expect("read config.toml.bak");
            assert!(backup.contains("version = 1"));
            assert!(backup.contains("[codex.configs.legacy]"));
        });
    }
}
