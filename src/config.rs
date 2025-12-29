use std::collections::HashMap;
use std::env;
use std::fs as stdfs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dirs::home_dir;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::fs;
use toml::Value as TomlValue;
use tracing::{info, warn};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub backoff_ms: u64,
    pub backoff_max_ms: u64,
    pub jitter_ms: u64,
    pub on_status: String,
    pub on_class: Vec<String>,
    pub cloudflare_challenge_cooldown_secs: u64,
    pub cloudflare_timeout_cooldown_secs: u64,
    pub transport_cooldown_secs: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 2,
            backoff_ms: 200,
            backoff_max_ms: 2_000,
            jitter_ms: 100,
            on_status: "429,502,503,504,524".to_string(),
            on_class: vec![
                "upstream_transport_error".to_string(),
                "cloudflare_timeout".to_string(),
                "cloudflare_challenge".to_string(),
            ],
            cloudflare_challenge_cooldown_secs: 300,
            cloudflare_timeout_cooldown_secs: 60,
            transport_cooldown_secs: 30,
        }
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
    /// Global retry policy (can be overridden by env vars)
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

fn config_dir() -> PathBuf {
    proxy_home_dir()
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

fn config_backup_path() -> PathBuf {
    config_dir().join("config.json.bak")
}

fn config_toml_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn config_toml_backup_path() -> PathBuf {
    config_dir().join("config.toml.bak")
}

/// Return the primary config file path that will be used by `load_config()`.
pub fn config_file_path() -> PathBuf {
    let toml_path = config_toml_path();
    if toml_path.exists() {
        toml_path
    } else if config_path().exists() {
        config_path()
    } else {
        toml_path
    }
}

const CONFIG_VERSION: u32 = 1;

fn ensure_config_version(cfg: &mut ProxyConfig) {
    if cfg.version.is_none() {
        cfg.version = Some(CONFIG_VERSION);
    }
}

const CONFIG_TOML_DOC_HEADER: &str = r#"# codex-helper config.toml
#
# This file is optional. If present, codex-helper will prefer it over config.json.
# - To generate a commented template: `codex-helper config init`
# - To keep config secrets off disk, prefer *_env fields (e.g. auth_token_env/api_key_env).
#
# Note: some commands may rewrite this file; the header is preserved to keep the docs close to the config.
"#;

const CONFIG_TOML_TEMPLATE: &str = r#"# codex-helper config.toml
#
# codex-helper supports both config.json and config.toml:
# - If `config.toml` exists, it takes precedence over `config.json`.
# - Otherwise `config.json` is used (backward-compatible default).
#
# This template focuses on discoverability: it includes commented examples and per-field notes.
#
# Paths:
# - Linux/macOS: ~/.codex-helper/config.toml
# - Windows:     %USERPROFILE%\.codex-helper\config.toml
#
# Tip:
# - Generate/overwrite this template: `codex-helper config init [--force]`
# - Fresh installs default to writing TOML on first write.

version = 1

# Which service to use by default when you omit --codex/--claude.
# default_service = "codex"
# default_service = "claude"

# --- Common: upstream configs (accounts / API keys) ---
#
# Most users only need to edit this section.
#
# Notes:
# - Prefer env-based secrets (`*_env`) instead of writing tokens to disk.
# - For multi-upstream failover, put multiple `[[...upstreams]]` under the same config.
# - Optional: set per-config `level` (1..=10) to enable level-based routing across configs (only when multiple distinct levels exist).
#
# [codex]
# active = "codex-main"
#
# [codex.configs.codex-main]
# name = "codex-main"
# alias = "primary+backup"
# # enabled = true
# # level = 1
#
# # Primary upstream
# [[codex.configs.codex-main.upstreams]]
# base_url = "https://api.openai.com/v1"
# [codex.configs.codex-main.upstreams.auth]
# auth_token_env = "OPENAI_API_KEY"
# # or: api_key_env = "OPENAI_API_KEY"
# # (not recommended) auth_token = "sk-..."
# [codex.configs.codex-main.upstreams.tags]
# provider_id = "openai"
#
# # Backup upstream
# [[codex.configs.codex-main.upstreams]]
# base_url = "https://your-backup-provider.example/v1"
# [codex.configs.codex-main.upstreams.auth]
# auth_token_env = "BACKUP_API_KEY"
# [codex.configs.codex-main.upstreams.tags]
# provider_id = "backup"
#
# Claude configs share the same structure under [claude].
#
# ---
#
# --- Notify integration (Codex `notify` hook) ---
#
# This is optional and disabled by default.
# It is designed for multi-Codex workflows: low-noise, duration-based, and rate-limited.
#
# To enable:
# 1) In Codex config `~/.codex/config.toml`:
#      notify = ["codex-helper", "notify", "codex"]
# 2) Here:
#      notify.enabled = true
#      notify.system.enabled = true
#
[notify]
# Master switch for notify processing (both system and exec sinks).
enabled = false

[notify.system]
# System notifications are supported on:
# - Windows: toast via powershell.exe
# - macOS: `osascript`
enabled = false

[notify.policy]
# D: duration-based filter (milliseconds)
min_duration_ms = 60000

# A: merge + rate-limit (milliseconds)
merge_window_ms = 10000
global_cooldown_ms = 60000
per_thread_cooldown_ms = 180000

# How far back to look in proxy /__codex_helper/status/recent (milliseconds).
# codex-helper matches Codex "thread-id" to proxy FinishedRequest.session_id.
recent_search_window_ms = 300000
# HTTP timeout for the proxy recent endpoint (milliseconds)
recent_endpoint_timeout_ms = 500

[notify.exec]
# Optional callback sink: run a command and write aggregated JSON to stdin.
enabled = false
# command = ["python", "my_hook.py"]

# ---
#
# --- Retry policy (proxy-side) ---
#
# Controls codex-helper's own retries before returning a response to Codex.
# Note: if you also enable Codex retries, you may get "double retry".
#
[retry]
# Max attempts per request (including the first attempt). Set to 1 to disable retries.
max_attempts = 2

# Base backoff between attempts (milliseconds).
backoff_ms = 200
# Maximum backoff cap (milliseconds).
backoff_max_ms = 2000
# Random jitter added to backoff (milliseconds).
jitter_ms = 100

# HTTP status codes/ranges that are retryable (string form).
# Examples: "429,502,503,504,524" or "429,500-599".
on_status = "429,502,503,504,524"

# Retryable error classes (from codex-helper classification).
on_class = ["upstream_transport_error", "cloudflare_timeout", "cloudflare_challenge"]

# Cooldown penalties (seconds) applied to an upstream after certain failure classes.
cloudflare_challenge_cooldown_secs = 300
cloudflare_timeout_cooldown_secs = 60
transport_cooldown_secs = 30
"#;

pub async fn init_config_toml(force: bool) -> Result<PathBuf> {
    let dir = config_dir();
    fs::create_dir_all(&dir).await?;
    let path = config_toml_path();
    let backup_path = config_toml_backup_path();

    if path.exists() && !force {
        anyhow::bail!(
            "config.toml already exists at {:?}; use --force to overwrite",
            path
        );
    }

    if path.exists()
        && let Err(err) = fs::copy(&path, &backup_path).await
    {
        warn!("failed to backup {:?} to {:?}: {}", path, backup_path, err);
    }

    let tmp_path = dir.join("config.toml.tmp");
    fs::write(&tmp_path, CONFIG_TOML_TEMPLATE.as_bytes()).await?;
    fs::rename(&tmp_path, &path).await?;
    Ok(path)
}

pub async fn load_config() -> Result<ProxyConfig> {
    let toml_path = config_toml_path();
    if toml_path.exists() {
        let text = fs::read_to_string(&toml_path).await?;
        let mut cfg = toml::from_str::<ProxyConfig>(&text)?;
        ensure_config_version(&mut cfg);
        return Ok(cfg);
    }

    let json_path = config_path();
    if json_path.exists() {
        let bytes = fs::read(json_path).await?;
        let mut cfg = serde_json::from_slice::<ProxyConfig>(&bytes)?;
        ensure_config_version(&mut cfg);
        return Ok(cfg);
    }

    let mut cfg = ProxyConfig::default();
    ensure_config_version(&mut cfg);
    Ok(cfg)
}

pub async fn save_config(cfg: &ProxyConfig) -> Result<()> {
    let mut cfg = cfg.clone();
    ensure_config_version(&mut cfg);

    let dir = config_dir();
    fs::create_dir_all(&dir).await?;
    let toml_path = config_toml_path();
    let json_path = config_path();
    let (path, backup_path, data) = if toml_path.exists() || !json_path.exists() {
        let body = toml::to_string_pretty(&cfg)?;
        let text = format!("{CONFIG_TOML_DOC_HEADER}\n{body}");
        (toml_path, config_toml_backup_path(), text.into_bytes())
    } else {
        (
            json_path,
            config_backup_path(),
            serde_json::to_vec_pretty(&cfg)?,
        )
    };

    // 先备份旧文件（若存在），再采用临时文件 + rename 方式原子写入，尽量避免配置损坏。
    if path.exists()
        && let Err(err) = fs::copy(&path, &backup_path).await
    {
        warn!("failed to backup {:?} to {:?}: {}", path, backup_path, err);
    }

    let tmp_path = dir.join("config.tmp");
    fs::write(&tmp_path, &data).await?;
    fs::rename(&tmp_path, &path).await?;
    Ok(())
}

/// 获取 codex-helper 的主目录（用于配置、日志等）
pub fn proxy_home_dir() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex-helper")
}

