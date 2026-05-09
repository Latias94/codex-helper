use super::bootstrap_impl::bootstrap_from_codex;
use super::*;
use crate::file_replace::write_bytes_file_async;

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

fn config_backup_source_and_path() -> (PathBuf, PathBuf) {
    let toml_path = config_toml_path();
    if toml_path.exists() {
        return (toml_path, config_toml_backup_path());
    }

    let json_path = config_path();
    if json_path.exists() {
        return (json_path, config_backup_path());
    }

    (toml_path, config_toml_backup_path())
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

const CONFIG_VERSION: u32 = 3;

fn ensure_config_version(cfg: &mut ProxyConfig) {
    if cfg.version.is_none() {
        cfg.version = Some(CONFIG_VERSION);
    }
}

const CONFIG_TOML_DOC_HEADER: &str = r#"# codex-helper config.toml
#
# 本文件可选；如果存在，codex-helper 会优先使用它（而不是 config.json）。
#
# 常用命令：
# - 生成带注释的模板：`codex-helper config init`
#
# 安全建议：
# - 尽量用环境变量保存密钥（*_env 字段，例如 auth_token_env / api_key_env），不要把 token 明文写入文件。
#
# 备注：某些命令会重写此文件；会保留本段 header，方便把说明贴近配置。
"#;

const CONFIG_TOML_TEMPLATE: &str = r#"# codex-helper config.toml
#
# codex-helper 同时支持 config.json 与 config.toml：
# - 如果 `config.toml` 存在，则优先使用它；
# - 否则使用 `config.json`（兼容旧版本）。
#
# 本模板以“可发现性”为主：包含可直接抄的示例，以及每个字段的说明。
#
# 路径：
# - Linux/macOS：`~/.codex-helper/config.toml`
# - Windows：    `%USERPROFILE%\.codex-helper\config.toml`
#
# 小贴士：
# - 生成/覆盖本模板：`codex-helper config init [--force]`
# - 新安装时：首次写入配置默认会写 TOML。

version = 3

# 省略 --codex/--claude 时默认使用哪个服务。
# default_service = "codex"
# default_service = "claude"

# --- 自动导入（可选） ---
#
# 如果你的机器上已配置 Codex CLI（存在 `~/.codex/config.toml`），`codex-helper config init`
# 会尝试自动把 Codex providers / routing 导入到本文件中，避免你手动抄写 base_url/env_key。
#
# 如果你只想生成纯模板（不导入），请使用：
#   codex-helper config init --no-import

# --- 推荐：provider / routing 配置（v3） ---
#
# 大部分用户只需要改这一段。
#
# 说明：
# - 优先使用环境变量方式保存密钥（`*_env`），避免写入磁盘。
# - `providers` 负责账号、认证、endpoint 和标签。
# - `routing` 负责顺序、策略和兜底行为。
# - 单 endpoint provider 尽量直接写 `base_url`，不要再包一层 `endpoints.default`。
#
# [codex.providers.openai]
# base_url = "https://api.openai.com/v1"
# auth_token_env = "OPENAI_API_KEY"
# tags = { vendor = "openai", region = "us" }
#
# [codex.providers.backup]
# base_url = "https://your-backup-provider.example/v1"
# auth_token_env = "BACKUP_API_KEY"
# tags = { vendor = "backup", region = "hk" }
#
# [codex.routing]
# policy = "ordered-failover"
# order = ["openai", "backup"]
# on_exhausted = "continue"
#
# --- 会话控制模板（profiles，可选） ---
#
# Phase 1 先支持“定义 / 列出 / 应用到会话”，暂不自动把 default_profile 绑定到新会话。
#
# [codex]
# default_profile = "daily"
#
# [codex.profiles.daily]
# station = "routing"
# reasoning_effort = "medium"
#
# [codex.profiles.fast]
# station = "routing"
# service_tier = "priority"
# reasoning_effort = "low"
#
# [codex.profiles.deep]
# station = "routing"
# model = "gpt-5.4"
# reasoning_effort = "high"
#
# Claude 配置在 [claude] 下结构相同。
#
# ---
#
# --- 通知集成（Codex `notify` hook） ---
#
# 可选功能，默认关闭。
# 设计目标：多 Codex 工作流下的低噪声通知（按耗时过滤 + 合并 + 限流）。
#
# 启用步骤：
# 1) 在 Codex 配置 `~/.codex/config.toml` 中添加：
#      notify = ["codex-helper", "notify", "codex"]
# 2) 在本文件中开启：
#      notify.enabled = true
#      notify.system.enabled = true
#
[notify]
# 通知总开关（system toast 与 exec 回调都受此控制）。
enabled = false

[notify.system]
# 系统通知支持：
# - Windows：toast（powershell.exe）
# - macOS：`osascript`
enabled = false

[notify.policy]
# D：按耗时过滤（毫秒）
min_duration_ms = 60000

# A：合并 + 限流（毫秒）
merge_window_ms = 10000
global_cooldown_ms = 60000
per_thread_cooldown_ms = 180000

# 在 proxy /__codex_helper/api/v1/status/recent 中向前回看多久（毫秒）。
# codex-helper 会把 Codex 的 "thread-id" 匹配到 proxy 的 FinishedRequest.session_id。
recent_search_window_ms = 300000
# 访问 recent endpoint 的 HTTP 超时（毫秒）
recent_endpoint_timeout_ms = 500

[notify.exec]
# 可选回调：执行一个命令，并把聚合后的 JSON 写到 stdin。
enabled = false
# command = ["python", "my_hook.py"]

# ---
#
# --- 重试策略（代理侧） ---
#
# 控制 codex-helper 在返回给 Codex 之前进行的内部重试。
# 注意：如果你同时开启了 Codex 自身的重试，可能会出现“双重重试”。
#
[retry]
# 策略预设（推荐）：
# - "balanced"（默认）
# - "same-upstream"（倾向同 upstream 重试，适合 CF/网络抖动）
# - "aggressive-failover"（更激进：更多尝试次数，可能增加时延/成本）
# - "cost-primary"（省钱主从：包月主线路 + 按量备选，支持回切探测）
profile = "balanced"

# 下面这些字段是“覆盖项”（在 profile 默认值之上进行覆盖）。
#
# 两层模型：
# - retry.upstream：在当前 station 已选中的 provider/endpoint 内，对单个 upstream 的内部重试（默认更偏向同一 upstream）。
# - retry.provider：当 upstream 层无法恢复时，决定是否切换到其他 upstream / 同一 station 可用的其他 provider 路径。
#
# 覆盖示例（可按需取消注释）：
#
# [retry.upstream]
# max_attempts = 2
# strategy = "same_upstream"
# backoff_ms = 200
# backoff_max_ms = 2000
# jitter_ms = 100
# on_status = "429,500-599,524"
# on_class = ["upstream_transport_error", "cloudflare_timeout", "cloudflare_challenge"]
#
# [retry.provider]
# max_attempts = 2
# strategy = "failover"
# on_status = "401,403,404,408,429,500-599,524"
# on_class = ["upstream_transport_error"]

# 明确禁止重试/切换的 HTTP 状态码/范围（字符串形式）。
# 示例："413,415,422"。
# never_on_status = "413,415,422"

# 明确禁止重试/切换的错误分类（来自 codex-helper 的 classify）。
# 默认包含 "client_error_non_retryable"（常见请求格式/参数错误）。
# never_on_class = ["client_error_non_retryable"]

# 对某些失败类型施加冷却（秒）。
# cloudflare_challenge_cooldown_secs = 300
# cloudflare_timeout_cooldown_secs = 60
# transport_cooldown_secs = 30

# 可选：冷却的指数退避（主要用于“便宜主线路不稳 → 降级到备选 → 隔一段时间探测回切”）。
#
# 启用后：同一 upstream/config 连续失败次数越多，冷却越久：
#   effective_cooldown = min(base_cooldown * factor^streak, cooldown_backoff_max_secs)
#
# factor=1 表示关闭退避（默认行为）。
# cooldown_backoff_factor = 2
# cooldown_backoff_max_secs = 600
"#;

fn insert_after_version_block(template: &str, insert: &str) -> String {
    let needle = "version = 3\n\n";
    if let Some(idx) = template.find(needle) {
        let insert_pos = idx + needle.len();
        let mut out = String::with_capacity(template.len() + insert.len() + 2);
        out.push_str(&template[..insert_pos]);
        out.push_str(insert);
        out.push('\n');
        out.push_str(&template[insert_pos..]);
        return out;
    }
    format!("{template}\n\n{insert}\n")
}

fn toml_schema_version_or_shape(text: &str) -> Option<u32> {
    let value = toml::from_str::<TomlValue>(text).ok()?;
    if let Some(version) = value
        .get("version")
        .and_then(|v| v.as_integer())
        .map(|value| value as u32)
    {
        return Some(version);
    }

    let has_routing = ["codex", "claude"].iter().any(|service| {
        value
            .get(*service)
            .and_then(|service| service.get("routing"))
            .is_some()
    });
    if has_routing { Some(3) } else { None }
}

fn codex_bootstrap_snippet() -> Result<Option<String>> {
    #[derive(Serialize)]
    struct CodexOnly<'a> {
        codex: &'a ServiceViewV3,
    }

    let mut cfg = ProxyConfig::default();
    ensure_config_version(&mut cfg);
    if bootstrap_from_codex(&mut cfg).is_err() {
        return Ok(None);
    }
    if !cfg.codex.has_stations() {
        return Ok(None);
    }

    let migrated = migrate_legacy_to_v3(&cfg)?;
    let body = toml::to_string_pretty(&CodexOnly {
        codex: &migrated.codex,
    })?;
    Ok(Some(format!(
        "# --- 自动导入：来自 ~/.codex/config.toml + auth.json ---\n{body}"
    )))
}

