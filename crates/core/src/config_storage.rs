use super::*;
use crate::file_replace::write_bytes_file_async;

fn config_dir() -> PathBuf {
    proxy_home_dir()
}

fn config_toml_path() -> PathBuf {
    config_dir().join("config.toml")
}

fn config_toml_backup_path() -> PathBuf {
    config_dir().join("config.toml.bak")
}

fn config_backup_source_and_path() -> (PathBuf, PathBuf) {
    let toml_path = config_toml_path();
    (toml_path, config_toml_backup_path())
}

/// Return the canonical path for the current configuration contract.
pub fn config_file_path() -> PathBuf {
    config_toml_path()
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub source: HelperConfig,
}

const CONFIG_TOML_DOC_HEADER: &str = r#"# codex-helper config.toml
#
# 启动路径只接受当前 `version = 5` TOML；其他版本会被拒绝，历史
# config.json 会被忽略。
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
# codex-helper 启动路径只读取当前 `version = 5` 的 config.toml。
# 其他版本的 TOML 会被拒绝，历史 config.json 会被忽略。
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

version = 5

# 省略 --codex/--claude 时默认使用哪个服务。
# default_service = "codex"
# default_service = "claude"

# 请求体 Content-Encoding 默认自动归一化（zstd / gzip / br / deflate），并会把
# body.prompt_cache_key 作为缺省 session affinity 信号。极少数中转若必须接收
# 原始 Codex 压缩体，请在启动 helper 的环境里设置：
# CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough

# --- Relay targets（可选，本机客户端入口） ---
#
# Relay target 是本机保存的本地/远端 helper runtime 书签，给 `ch relay ...` 使用。
# 它不替代下面的 provider/routing；真正处理请求的 server runtime 仍使用自己的
# provider/routing 配置。
#
# local 是内置 target：`ch relay local` 等同于显式选择本机前台 helper。
# 命名 target 通常指 NAS、Tailscale 或 LAN 上的 codex-helper-server：
#
# [relay_targets.nas]
# service = "codex"
# proxy_url = "http://nas.local:3211"
# admin_url = "https://nas.example.com:4211"
# admin_token_env = "CODEX_HELPER_NAS_ADMIN_TOKEN"
#
# 常用命令：
#   ch relay add nas --proxy-url http://nas.local:3211 --admin-url https://nas.example.com:4211 --admin-token-env CODEX_HELPER_NAS_ADMIN_TOKEN
#   ch relay nas
#   ch relay nas --no-tui
#   ch relay nas --attach-only
#   ch relay off

# --- TUI 服务状态探针（可选，默认关闭） ---
#
# TUI 的 5 状态页可以对指定 provider 发起轻量模型请求，验证真实上游链路。
# 注意：provider 探针会产生极少 token 消耗；只有 enabled=true 且显式配置 probes 时才会运行。
#
# [ui.service_status]
# enabled = true
# refresh_interval_secs = 300
# timeout_ms = 3000
# high_latency_ms = 3000
# history_cells = 60
#
# [[ui.service_status.probes]]
# id = "primary-relay"
# provider = "openai"        # 对应 [codex.providers.openai] 或 [claude.providers.openai]
# endpoint = "default"       # 可省略；多 endpoint provider 可指定
# models = ["gpt-5.5"]       # 必填更稳妥；请求使用 max_tokens=1, stream=false
# # timeout_ms = 3000
# # high_latency_ms = 2500
#
# 兼容模式：也可以读取 UsageMonitor 风格只读 status JSON（不会消耗 token）。
# { "all_ok": true, "generated_at": 1778762578, "services": [
#   { "model": "gpt-5.5", "uptime_pct": "99.5", "last": { "ok": true, "latency_ms": 1200 },
#     "history": [{ "ts": 1778762500, "ok": true, "latency_ms": 1200 }] }
# ] }
#
# [[ui.service_status.probes]]
# id = "relay-status-json"
# url = "https://relay.example.com/api/status"
# models = ["gpt-5.5", "gpt-5.4", "gpt-5.4-mini"]
# # headers = { "x-status-token" = "use-an-env-rendered-static-token-only-if-needed" }

