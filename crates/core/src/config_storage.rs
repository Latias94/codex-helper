use super::*;
use crate::file_replace::{write_bytes_file_async, write_bytes_file_async_with_permissions};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

fn config_dir() -> PathBuf {
    proxy_home_dir()
}

fn config_toml_path() -> PathBuf {
    config_dir().join("config.toml")
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
# config.json 不受支持，且会在没有 canonical config.toml 时被明确拒绝。
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
# 其他版本的 TOML 会被拒绝；历史 config.json 不受支持，且不会被导入。
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

fn nested_toml_value<'a>(value: &'a TomlValue, path: &[&str]) -> Option<&'a TomlValue> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn toml_path_key(key: &str) -> String {
    if !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        key.to_string()
    } else {
        format!("{key:?}")
    }
}

fn collect_retired_profile_settings(value: &TomlValue, service: &str, retired: &mut Vec<String>) {
    let Some(profiles) =
        nested_toml_value(value, &[service, "profiles"]).and_then(TomlValue::as_table)
    else {
        return;
    };

    for (profile_name, profile) in profiles {
        if profile
            .as_table()
            .is_some_and(|profile| profile.contains_key("station"))
        {
            retired.push(format!(
                "{service}.profiles.{}.station",
                toml_path_key(profile_name)
            ));
        }
    }
}

fn collect_retired_relay_target_settings(value: &TomlValue, retired: &mut Vec<String>) {
    let Some(targets) = value.get("relay_targets").and_then(TomlValue::as_table) else {
        return;
    };

    for (target_name, target) in targets {
        let Some(target) = target.as_table() else {
            continue;
        };
        for field in ["client_preset", "responses_websocket"] {
            if target.contains_key(field) {
                retired.push(format!(
                    "relay_targets.{}.{field}",
                    toml_path_key(target_name)
                ));
            }
        }
    }
}

fn reject_retired_v5_settings(value: &TomlValue) -> Result<()> {
    let mut retired = Vec::new();
    for (path, label) in [
        (&["codex", "client_patch"][..], "codex.client_patch"),
        (&["codex", "compaction"][..], "codex.compaction"),
        (&["claude", "compaction"][..], "claude.compaction"),
        (&["ui", "usage_forecast"][..], "ui.usage_forecast"),
        (
            &["retry", "allow_cross_station_before_first_output"][..],
            "retry.allow_cross_station_before_first_output",
        ),
    ] {
        if nested_toml_value(value, path).is_some() {
            retired.push(label.to_string());
        }
    }
    collect_retired_profile_settings(value, "codex", &mut retired);
    collect_retired_profile_settings(value, "claude", &mut retired);
    collect_retired_relay_target_settings(value, &mut retired);

    if retired.is_empty() {
        return Ok(());
    }

    retired.sort();
    let labels = retired
        .iter()
        .map(|path| format!("`{path}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut compaction_guidance = String::new();
    if retired.iter().any(|path| path == "codex.compaction") {
        compaction_guidance.push_str(
            " `[codex.compaction].remote_v2_downgrade` has been removed because codex-helper no longer performs remote compaction v2-to-v1 downgrade. Delete the entire `[codex.compaction]` table.",
        );
    }
    if retired.iter().any(|path| path == "claude.compaction") {
        compaction_guidance.push_str(
            " `[claude.compaction]` was accepted by the shared v0.20.3 version 5 service schema but had no Claude runtime effect. Delete the entire `[claude.compaction]` table.",
        );
    }

    anyhow::bail!(
        "config.toml contains retired version 5 setting(s): {labels}. Each listed setting has been removed from the runtime contract. Remove or replace every listed setting before retrying; config.toml was not modified, preventing a typed save from silently deleting it.{compaction_guidance}"
    )
}

fn unsupported_legacy_json_error(path: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "{} is an unsupported legacy config source; normal startup only reads ~/.codex-helper/config.toml with version = {}. Back up config.json, run `codex-helper config init`, and copy any needed settings into TOML manually. config.json was not imported, rewritten, or deleted.",
        path.display(),
        CURRENT_CONFIG_VERSION
    )
}

#[derive(Debug)]
struct ResolvedConfigDirectory {
    logical_path: PathBuf,
    resolved_path: PathBuf,
}

impl ResolvedConfigDirectory {
    async fn inspect() -> Result<Option<Self>> {
        let logical_path = config_dir();
        let entry_metadata = match fs::symlink_metadata(&logical_path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("inspect config directory {}", logical_path.display())
                });
            }
        };
        if !entry_metadata.is_dir() && !entry_metadata.file_type().is_symlink() {
            anyhow::bail!(
                "config directory path {} is not a directory",
                logical_path.display()
            );
        }

        let resolved_path = fs::canonicalize(&logical_path)
            .await
            .with_context(|| format!("resolve config directory {}", logical_path.display()))?;
        let resolved_metadata = fs::metadata(&resolved_path).await.with_context(|| {
            format!(
                "inspect resolved config directory {}",
                resolved_path.display()
            )
        })?;
        if !resolved_metadata.is_dir() {
            anyhow::bail!(
                "config directory path {} does not resolve to a directory",
                logical_path.display()
            );
        }

        Ok(Some(Self {
            logical_path,
            resolved_path,
        }))
    }

    async fn prepare() -> Result<Self> {
        if let Some(paths) = Self::inspect().await? {
            return Ok(paths);
        }

        let logical_path = config_dir();
        fs::create_dir_all(&logical_path)
            .await
            .with_context(|| format!("create config directory {}", logical_path.display()))?;
        Self::inspect().await?.ok_or_else(|| {
            anyhow::anyhow!(
                "config directory {} disappeared after creation",
                logical_path.display()
            )
        })
    }

    fn logical_file(&self, name: &str) -> PathBuf {
        self.logical_path.join(name)
    }

    fn resolved_file(&self, name: &str) -> PathBuf {
        self.resolved_path.join(name)
    }

    async fn ensure_unchanged(&self) -> Result<()> {
        let current = fs::canonicalize(&self.logical_path)
            .await
            .with_context(|| {
                format!(
                    "re-resolve config directory {}",
                    self.logical_path.display()
                )
            })?;
        if current != self.resolved_path {
            anyhow::bail!(
                "config directory {} changed target during the operation; expected {}, found {}",
                self.logical_path.display(),
                self.resolved_path.display(),
                current.display()
            );
        }
        Ok(())
    }
}