pub async fn init_config_toml(force: bool, import_codex: bool) -> Result<PathBuf> {
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

    let mut text = CONFIG_TOML_TEMPLATE.to_string();
    if import_codex && let Some(snippet) = codex_bootstrap_snippet()? {
        text = insert_after_version_block(&text, snippet.as_str());
    }
    write_bytes_file_async(&path, text.as_bytes()).await?;
    Ok(path)
}

pub async fn load_config() -> Result<ProxyConfig> {
    let toml_path = config_toml_path();
    if toml_path.exists() {
        let text = fs::read_to_string(&toml_path).await?;
        let version = toml_schema_version_or_shape(&text);

        let mut loaded_v3 = None;
        let mut cfg = if version == Some(3) {
            let cfg_v3 = toml::from_str::<ProxyConfigV3>(&text)?;
            let runtime = compile_v3_to_runtime(&cfg_v3)?;
            loaded_v3 = Some(cfg_v3);
            runtime
        } else if version == Some(2) {
            let cfg_v2 = toml::from_str::<ProxyConfigV2>(&text)?;
            compile_v2_to_runtime(&cfg_v2)?
        } else {
            let mut cfg = toml::from_str::<ProxyConfig>(&text)?;
            ensure_config_version(&mut cfg);
            cfg
        };
        normalize_proxy_config(&mut cfg);
        validate_proxy_config(&cfg)?;
        if version != Some(3) {
            auto_migrate_loaded_config(&mut cfg, "config.toml", version).await;
        } else if let Some(cfg_v3) = loaded_v3.as_ref() {
            auto_compact_loaded_v3_config(cfg_v3, "config.toml").await;
        }
        return Ok(cfg);
    }

    let json_path = config_path();
    if json_path.exists() {
        let bytes = fs::read(json_path).await?;
        let mut cfg = serde_json::from_slice::<ProxyConfig>(&bytes)?;
        let version = cfg.version;
        ensure_config_version(&mut cfg);
        normalize_proxy_config(&mut cfg);
        validate_proxy_config(&cfg)?;
        auto_migrate_loaded_config(&mut cfg, "config.json", version).await;
        return Ok(cfg);
    }

    let mut cfg = ProxyConfig::default();
    ensure_config_version(&mut cfg);
    normalize_proxy_config(&mut cfg);
    validate_proxy_config(&cfg)?;
    Ok(cfg)
}