fn codex_home() -> PathBuf {
    if let Ok(dir) = env::var("CODEX_HOME") {
        return PathBuf::from(dir);
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
}

pub fn codex_config_path() -> PathBuf {
    codex_home().join("config.toml")
}

pub fn codex_backup_config_path() -> PathBuf {
    codex_home().join("config.toml.codex-helper-backup")
}

pub fn codex_auth_path() -> PathBuf {
    codex_home().join("auth.json")
}

fn claude_home() -> PathBuf {
    if let Ok(dir) = env::var("CLAUDE_HOME") {
        return PathBuf::from(dir);
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
}

pub fn claude_settings_path() -> PathBuf {
    let dir = claude_home();
    let settings = dir.join("settings.json");
    if settings.exists() {
        return settings;
    }
    let legacy = dir.join("claude.json");
    if legacy.exists() {
        return legacy;
    }
    settings
}

pub fn claude_settings_backup_path() -> PathBuf {
    let mut path = claude_settings_path();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "settings.json".to_string());
    path.set_file_name(format!("{file_name}.codex-helper-backup"));
    path
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

fn read_file_if_exists(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let s = stdfs::read_to_string(path).with_context(|| format!("failed to read {:?}", path))?;
    Ok(Some(s))
}

fn is_codex_absent_backup_sentinel(text: &str) -> bool {
    text.trim() == "# codex-helper-backup:absent"
}

fn is_claude_absent_backup_sentinel(text: &str) -> bool {
    text.trim() == "{\"__codex_helper_backup_absent\":true}"
}

/// Try to infer a unique API key from ~/.codex/auth.json when the provider
/// does not declare an explicit `env_key`.
///
/// This mirrors the common Codex CLI layout where `auth.json` contains a
/// single `*_API_KEY` field (e.g. `OPENAI_API_KEY`) plus metadata fields
/// like `tokens` / `last_refresh`. We only consider string values whose
/// key ends with `_API_KEY`, and only succeed when there is exactly one
/// such candidate; otherwise we return None and let the caller error out.
fn infer_env_key_from_auth_json(auth_json: &Option<JsonValue>) -> Option<(String, String)> {
    let json = auth_json.as_ref()?;
    let obj = json.as_object()?;

    let mut candidates: Vec<(String, String)> = obj
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k, s)))
        .filter(|(k, v)| k.ends_with("_API_KEY") && !v.trim().is_empty())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    if candidates.len() == 1 {
        candidates.pop()
    } else {
        None
    }
}