# --- 推荐：provider / routing 配置（v5 route graph） ---
#
# 大部分用户只需要改这一段。
#
# 说明：
# - 优先使用环境变量方式保存密钥（`*_env`），避免写入磁盘。
# - `providers` 负责账号、认证、endpoint 和标签。
# - `routing.entry` 指向入口 route node。
# - `routing.routes.*` 负责顺序、策略、分组和兜底行为。
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
# 如果 Codex 请求的模型名和中转站要求的模型名不同，可按 provider 配置 model_mapping。
# 例如 Codex 仍请求 `gpt-5.5`，但 relay 要求 `openai/gpt-5.5`：
#
# [codex.providers.relay]
# base_url = "https://relay.example/v1"
# auth_token_env = "RELAY_API_KEY"
# supported_models = { "gpt-5.5" = true }
# model_mapping = { "gpt-5.5" = "openai/gpt-5.5", "gpt-*" = "openai/gpt-*" }
#
# [codex.routing]
# entry = "main"
# affinity_policy = "fallback-sticky"
# 默认 fallback-sticky：失败切到备用上游后，同一 session 会尽量粘住已成功的备用账号，
# 对 official relay / remote compaction / encrypted conversation state 更安全。
# 如果你想每次都优先回到最高优先级 provider，可以显式改为 "preferred-group"。
# fallback_ttl_ms = 120000
# reprobe_preferred_after_ms = 30000
#
# [codex.routing.routes.main]
# strategy = "ordered-failover"
# children = ["openai", "backup"]
#
# --- 会话控制模板（profiles，可选） ---
#
# Phase 1 先支持“定义 / 列出 / 应用到会话”，暂不自动把 default_profile 绑定到新会话。
#
# [codex]
# default_profile = "daily"
#
# [codex.profiles.daily]
# reasoning_effort = "medium"
#
# [codex.profiles.fast]
# service_tier = "priority"
# reasoning_effort = "low"
#
# [codex.profiles.deep]
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

# 在 typed operator read model 的 recent_requests 中向前回看多久（毫秒）。
# codex-helper 会把 Codex 的 "thread-id" 匹配到脱敏后的 session_key。
recent_search_window_ms = 300000
# 读取 typed operator read model 的 HTTP 超时（毫秒）
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
# - retry.upstream：在当前 provider/endpoint 内，对单个 upstream 的内部重试（默认更偏向同一 upstream）。
# - retry.provider：当 upstream 层无法恢复时，决定是否切换到其他 provider candidate。
#
# 覆盖示例（可按需取消注释）：
#
# [retry.upstream]
# max_attempts = 2
# strategy = "same_upstream"
# backoff_ms = 200
# backoff_max_ms = 2000
# jitter_ms = 100
# on_status = "429,500-502,504-528,530-599"
# on_class = ["upstream_transport_error", "cloudflare_timeout", "cloudflare_challenge", "upstream_rate_limited", "upstream_overloaded"]
#
# [retry.provider]
# max_attempts = 2
# strategy = "failover"
# on_status = "401,403,404,408,429,500-599,524"
# on_class = ["upstream_transport_error", "upstream_rate_limited", "upstream_overloaded"]

# 可选：Reasoning Guard，用于拦截 Codex 上游偶发的 reasoning_tokens 异常桶。
# 默认关闭；开启后只基于上游 usage 元数据判断，不会判断答案文本是否正确。
# 运行中修改本段配置会被新请求自动加载；已在途请求继续使用它启动时的配置快照。
#
# [retry.reasoning_guard]
# enabled = true
# 固定异常桶：精确命中这些 reasoning token 数时触发 guard。
# reasoning_equals = [516, 1034, 1552]
# 序列异常桶：额外匹配 reasoning_tokens = 518*n-2；默认 n<=4，设为 0 可关闭。
# boundary_sequence_max_n = 4
# 命中后的动作：retry 改判为本地 502 并交给重试策略；block 直接拦截；observe 只记录。
# action = "retry"
# 流式响应检查方式：strict-buffer 会先完整缓冲 SSE，避免异常内容先写给客户端。
# stream_mode = "strict-buffer"
# 同一个客户端请求最多因 reasoning guard 增加多少轮上游请求。
# max_guard_retries = 1
# guard 重试预算耗尽后如何处理仍命中的响应：pass 原样放行；block 继续拦截。
# on_retry_exhausted = "pass"
# 只在这些路径上启用，避免影响非 Codex / 非 Responses 请求。
# paths = ["/v1/responses", "/responses", "/v1/chat/completions", "/chat/completions"]
# 是否把命中记录为 control trace event，便于 TUI Requests 和日志排查。
# log_matches = true

