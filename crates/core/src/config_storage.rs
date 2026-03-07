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

version = 1

# 省略 --codex/--claude 时默认使用哪个服务。
# default_service = "codex"
# default_service = "claude"

# --- 自动导入（可选） ---
#
# 如果你的机器上已配置 Codex CLI（存在 `~/.codex/config.toml`），`codex-helper config init`
# 会尝试自动把 Codex providers 导入到本文件中，避免你手动抄写 base_url/env_key。
#
# 如果你只想生成纯模板（不导入），请使用：
#   codex-helper config init --no-import

# --- 通用：上游配置（账号 / API Key） ---
#
# 大部分用户只需要改这一段。
#
# 说明：
# - 优先使用环境变量方式保存密钥（`*_env`），避免写入磁盘。
# - 单个 config 内可配置多个 `[[...upstreams]]`，用于“同账号多 endpoint 自动切换”。
# - 可选：给每个 config 设置 `level`（1..=10）用于“按 level 分组跨配置降级”（只有存在多个不同 level 时才会生效）。
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
# # 主线路 upstream
# [[codex.configs.codex-main.upstreams]]
# base_url = "https://api.openai.com/v1"
# [codex.configs.codex-main.upstreams.auth]
# auth_token_env = "OPENAI_API_KEY"
# # or: api_key_env = "OPENAI_API_KEY"
# # （不推荐）auth_token = "sk-..."
# [codex.configs.codex-main.upstreams.tags]
# provider_id = "openai"
#
# # 备份线路 upstream
# [[codex.configs.codex-main.upstreams]]
# base_url = "https://your-backup-provider.example/v1"
# [codex.configs.codex-main.upstreams.auth]
# auth_token_env = "BACKUP_API_KEY"
# [codex.configs.codex-main.upstreams.tags]
# provider_id = "backup"
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

# 在 proxy /__codex_helper/status/recent 中向前回看多久（毫秒）。
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
# - retry.upstream：在当前 provider/config 内，对单个 upstream 的内部重试（默认更偏向同一 upstream）。
# - retry.provider：当 upstream 层无法恢复时，决定是否切换到其他 upstream / 其他同级 config/provider。
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

# 兼容说明：旧版扁平字段（max_attempts/on_status/strategy/...）仍可解析，默认映射到 retry.upstream.*。

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
    let needle = "version = 1\n\n";
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

fn codex_bootstrap_snippet() -> Result<Option<String>> {
    #[derive(Serialize)]
    struct CodexOnly<'a> {
        codex: &'a ServiceConfigManager,
    }

    let mut cfg = ProxyConfig::default();
    ensure_config_version(&mut cfg);
    if bootstrap_from_codex(&mut cfg).is_err() {
        return Ok(None);
    }
    if cfg.codex.configs.is_empty() {
        return Ok(None);
    }

    let body = toml::to_string_pretty(&CodexOnly { codex: &cfg.codex })?;
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
        let mut cfg = toml::from_str::<ProxyConfig>(&text)?;
        ensure_config_version(&mut cfg);
        normalize_proxy_config(&mut cfg);
        return Ok(cfg);
    }

    let json_path = config_path();
    if json_path.exists() {
        let bytes = fs::read(json_path).await?;
        let mut cfg = serde_json::from_slice::<ProxyConfig>(&bytes)?;
        ensure_config_version(&mut cfg);
        normalize_proxy_config(&mut cfg);
        return Ok(cfg);
    }

    let mut cfg = ProxyConfig::default();
    ensure_config_version(&mut cfg);
    normalize_proxy_config(&mut cfg);
    Ok(cfg)
}

pub async fn save_config(cfg: &ProxyConfig) -> Result<()> {
    let mut cfg = cfg.clone();
    ensure_config_version(&mut cfg);
    normalize_proxy_config(&mut cfg);

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

    write_bytes_file_async(&path, &data).await?;
    Ok(())
}

fn normalize_proxy_config(cfg: &mut ProxyConfig) {
    fn normalize_mgr(mgr: &mut ServiceConfigManager) {
        for (key, svc) in mgr.configs.iter_mut() {
            if svc.name.trim().is_empty() {
                svc.name = key.clone();
            }
        }
    }

    normalize_mgr(&mut cfg.codex);
    normalize_mgr(&mut cfg.claude);
}