async fn auto_migrate_loaded_config(
    cfg: &mut ProxyConfig,
    source: &str,
    source_version: Option<u32>,
) {
    match save_config(cfg).await {
        Ok(()) => {
            cfg.version = Some(3);
            info!(
                "auto-migrated {} from version {:?} to version 3",
                source, source_version
            );
        }
        Err(err) => {
            warn!(
                "failed to auto-migrate {} from version {:?} to version 3: {}",
                source, source_version, err
            );
        }
    }
}

fn runtime_service_manager_value(mgr: &ServiceConfigManager) -> Result<JsonValue> {
    serde_json::to_value(mgr).context("serialize runtime service manager")
}

fn v3_service_has_import_metadata(view: &ServiceViewV3) -> bool {
    view.providers.values().any(|provider| {
        provider.tags.contains_key("provider_id")
            || provider.tags.contains_key("requires_openai_auth")
            || provider
                .tags
                .get("source")
                .is_some_and(|value| value == "codex-config")
            || provider.endpoints.values().any(|endpoint| {
                endpoint.tags.contains_key("provider_id")
                    || endpoint.tags.contains_key("requires_openai_auth")
                    || endpoint
                        .tags
                        .get("source")
                        .is_some_and(|value| value == "codex-config")
            })
    })
}