fn bootstrap_from_codex(cfg: &mut ProxyConfig) -> Result<()> {
    if !cfg.codex.configs.is_empty() {
        return Ok(());
    }

    // 优先从备份配置中推导原始上游，避免在 ~/.codex/config.toml 已被 codex-helper
    // 写成本地 provider（codex_proxy）时出现“自我转发”。
    let backup_path = codex_backup_config_path();
    let cfg_path = codex_config_path();
    let cfg_text_opt = if let Some(text) = read_file_if_exists(&backup_path)?
        && !is_codex_absent_backup_sentinel(&text)
    {
        Some(text)
    } else {
        read_file_if_exists(&cfg_path)?
    };
    let cfg_text = match cfg_text_opt {
        Some(s) if !s.trim().is_empty() => s,
        _ => {
            anyhow::bail!("未找到 ~/.codex/config.toml 或文件为空，无法自动推导 Codex 上游");
        }
    };

    let value: TomlValue = cfg_text.parse()?;
    let table = value
        .as_table()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Codex config root must be table"))?;

    let current_provider_id = table
        .get("model_provider")
        .and_then(|v| v.as_str())
        .unwrap_or("openai")
        .to_string();

    let providers_table = table
        .get("model_providers")
        .and_then(|v| v.as_table())
        .cloned()
        .unwrap_or_default();

    let auth_json_path = codex_auth_path();
    let auth_json: Option<JsonValue> = match read_file_if_exists(&auth_json_path)? {
        Some(s) if !s.trim().is_empty() => serde_json::from_str(&s).ok(),
        _ => None,
    };
    let inferred_env_key = infer_env_key_from_auth_json(&auth_json).map(|(k, _)| k);

    // 如当前 provider 看起来是本地 codex-helper 代理且没有备份（或备份无效），
    // 则无法安全推导原始上游，直接报错，避免将代理指向自身。
    if current_provider_id == "codex_proxy" && !backup_path.exists() {
        let provider_table = providers_table.get(&current_provider_id);
        let is_local_helper = provider_table
            .and_then(|t| t.get("base_url"))
            .and_then(|v| v.as_str())
            .map(|u| u.contains("127.0.0.1") || u.contains("localhost"))
            .unwrap_or(false);
        if is_local_helper {
            anyhow::bail!(
                "检测到 ~/.codex/config.toml 的当前 model_provider 指向本地代理 codex-helper，且未找到备份配置；\
无法自动推导原始 Codex 上游。请先恢复 ~/.codex/config.toml 后重试，或在 ~/.codex-helper/config.json 中手动添加 codex 上游配置。"
            );
        }
    }

    let mut imported_any = false;
    let mut imported_active = false;

    // Import all providers from [model_providers.*] as switchable configs.
    for (provider_id, provider_val) in providers_table.iter() {
        let Some(provider_table) = provider_val.as_table() else {
            continue;
        };

        let requires_openai_auth = provider_table
            .get("requires_openai_auth")
            .and_then(|v| v.as_bool())
            .unwrap_or(provider_id == "openai");

        let base_url_opt = provider_table
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                if provider_id == "openai" {
                    Some("https://api.openai.com/v1".to_string())
                } else {
                    None
                }
            });

        let base_url = match base_url_opt {
            Some(u) if !u.trim().is_empty() => u,
            _ => {
                if provider_id == &current_provider_id {
                    anyhow::bail!(
                        "当前 model_provider '{}' 缺少 base_url，无法自动推导 Codex 上游",
                        provider_id
                    );
                }
                warn!(
                    "skip model_provider '{}' because base_url is missing",
                    provider_id
                );
                continue;
            }
        };

        if provider_id == "codex_proxy"
            && (base_url.contains("127.0.0.1") || base_url.contains("localhost"))
        {
            if provider_id == &current_provider_id && !backup_path.exists() {
                anyhow::bail!(
                    "检测到 ~/.codex/config.toml 的当前 model_provider 指向本地代理 codex-helper，且未找到备份配置；\
无法自动推导原始 Codex 上游。请先恢复 ~/.codex/config.toml 后重试，或在 ~/.codex-helper/config.json 中手动添加 codex 上游配置。"
                );
            }
            warn!("skip model_provider 'codex_proxy' to avoid self-forwarding loop");
            continue;
        }

        let env_key = provider_table
            .get("env_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty());

        let (auth_token, auth_token_env) = if requires_openai_auth {
            (None, None)
        } else {
            let effective_env_key = env_key.clone().or_else(|| inferred_env_key.clone());
            if effective_env_key.is_none() {
                if provider_id == &current_provider_id {
                    anyhow::bail!(
                        "当前 model_provider 未声明 env_key，且无法从 ~/.codex/auth.json 推断唯一的 `*_API_KEY` 字段；请为该 provider 配置 env_key"
                    );
                }
                warn!(
                    "skip model_provider '{}' because env_key is missing and auth.json can't infer a unique *_API_KEY",
                    provider_id
                );
                continue;
            }
            (None, effective_env_key)
        };

        let alias = provider_table
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .filter(|s| s != provider_id);

        let mut tags = HashMap::new();
        tags.insert("source".into(), "codex-config".into());
        tags.insert("provider_id".into(), provider_id.to_string());
        tags.insert(
            "requires_openai_auth".into(),
            requires_openai_auth.to_string(),
        );

        let upstream = UpstreamConfig {
            base_url: base_url.clone(),
            auth: UpstreamAuth {
                auth_token,
                auth_token_env,
                api_key: None,
                api_key_env: None,
            },
            tags,
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        };

        let service = ServiceConfig {
            name: provider_id.to_string(),
            alias,
            enabled: true,
            level: 1,
            upstreams: vec![upstream],
        };

        cfg.codex.configs.insert(provider_id.to_string(), service);
        imported_any = true;
        if provider_id == &current_provider_id {
            imported_active = true;
        }
    }

    // Ensure openai exists as a safe default (even if model_providers table is absent).
    if !cfg.codex.configs.contains_key("openai") {
        let mut tags = HashMap::new();
        tags.insert("source".into(), "codex-config".into());
        tags.insert("provider_id".into(), "openai".into());
        tags.insert("requires_openai_auth".into(), "true".into());
        cfg.codex.configs.insert(
            "openai".into(),
            ServiceConfig {
                name: "openai".into(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: vec![UpstreamConfig {
                    base_url: "https://api.openai.com/v1".into(),
                    auth: UpstreamAuth {
                        auth_token: None,
                        auth_token_env: None,
                        api_key: None,
                        api_key_env: None,
                    },
                    tags,
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                }],
            },
        );
        imported_any = true;
    }

    if !imported_any {
        anyhow::bail!("未能从 ~/.codex/config.toml 推导出任何可用的 Codex 上游配置");
    }

    // Prefer the Codex CLI current provider as active.
    if imported_active && cfg.codex.configs.contains_key(&current_provider_id) {
        cfg.codex.active = Some(current_provider_id);
    } else {
        cfg.codex.active = cfg
            .codex
            .configs
            .keys()
            .min()
            .cloned()
            .or_else(|| Some("openai".to_string()));
    }

    Ok(())
}

