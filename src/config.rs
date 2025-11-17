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
    /// Optional API key header for some providers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub base_url: String,
    #[serde(default)]
    pub auth: UpstreamAuth,
    /// Optional free-form metadata, e.g. region / label
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

/// A logical config entry (roughly corresponds to cli_proxy 的一个配置名)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// 配置标识（map key），保持稳定
    pub name: String,
    /// 可选别名，便于展示/记忆
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,
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
            .or_else(|| self.configs.values().next())
    }
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
    /// 默认目标服务（用于 CLI 默认选择 codex/claude）
    #[serde(default)]
    pub default_service: Option<ServiceKind>,
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

const CONFIG_VERSION: u32 = 1;

fn ensure_config_version(cfg: &mut ProxyConfig) {
    if cfg.version.is_none() {
        cfg.version = Some(CONFIG_VERSION);
    }
}

pub async fn load_config() -> Result<ProxyConfig> {
    let path = config_path();
    if !path.exists() {
        let mut cfg = ProxyConfig::default();
        ensure_config_version(&mut cfg);
        return Ok(cfg);
    }
    let bytes = fs::read(path).await?;
    let mut cfg = serde_json::from_slice::<ProxyConfig>(&bytes)?;
    ensure_config_version(&mut cfg);
    Ok(cfg)
}

pub async fn save_config(cfg: &ProxyConfig) -> Result<()> {
    let mut cfg = cfg.clone();
    ensure_config_version(&mut cfg);

    let dir = config_dir();
    fs::create_dir_all(&dir).await?;
    let path = config_path();
    let backup_path = config_backup_path();
    let data = serde_json::to_vec_pretty(&cfg)?;

    // 先备份旧文件（若存在），再采用临时文件 + rename 方式原子写入，尽量避免配置损坏。
    if path.exists()
        && let Err(err) = fs::copy(&path, &backup_path).await
    {
        warn!("failed to backup {:?} to {:?}: {}", path, backup_path, err);
    }

    let tmp_path = dir.join("config.json.tmp");
    fs::write(&tmp_path, &data).await?;
    fs::rename(&tmp_path, &path).await?;
    Ok(())
}

pub fn codex_list_configs(cfg: &ProxyConfig) -> Vec<String> {
    cfg.codex.configs.keys().cloned().collect()
}

pub fn codex_set_active(cfg: &mut ProxyConfig, name: &str) -> bool {
    if cfg.codex.configs.contains_key(name) {
        cfg.codex.active = Some(name.to_string());
        true
    } else {
        false
    }
}

/// 获取 codex-proxy 的主目录（用于配置、日志等）
pub fn proxy_home_dir() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex-proxy")
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
    codex_home().join("config.toml.codex-proxy-backup")
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