async fn auto_compact_loaded_v3_config(cfg: &ProxyConfigV3, source: &str) {
    if !v3_service_has_import_metadata(&cfg.codex) && !v3_service_has_import_metadata(&cfg.claude) {
        return;
    }

    match save_config_v3(cfg).await {
        Ok(_) => {
            info!(
                "auto-compacted {} v3 provider config metadata for authoring format",
                source
            );
        }
        Err(err) => {
            warn!(
                "failed to auto-compact {} v3 provider config metadata: {}",
                source, err
            );
        }
    }
}

async fn save_existing_v3_if_only_runtime_metadata_changed(
    cfg: &ProxyConfig,
) -> Result<Option<PathBuf>> {
    let path = config_toml_path();
    if !path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&path).await?;
    if toml_schema_version_or_shape(&text) != Some(3) {
        return Ok(None);
    }

    let mut requested = cfg.clone();
    normalize_proxy_config(&mut requested);
    validate_proxy_config(&requested)?;

    let mut existing = toml::from_str::<ProxyConfigV3>(&text)?;
    let mut existing_runtime = compile_v3_to_runtime(&existing)?;
    normalize_proxy_config(&mut existing_runtime);

    if runtime_service_manager_value(&existing_runtime.codex)?
        != runtime_service_manager_value(&requested.codex)?
        || runtime_service_manager_value(&existing_runtime.claude)?
            != runtime_service_manager_value(&requested.claude)?
    {
        return Ok(None);
    }

    existing.retry = requested.retry;
    existing.notify = requested.notify;
    existing.default_service = requested.default_service;
    existing.ui = requested.ui;
    save_config_v3(&existing).await.map(Some)
}

pub async fn save_config(cfg: &ProxyConfig) -> Result<()> {
    if cfg.version == Some(3) {
        if save_existing_v3_if_only_runtime_metadata_changed(cfg)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let migrated = migrate_legacy_to_v3(cfg)?;
        save_config_v3(&migrated).await?;
        return Ok(());
    }

    let migrated = migrate_legacy_to_v3(cfg)?;
    save_config_v3(&migrated).await?;
    Ok(())
}