struct ConfigMutationLock {
    _file: File,
}

impl ConfigMutationLock {
    fn acquire(paths: &ResolvedConfigDirectory) -> Result<Self> {
        let path = paths.resolved_file("config.toml.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .with_context(|| format!("open config mutation lock {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => anyhow::bail!(
                "another config mutation is already running; lock is held at {}",
                paths.logical_file("config.toml.lock").display()
            ),
            Err(TryLockError::Error(source)) => {
                Err(source).with_context(|| format!("lock config mutation path {}", path.display()))
            }
        }
    }
}

#[derive(Debug)]
struct ExistingConfigToml {
    entry_is_symlink: bool,
    permissions: std::fs::Permissions,
    contents: Vec<u8>,
}

impl ExistingConfigToml {
    fn text(&self) -> Result<&str> {
        std::str::from_utf8(&self.contents).context("config.toml is not valid UTF-8")
    }
}

async fn read_existing_config_toml(
    paths: &ResolvedConfigDirectory,
) -> Result<Option<ExistingConfigToml>> {
    let logical_path = paths.logical_file("config.toml");
    let entry_path = paths.resolved_file("config.toml");
    let entry_metadata = match fs::symlink_metadata(&entry_path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect config {}", logical_path.display()));
        }
    };

    let entry_is_symlink = entry_metadata.file_type().is_symlink();
    let source_path = if entry_is_symlink {
        fs::canonicalize(&entry_path)
            .await
            .with_context(|| format!("resolve config symlink {}", logical_path.display()))?
    } else {
        entry_path
    };
    let source_metadata = fs::metadata(&source_path)
        .await
        .with_context(|| format!("inspect config target {}", source_path.display()))?;
    if !source_metadata.is_file() {
        anyhow::bail!(
            "config path {} does not resolve to a regular file",
            logical_path.display()
        );
    }
    let contents = fs::read(&source_path)
        .await
        .with_context(|| format!("read config {}", logical_path.display()))?;
    Ok(Some(ExistingConfigToml {
        entry_is_symlink,
        permissions: source_metadata.permissions(),
        contents,
    }))
}

fn validate_current_config_toml(text: &str) -> Result<TomlValue> {
    let raw_config = toml::from_str::<TomlValue>(text).map_err(|source| {
        let location = source.span().map_or_else(
            || "unknown location".to_string(),
            |span| {
                let prefix = &text[..span.start.min(text.len())];
                let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
                let column = prefix
                    .rsplit_once('\n')
                    .map_or(prefix.len(), |(_, suffix)| suffix.len())
                    + 1;
                format!("line {line}, column {column}")
            },
        );
        anyhow::anyhow!(
            "parse current config.toml at {location}: {}",
            source.message()
        )
    })?;
    let version = toml_schema_version(&raw_config);
    if version != Some(CURRENT_CONFIG_VERSION) {
        return Err(unsupported_config_error("config.toml", version));
    }

    reject_retired_v5_settings(&raw_config)?;
    Ok(raw_config)
}