fn bootstrap_from_claude(cfg: &mut ProxyConfig) -> Result<()> {
    if !cfg.claude.configs.is_empty() {
        return Ok(());
    }

    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path();
    // Claude 配置同样优先从备份读取，避免将代理指向自身（本地 codex-helper）。
    let settings_text_opt = if let Some(text) = read_file_if_exists(&backup_path)?
        && !is_claude_absent_backup_sentinel(&text)
    {
        Some(text)
    } else {
        read_file_if_exists(&settings_path)?
    };
    let settings_text = match settings_text_opt {
        Some(s) if !s.trim().is_empty() => s,
        _ => {
            anyhow::bail!(
                "未找到 Claude Code 配置文件 {:?}（或文件为空），无法自动推导 Claude 上游；请先在 Claude Code 中完成配置，或手动在 ~/.codex-helper/config.json 中添加 claude 配置",
                settings_path
            );
        }
    };

    let value: JsonValue = serde_json::from_str(&settings_text)
        .with_context(|| format!("解析 {:?} 失败，需为有效的 JSON", settings_path))?;
    let obj = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Claude settings 根节点必须是 JSON object"))?;

    let env_obj = obj
        .get("env")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("Claude settings 中缺少 env 对象"))?;

    let api_key_env = if env_obj
        .get("ANTHROPIC_AUTH_TOKEN")
        .and_then(|v| v.as_str())
        .is_some()
    {
        Some("ANTHROPIC_AUTH_TOKEN".to_string())
    } else if env_obj
        .get("ANTHROPIC_API_KEY")
        .and_then(|v| v.as_str())
        .is_some()
    {
        Some("ANTHROPIC_API_KEY".to_string())
    } else {
        None
    }
    .ok_or_else(|| {
            anyhow::anyhow!(
                "Claude settings 中缺少 ANTHROPIC_AUTH_TOKEN / ANTHROPIC_API_KEY；请先在 Claude Code 中完成登录或配置 API Key"
            )
        })?;

    let base_url = env_obj
        .get("ANTHROPIC_BASE_URL")
        .and_then(|v| v.as_str())
        .unwrap_or("https://api.anthropic.com/v1")
        .to_string();

    // 如当前 base_url 看起来是本地地址且没有备份，则无法安全推导真实上游，
    // 直接报错，避免将 Claude 代理指向自身。
    if !backup_path.exists() && (base_url.contains("127.0.0.1") || base_url.contains("localhost")) {
        anyhow::bail!(
            "检测到 Claude settings {:?} 的 ANTHROPIC_BASE_URL 指向本地地址 ({base_url})，且未找到备份配置；\
无法自动推导原始 Claude 上游。请先恢复 Claude 配置后重试，或在 ~/.codex-helper/config.json 中手动添加 claude 上游配置。",
            settings_path
        );
    }

    let mut tags = HashMap::new();
    tags.insert("source".into(), "claude-settings".into());
    tags.insert("provider_id".into(), "anthropic".into());

    let upstream = UpstreamConfig {
        base_url,
        auth: UpstreamAuth {
            auth_token: None,
            auth_token_env: None,
            api_key: None,
            api_key_env: Some(api_key_env),
        },
        tags,
        supported_models: HashMap::new(),
        model_mapping: HashMap::new(),
    };

    let service = ServiceConfig {
        name: "default".to_string(),
        alias: Some("Claude default".to_string()),
        enabled: true,
        level: 1,
        upstreams: vec![upstream],
    };

    cfg.claude.configs.insert("default".to_string(), service);
    cfg.claude.active = Some("default".to_string());

    Ok(())
}