pub async fn save_config_v2(cfg: &ProxyConfigV2) -> Result<PathBuf> {
    let mut normalized = compact_v2_config(cfg)?;
    let mut runtime = compile_v2_to_runtime(&normalized)?;
    normalize_proxy_config(&mut runtime);
    validate_proxy_config(&runtime)?;
    normalized.version = 2;

    let dir = config_dir();
    fs::create_dir_all(&dir).await?;
    let path = config_toml_path();
    let (backup_source_path, backup_path) = config_backup_source_and_path();
    let body = toml::to_string_pretty(&normalized)?;
    let text = format!(
        "{CONFIG_TOML_DOC_HEADER}
{body}"
    );
    let data = text.into_bytes();

    if backup_source_path.exists()
        && let Err(err) = fs::copy(&backup_source_path, &backup_path).await
    {
        warn!(
            "failed to backup {:?} to {:?}: {}",
            backup_source_path, backup_path, err
        );
    }

    write_bytes_file_async(&path, &data).await?;
    Ok(path)
}

pub async fn save_config_v3(cfg: &ProxyConfigV3) -> Result<PathBuf> {
    let mut normalized = cfg.clone();
    normalized.version = 3;
    compact_v3_config_for_write(&mut normalized);
    let mut runtime = compile_v3_to_runtime(&normalized)?;
    normalize_proxy_config(&mut runtime);
    validate_proxy_config(&runtime)?;

    let dir = config_dir();
    fs::create_dir_all(&dir).await?;
    let path = config_toml_path();
    let (backup_source_path, backup_path) = config_backup_source_and_path();
    let body = toml::to_string_pretty(&normalized)?;
    let text = format!(
        "{CONFIG_TOML_DOC_HEADER}
{body}"
    );
    let data = text.into_bytes();

    if backup_source_path.exists()
        && let Err(err) = fs::copy(&backup_source_path, &backup_path).await
    {
        warn!(
            "failed to backup {:?} to {:?}: {}",
            backup_source_path, backup_path, err
        );
    }

    write_bytes_file_async(&path, &data).await?;
    Ok(path)
}

fn normalize_proxy_config(cfg: &mut ProxyConfig) {
    fn normalize_mgr(mgr: &mut ServiceConfigManager) {
        fn select_default_active_name(configs: &HashMap<String, ServiceConfig>) -> Option<String> {
            let mut items = configs.iter().collect::<Vec<_>>();
            items.sort_by(|(name_a, svc_a), (name_b, svc_b)| {
                svc_a
                    .level
                    .cmp(&svc_b.level)
                    .then_with(|| name_a.cmp(name_b))
            });
            items
                .iter()
                .find(|(_, svc)| svc.enabled)
                .map(|(name, _)| (*name).clone())
                .or_else(|| items.first().map(|(name, _)| (*name).clone()))
        }

        for (key, svc) in mgr.stations_mut() {
            if svc.name.trim().is_empty() {
                svc.name = key.clone();
            }
        }
        let normalized_active = mgr
            .active
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        mgr.active = match normalized_active {
            Some(active) if mgr.contains_station(active.as_str()) => Some(active),
            Some(active) => match active.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => select_default_active_name(mgr.stations()),
                "false" | "0" | "no" | "off" => None,
                _ => Some(active),
            },
            None => None,
        };
        mgr.default_profile = mgr
            .default_profile
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        for profile in mgr.profiles.values_mut() {
            profile.extends = profile
                .extends
                .as_ref()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            profile.station = profile
                .station
                .as_ref()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            profile.model = profile
                .model
                .as_ref()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            profile.reasoning_effort = profile
                .reasoning_effort
                .as_ref()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            profile.service_tier = profile
                .service_tier
                .as_ref()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
        }
    }

    normalize_mgr(&mut cfg.codex);
    normalize_mgr(&mut cfg.claude);
}

fn validate_proxy_config(cfg: &ProxyConfig) -> Result<()> {
    validate_service_profiles("codex", &cfg.codex)?;
    validate_service_profiles("claude", &cfg.claude)?;
    Ok(())
}