fn reject_symlink_config_mutation(existing: &ExistingConfigToml, action: &str) -> Result<()> {
    if existing.entry_is_symlink {
        anyhow::bail!(
            "refusing to {action} config.toml because it is a symbolic link; edit the link target directly or replace the link with a regular config.toml. The link and its target were not modified"
        );
    }
    Ok(())
}

async fn write_config_backup(
    paths: &ResolvedConfigDirectory,
    existing: &ExistingConfigToml,
) -> Result<()> {
    paths.ensure_unchanged().await?;
    let backup_path = paths.resolved_file("config.toml.bak");
    write_bytes_file_async_with_permissions(
        &backup_path,
        &existing.contents,
        existing.permissions.clone(),
    )
    .await
    .with_context(|| {
        format!(
            "back up config.toml to {}",
            paths.logical_file("config.toml.bak").display()
        )
    })?;
    paths.ensure_unchanged().await
}

async fn preflight_existing_config_before_save(
    paths: &ResolvedConfigDirectory,
) -> Result<Option<ExistingConfigToml>> {
    if let Some(existing) = read_existing_config_toml(paths).await? {
        validate_current_config_toml(existing.text()?)?;
        reject_symlink_config_mutation(&existing, "save")?;
        return Ok(Some(existing));
    }

    let logical_json_path = paths.logical_file("config.json");
    let json_path = paths.resolved_file("config.json");
    match fs::symlink_metadata(json_path).await {
        Ok(_) => Err(unsupported_legacy_json_error(&logical_json_path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("inspect legacy config {}", logical_json_path.display())),
    }
}

pub async fn init_config_toml(force: bool) -> Result<PathBuf> {
    let paths = ResolvedConfigDirectory::prepare().await?;
    let _lock = ConfigMutationLock::acquire(&paths)?;
    paths.ensure_unchanged().await?;
    let path = paths.logical_file("config.toml");

    let existing = read_existing_config_toml(&paths).await?;
    if existing.is_some() && !force {
        anyhow::bail!(
            "config.toml already exists at {:?}; use --force to overwrite",
            path
        );
    }

    if let Some(existing) = existing.as_ref() {
        reject_symlink_config_mutation(existing, "initialize")?;
        write_config_backup(&paths, existing).await?;
    }

    paths.ensure_unchanged().await?;
    write_bytes_file_async(
        &paths.resolved_file("config.toml"),
        CONFIG_TOML_TEMPLATE.as_bytes(),
    )
    .await?;
    paths.ensure_unchanged().await?;
    Ok(path)
}

pub async fn load_config() -> Result<HelperConfig> {
    Ok(load_config_with_source().await?.source)
}

pub async fn load_config_with_source() -> Result<LoadedConfig> {
    let Some(paths) = ResolvedConfigDirectory::inspect().await? else {
        let source = HelperConfig::default();
        validate_helper_config(&source)?;
        return Ok(LoadedConfig { source });
    };

    if let Some(existing) = read_existing_config_toml(&paths).await? {
        let text = existing.text()?;
        validate_current_config_toml(text)?;
        let config_source = toml::from_str::<HelperConfig>(text)?;
        validate_helper_config(&config_source)?;
        paths.ensure_unchanged().await?;
        return Ok(LoadedConfig {
            source: config_source,
        });
    }

    let logical_json_path = paths.logical_file("config.json");
    let json_path = paths.resolved_file("config.json");
    match fs::symlink_metadata(json_path).await {
        Ok(_) => return Err(unsupported_legacy_json_error(&logical_json_path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect legacy config {}", logical_json_path.display()));
        }
    }

    paths.ensure_unchanged().await?;
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
    let paths = ResolvedConfigDirectory::prepare().await?;
    let _lock = ConfigMutationLock::acquire(&paths)?;
    paths.ensure_unchanged().await?;
    let existing = preflight_existing_config_before_save(&paths).await?;

    let mut normalized = cfg.clone();
    normalized.version = CURRENT_CONFIG_VERSION;
    validate_helper_config(&normalized)?;

    let path = paths.logical_file("config.toml");
    let body = toml::to_string_pretty(&normalized)?;
    let text = format!("{CONFIG_TOML_DOC_HEADER}\n{body}");
    let data = text.into_bytes();

    if let Some(existing) = existing.as_ref() {
        write_config_backup(&paths, existing).await?;
    }

    paths.ensure_unchanged().await?;
    write_bytes_file_async(&paths.resolved_file("config.toml"), &data).await?;
    paths.ensure_unchanged().await?;
    Ok(path)
}