/// 加载代理配置，如有必要从 ~/.codex 自动初始化 codex 配置。
pub async fn load_or_bootstrap_from_codex() -> Result<ProxyConfig> {
    let mut cfg = load_config().await?;
    if cfg.codex.configs.is_empty() {
        match bootstrap_from_codex(&mut cfg) {
            Ok(()) => {
                let _ = save_config(&cfg).await;
                info!(
                    "已根据 ~/.codex/config.toml 与 ~/.codex/auth.json 自动创建默认 Codex 上游配置"
                );
            }
            Err(err) => {
                warn!(
                    "无法从 ~/.codex 引导 Codex 配置: {err}; \
                     如果尚未安装或配置 Codex CLI 可以忽略，否则请检查 ~/.codex/config.toml 和 ~/.codex/auth.json，或使用 `codex-helper config add` 手动添加上游"
                );
            }
        }
    } else {
        // 已存在配置但没有 active，提示用户检查
        if cfg.codex.active.is_none() && !cfg.codex.configs.is_empty() {
            warn!(
                "检测到 Codex 配置但没有激活项，将使用任意一条配置作为默认；如需指定，请使用 `codex-helper config set-active <name>`"
            );
        }
    }
    Ok(cfg)
}

/// 显式从 Codex CLI 的配置文件（~/.codex/config.toml + auth.json）导入/刷新 codex 段配置。
/// - 当 force = false 且当前已存在 codex 配置时，将返回错误，避免意外覆盖；
/// - 当 force = true 时，将清空现有 codex 段后重新基于 Codex 配置推导。
pub async fn import_codex_config_from_codex_cli(force: bool) -> Result<ProxyConfig> {
    let mut cfg = load_config().await?;
    if !cfg.codex.configs.is_empty() && !force {
        anyhow::bail!(
            "检测到 ~/.codex-helper/config.json 中已存在 Codex 配置；如需根据 ~/.codex/config.toml 重新导入，请使用 --force 覆盖"
        );
    }

    cfg.codex = ServiceConfigManager::default();
    bootstrap_from_codex(&mut cfg)?;
    save_config(&cfg).await?;
    info!(
        "已根据 ~/.codex/config.toml 与 ~/.codex/auth.json 重新导入 Codex 上游配置（force = {}）",
        force
    );
    Ok(cfg)
}

