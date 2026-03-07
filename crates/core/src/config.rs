use std::collections::HashMap;
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
pub use storage_impl::{config_file_path, init_config_toml, load_config, save_config};

#[cfg(test)]
use bootstrap_impl::bootstrap_from_codex;

pub mod storage {
    pub use super::storage_impl::{config_file_path, init_config_toml, load_config, save_config};
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
}