fn resolve_auth_token(env_key: &str, auth_json: &Option<JsonValue>) -> Option<String> {
    if let Ok(val) = env::var(env_key)
        && !val.trim().is_empty()
    {
        return Some(val);
    }
    if let Some(json) = auth_json
        && let Some(obj) = json.as_object()
        && let Some(v) = obj.get(env_key).and_then(|v| v.as_str())
        && !v.trim().is_empty()
    {
        return Some(v.to_string());
    }
    None
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
    let cfg_text_opt = if let Some(text) = read_file_if_exists(&backup_path)? {
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

    let provider_id = table
        .get("model_provider")
        .and_then(|v| v.as_str())
        .unwrap_or("openai")
        .to_string();

    let providers_table = table
        .get("model_providers")
        .and_then(|v| v.as_table())
        .cloned()
        .unwrap_or_default();

    let provider_table = providers_table.get(&provider_id);

    // 如当前 provider 看起来是本地 codex-helper 代理且没有备份（或备份无效），
    // 则无法安全推导原始上游，直接报错，避免将代理指向自身。
    if provider_id == "codex_proxy" && !backup_path.exists() {
        let is_local_helper = provider_table
            .and_then(|t| t.get("base_url"))
            .and_then(|v| v.as_str())
            .map(|u| u.contains("127.0.0.1") || u.contains("localhost"))
            .unwrap_or(false);
        if is_local_helper {
            anyhow::bail!(
                "检测到 ~/.codex/config.toml 的当前 model_provider 指向本地代理 codex-helper，且未找到备份配置；\
无法自动推导原始 Codex 上游。请先恢复 ~/.codex/config.toml 后重试，或在 ~/.codex-proxy/config.json 中手动添加 codex 上游配置。"
            );
        }
    }

    let base_url = provider_table
        .and_then(|t| t.get("base_url"))
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| {
            if provider_id == "openai" {
                "https://api.openai.com/v1"
            } else {
                "https://api.openai.com/v1"
            }
        })
        .to_string();

    let env_key = provider_table
        .and_then(|t| t.get("env_key"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let auth_json_path = codex_auth_path();
    let auth_json: Option<JsonValue> = match read_file_if_exists(&auth_json_path)? {
        Some(s) if !s.trim().is_empty() => serde_json::from_str(&s).ok(),
        _ => None,
    };

    // 按 Codex 官方语义优先使用 env_key；若当前 provider 未声明 env_key，
    // 则在 ~/.codex/auth.json 中尝试推断唯一的 `*_API_KEY` 字段作为后备。
    let mut effective_env_key = env_key.clone();
    let mut auth_token = effective_env_key
        .as_deref()
        .and_then(|key| resolve_auth_token(key, &auth_json));

    if auth_token.is_none()
        && effective_env_key.is_none()
        && let Some((inferred_key, inferred_token)) = infer_env_key_from_auth_json(&auth_json)
    {
        info!(
            "当前 model_provider 未声明 env_key，已从 ~/.codex/auth.json 自动推断为 `{}`",
            inferred_key
        );
        effective_env_key = Some(inferred_key);
        auth_token = Some(inferred_token);
    }

    if auth_token.is_none() {
        if let Some(key) = effective_env_key.as_deref() {
            anyhow::bail!(
                "未在环境变量或 ~/.codex/auth.json 中找到 `{}`，无法为 Codex 上游构建有效的 Authorization 头；请先在 Codex CLI 中完成登录或配置对应环境变量，然后重试",
                key
            );
        } else {
            anyhow::bail!(
                "当前 model_provider 未声明 env_key，且无法从 ~/.codex/auth.json 推断唯一的 token；请在 Codex CLI 中完成登录，或手动在 ~/.codex/config.toml 中为当前 provider 配置 env_key，并在环境变量或 ~/.codex/auth.json 中提供对应的 token"
            );
        }
    } else {
        info!("已从 ~/.codex/auth.json 或环境变量解析到 Codex 上游 token");
    }

    let mut tags = HashMap::new();
    tags.insert("source".into(), "codex-config".into());
    tags.insert("provider_id".into(), provider_id.clone());

    let upstream = UpstreamConfig {
        base_url,
        auth: UpstreamAuth {
            auth_token,
            api_key: None,
        },
        tags,
    };

    let service = ServiceConfig {
        name: provider_id.clone(),
        alias: None,
        upstreams: vec![upstream],
    };

    cfg.codex.configs.insert(provider_id.clone(), service);
    cfg.codex.active = Some(provider_id);

    Ok(())
}

fn bootstrap_from_claude(cfg: &mut ProxyConfig) -> Result<()> {
    if !cfg.claude.configs.is_empty() {
        return Ok(());
    }

    let settings_path = claude_settings_path();
    let backup_path = claude_settings_backup_path();
    // Claude 配置同样优先从备份读取，避免将代理指向自身（本地 codex-helper）。
    let settings_text_opt = if let Some(text) = read_file_if_exists(&backup_path)? {
        Some(text)
    } else {
        read_file_if_exists(&settings_path)?
    };
    let settings_text = match settings_text_opt {
        Some(s) if !s.trim().is_empty() => s,
        _ => {
            anyhow::bail!(
                "未找到 Claude Code 配置文件 {:?}（或文件为空），无法自动推导 Claude 上游；请先在 Claude Code 中完成配置，或手动在 ~/.codex-proxy/config.json 中添加 claude 配置",
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

    let api_key = env_obj
        .get("ANTHROPIC_AUTH_TOKEN")
        .or_else(|| env_obj.get("ANTHROPIC_API_KEY"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
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
无法自动推导原始 Claude 上游。请先恢复 Claude 配置后重试，或在 ~/.codex-proxy/config.json 中手动添加 claude 上游配置。",
            settings_path
        );
    }

    let mut tags = HashMap::new();
    tags.insert("source".into(), "claude-settings".into());
    tags.insert("provider_id".into(), "anthropic".into());

    let upstream = UpstreamConfig {
        base_url,
        auth: UpstreamAuth {
            auth_token: Some(api_key),
            api_key: None,
        },
        tags,
    };

    let service = ServiceConfig {
        name: "default".to_string(),
        alias: Some("Claude default".to_string()),
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
                     如果尚未安装或配置 Codex CLI 可以忽略，否则请检查 ~/.codex/config.toml 和 ~/.codex/auth.json，或使用 `codex-proxy config add` 手动添加上游"
                );
            }
        }
    } else {
        // 已存在配置但没有 active，提示用户检查
        if cfg.codex.active.is_none() && !cfg.codex.configs.is_empty() {
            warn!(
                "检测到 Codex 配置但没有激活项，将使用任意一条配置作为默认；如需指定，请使用 `codex-proxy config set-active <name>`"
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
            "检测到 ~/.codex-proxy/config.json 中已存在 Codex 配置；如需根据 ~/.codex/config.toml 重新导入，请使用 --force 覆盖"
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
                     如果尚未安装或配置 Claude Code 可以忽略，否则请检查 ~/.claude/settings.json，或在 ~/.codex-proxy/config.json 中手动添加 claude 配置"
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

    fn setup_temp_codex_home() -> PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = format!("codex-helper-test-{}", uuid::Uuid::new_v4().to_string());
        dir.push(suffix);
        std::fs::create_dir_all(&dir).expect("create temp codex home");
        unsafe {
            std::env::set_var("CODEX_HOME", &dir);
            // 将 HOME 也指向该目录，确保 proxy_home_dir()/config.json 也被隔离在测试目录中。
            std::env::set_var("HOME", &dir);
        }
        dir
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(path, content).expect("write test file");
    }

    #[test]
    fn bootstrap_from_codex_with_env_key_and_auth_json() {
        let home = setup_temp_codex_home();
        // Write config.toml with explicit env_key
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "openai"

[model_providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
env_key = "OPENAI_API_KEY"
"#;
        write_file(&cfg_path, config_text);

        // Write auth.json with matching OPENAI_API_KEY
        let auth_path = home.join("auth.json");
        let auth_text = r#"{ "OPENAI_API_KEY": "sk-test-123" }"#;
        write_file(&auth_path, auth_text);

        let mut cfg = ProxyConfig::default();
        bootstrap_from_codex(&mut cfg).expect("bootstrap_from_codex should succeed");

        assert!(!cfg.codex.configs.is_empty());
        let svc = cfg.codex.active_config().expect("active codex config");
        assert_eq!(svc.name, "openai");
        assert_eq!(svc.upstreams.len(), 1);
        let up = &svc.upstreams[0];
        assert_eq!(up.base_url, "https://api.openai.com/v1");
        assert_eq!(
            up.auth.auth_token.as_deref(),
            Some("sk-test-123"),
            "auth_token should come from OPENAI_API_KEY"
        );
    }

    #[test]
    fn bootstrap_from_codex_infers_env_key_from_auth_json_when_missing() {
        let home = setup_temp_codex_home();
        // config.toml without env_key
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "openai"

[model_providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
"#;
        write_file(&cfg_path, config_text);

        // auth.json with a single *_API_KEY field
        let auth_path = home.join("auth.json");
        let auth_text = r#"{ "OPENAI_API_KEY": "sk-test-456" }"#;
        write_file(&auth_path, auth_text);

        let mut cfg = ProxyConfig::default();
        bootstrap_from_codex(&mut cfg).expect("bootstrap_from_codex should infer env_key");

        let svc = cfg.codex.active_config().expect("active codex config");
        assert_eq!(svc.name, "openai");
        let up = &svc.upstreams[0];
        assert_eq!(
            up.auth.auth_token.as_deref(),
            Some("sk-test-456"),
            "auth_token should be inferred from unique *_API_KEY"
        );
    }

    #[test]
    fn bootstrap_from_codex_fails_when_multiple_api_keys_without_env_key() {
        let home = setup_temp_codex_home();
        // config.toml still without env_key
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "openai"

[model_providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
"#;
        write_file(&cfg_path, config_text);

        // auth.json with multiple *_API_KEY fields
        let auth_path = home.join("auth.json");
        let auth_text = r#"
{
  "OPENAI_API_KEY": "sk-test-1",
  "MISTRAL_API_KEY": "sk-test-2"
}
"#;
        write_file(&auth_path, auth_text);

        let mut cfg = ProxyConfig::default();
        let err = bootstrap_from_codex(&mut cfg).expect_err("should fail to infer unique token");
        let msg = err.to_string();
        assert!(
            msg.contains("无法从 ~/.codex/auth.json 推断唯一的 token"),
            "unexpected error message: {}",
            msg
        );
    }

    #[tokio::test]
    async fn load_or_bootstrap_for_service_writes_proxy_config() {
        let home = setup_temp_codex_home();
        // Prepare Codex CLI config and auth under CODEX_HOME/HOME
        let cfg_path = home.join("config.toml");
        let config_text = r#"
model_provider = "openai"

[model_providers.openai]
name = "OpenAI"
base_url = "https://api.openai.com/v1"
env_key = "OPENAI_API_KEY"
"#;
        write_file(&cfg_path, config_text);

        let auth_path = home.join("auth.json");
        let auth_text = r#"{ "OPENAI_API_KEY": "sk-test-789" }"#;
        write_file(&auth_path, auth_text);

        // 确保 proxy 配置文件起始不存在
        let proxy_cfg_path = super::proxy_home_dir().join("config.json");
        let _ = std::fs::remove_file(&proxy_cfg_path);

        let cfg = super::load_or_bootstrap_for_service(ServiceKind::Codex)
            .await
            .expect("load_or_bootstrap_for_service should succeed");

        // 内存中的配置应包含 openai upstream 与正确的 token
        let svc = cfg.codex.active_config().expect("active codex config");
        assert_eq!(svc.name, "openai");
        assert_eq!(svc.upstreams.len(), 1);
        assert_eq!(
            svc.upstreams[0].auth.auth_token.as_deref(),
            Some("sk-test-789")
        );

        // 并且应已将配置写入到 proxy_home_dir()/config.json
        let text = std::fs::read_to_string(&proxy_cfg_path)
            .expect("config.json should be written by load_or_bootstrap");
        let loaded: ProxyConfig =
            serde_json::from_str(&text).expect("config.json should be valid ProxyConfig");
        let svc2 = loaded.codex.active_config().expect("active codex config");
        assert_eq!(svc2.name, "openai");
        assert_eq!(
            svc2.upstreams[0].auth.auth_token.as_deref(),
            Some("sk-test-789")
        );
    }

    #[tokio::test]
    async fn probe_codex_bootstrap_detects_codex_proxy_without_backup() {
        let home = setup_temp_codex_home();
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
    }
}