/// 加载代理配置，如有必要从 ~/.claude 初始化 Claude 配置。
pub async fn load_or_bootstrap_from_claude() -> Result<ProxyConfig> {
    let mut cfg = load_config().await?;
    if cfg.claude.configs.is_empty() {
        match bootstrap_from_claude(&mut cfg) {
            Ok(()) => {
                let _ = save_config(&cfg).await;
                info!("已根据 ~/.claude/settings.json 自动创建默认 Claude 上游配置");
            }
            Err(err) => {
                warn!(
                    "无法从 ~/.claude 引导 Claude 配置: {err}; \
                     如果尚未安装或配置 Claude Code 可以忽略，否则请检查 ~/.claude/settings.json，或在 ~/.codex-helper/config.json 中手动添加 claude 配置"
                );
            }
        }
    } else if cfg.claude.active.is_none() && !cfg.claude.configs.is_empty() {
        warn!(
            "检测到 Claude 配置但没有激活项，将使用任意一条配置作为默认；如需指定，请使用 `codex-helper config set-active <name>`（后续将扩展对 Claude 的专用子命令）"
        );
    }
    Ok(cfg)
}

/// Unified entry to load proxy config and, if necessary, bootstrap upstreams
/// from the official Codex / Claude configuration files.
pub async fn load_or_bootstrap_for_service(kind: ServiceKind) -> Result<ProxyConfig> {
    match kind {
        ServiceKind::Codex => load_or_bootstrap_from_codex().await,
        ServiceKind::Claude => load_or_bootstrap_from_claude().await,
    }
}

/// Probe whether we can successfully bootstrap Codex upstreams from
/// ~/.codex/config.toml and ~/.codex/auth.json without mutating any
/// codex-helper configs. Intended for diagnostics (`codex-helper doctor`).
pub async fn probe_codex_bootstrap_from_cli() -> Result<()> {
    let mut cfg = ProxyConfig::default();
    bootstrap_from_codex(&mut cfg)
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
        unsafe {
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
}