# 明确禁止重试/切换的 HTTP 状态码/范围（字符串形式）。
# 示例："413,415,422"。
# never_on_status = "413,415,422"

# 明确禁止重试/切换的错误分类（来自 codex-helper 的 classify）。
# 默认包含 "client_error_non_retryable"（常见请求格式/参数错误）。
# never_on_class = ["client_error_non_retryable"]

# 对某些失败类型施加冷却（秒）。
# upstream_rate_limited / upstream_overloaded 会优先使用 Retry-After 或 Codex usage_limit_reached
# body 中的 resets_at / resets_in_seconds；没有显式等待窗口时回落到 transport_cooldown_secs。
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

fn toml_schema_version(value: &TomlValue) -> Option<u32> {
    value
        .get("version")
        .and_then(|v| v.as_integer())
        .and_then(|value| u32::try_from(value).ok())
}

fn reject_removed_codex_compaction_config(value: &TomlValue) -> Result<()> {
    let has_removed_config = value
        .get("codex")
        .and_then(TomlValue::as_table)
        .is_some_and(|codex| codex.contains_key("compaction"));

    if has_removed_config {
        anyhow::bail!(
            "`[codex.compaction].remote_v2_downgrade` has been removed; codex-helper no longer performs remote compaction v2-to-v1 downgrade. Delete the entire `[codex.compaction]` table from config.toml."
        );
    }

    Ok(())
}

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

    write_bytes_file_async(&path, CONFIG_TOML_TEMPLATE.as_bytes()).await?;
    Ok(path)
}

pub async fn load_config() -> Result<HelperConfig> {
    Ok(load_config_with_source().await?.source)
}

pub async fn load_config_with_source() -> Result<LoadedConfig> {
    let toml_path = config_toml_path();
    if toml_path.exists() {
        let text = fs::read_to_string(&toml_path).await?;
        let raw_config = toml::from_str::<TomlValue>(&text).ok();
        let version = raw_config.as_ref().and_then(toml_schema_version);

        if version != Some(CURRENT_CONFIG_VERSION) {
            return Err(unsupported_config_error("config.toml", version));
        }

        let raw_config = raw_config.with_context(|| "parse current config.toml")?;
        reject_removed_codex_compaction_config(&raw_config)?;

        let config_source = toml::from_str::<HelperConfig>(&text)?;
        validate_helper_config(&config_source)?;
        return Ok(LoadedConfig {
            source: config_source,
        });
    }

    let source = HelperConfig::default();
    validate_helper_config(&source)?;
    Ok(LoadedConfig { source })
}

fn unsupported_config_error(source: &str, source_version: Option<u32>) -> anyhow::Error {
    let version_label = source_version
        .map(|value| value.to_string())
        .unwrap_or_else(|| "missing or invalid".to_string());
    anyhow::anyhow!(
        "{} uses unsupported config version {}; normal startup only accepts version = {}. Back up the file, then replace it with a current ~/.codex-helper/config.toml.",
        source,
        version_label,
        CURRENT_CONFIG_VERSION
    )
}

pub async fn save_helper_config(cfg: &HelperConfig) -> Result<PathBuf> {
    let mut normalized = cfg.clone();
    normalized.version = CURRENT_CONFIG_VERSION;
    validate_helper_config(&normalized)?;

    let dir = config_dir();
    fs::create_dir_all(&dir).await?;
    let path = config_toml_path();
    let (backup_source_path, backup_path) = config_backup_source_and_path();
    let body = toml::to_string_pretty(&normalized)?;
    let text = format!("{CONFIG_TOML_DOC_HEADER}\n{body}");
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
