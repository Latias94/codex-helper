use super::*;
use crate::file_replace::{
    AtomicWriteError, write_bytes_file_async,
    write_bytes_file_async_with_permissions_and_before_replace,
};
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
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

#[derive(Debug, Clone)]
pub struct ConfigInitOutcome {
    pub path: PathBuf,
    pub migration_report: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigMigrationFormat {
    Toml,
    Json,
}

#[derive(Debug)]
struct ConfigMigrationPlan {
    source_name: &'static str,
    source_version: Option<u64>,
    requires_write: bool,
    source: ExistingConfigToml,
    target_path: PathBuf,
    backup_path: PathBuf,
    rendered: String,
    preview: String,
    notices: Vec<String>,
}

impl ConfigMigrationPlan {
    fn report(&self, wrote: bool) -> String {
        let version = self
            .source_version
            .map(|version| version.to_string())
            .unwrap_or_else(|| "unversioned".to_string());
        let mut report = if !self.requires_write {
            format!(
                "{} already uses version = {}; no files were written.\n",
                self.source_name, CURRENT_CONFIG_VERSION
            )
        } else if wrote {
            format!(
                "Migrated {} schema {} to version = {} at {}.\nBackup: {}\n",
                self.source_name,
                version,
                CURRENT_CONFIG_VERSION,
                self.target_path.display(),
                self.backup_path.display(),
            )
        } else {
            format!(
                "Dry-run: {} schema {} can be migrated to version = {}; no files were written.\nTarget: {}\n",
                self.source_name,
                version,
                CURRENT_CONFIG_VERSION,
                self.target_path.display(),
            )
        };

        for notice in &self.notices {
            report.push_str("warning: ");
            report.push_str(notice);
            report.push('\n');
        }

        if !wrote {
            report.push('\n');
            report.push_str(&self.preview);
            if !self.preview.ends_with('\n') {
                report.push('\n');
            }
        }
        report
    }
}

impl LoadedConfig {
    /// Preview or explicitly apply migration of the on-disk configuration to the current contract.
    ///
    /// This shares the validated migration path used by normal startup. Preview mode never
    /// writes configuration, and neither mode reads or mutates runtime SQLite state.
    pub async fn migrate_config_file(write: bool) -> Result<String> {
        let Some(paths) = ResolvedConfigDirectory::inspect().await? else {
            anyhow::bail!(
                "no codex-helper configuration directory exists at {}; run `codex-helper config init` for a new installation",
                config_dir().display()
            );
        };

        if write {
            let _lock = ConfigMutationLock::try_acquire(&paths)?;
            paths.ensure_unchanged().await?;
            let plan = build_config_migration_plan(&paths).await?;
            apply_config_migration_plan(&paths, &plan).await?;
            Ok(plan.report(true))
        } else {
            let plan = build_config_migration_plan(&paths).await?;
            paths.ensure_unchanged().await?;
            Ok(plan.report(false))
        }
    }
}

const CONFIG_TOML_DOC_HEADER: &str = r#"# codex-helper config.toml
#
# 启动路径最终只加载当前 `version = 6` TOML。发现旧版本/无版本
# TOML，或在没有 canonical config.toml 时发现 config.json，会先自动
# 校验迁移并把源文件保存为对应的 `.bak`；未来版本和损坏文件会拒绝启动。
#
# 常用命令：
# - 生成带注释的模板：`codex-helper config init`
# - 预览迁移：`codex-helper config migrate --dry-run`
# - 确认写入：`codex-helper config migrate --write --yes`
#
# 安全建议：
# - 尽量用环境变量保存密钥（*_env 字段，例如 auth_token_env / api_key_env），不要把 token 明文写入文件。
#
# 备注：某些命令会重写此文件；会保留本段 header，方便把说明贴近配置。
"#;

const CONFIG_TOML_TEMPLATE: &str = r#"# codex-helper config.toml
#
# codex-helper 启动路径最终只读取当前 `version = 6` 的 config.toml。
# 旧版本/无版本 TOML 和没有 canonical TOML 时的 config.json 会自动迁移，
# 写入前保留 `.bak`；未来版本或损坏文件不会被猜测降级。
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

version = 6

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

# --- 推荐：provider / routing 配置（version 6 route graph） ---
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
# # 本地用户服务也可显式引用 native store；值不会写入本文件：
# # auth_token_ref = { source = "native", name = "openai.primary" }
# # headless/Docker 可显式引用只读挂载的绝对路径：
# # auth_token_ref = { source = "secret_file", path = "/run/secrets/openai-token" }
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
# 并发上限不同的 relay 可以使用容量加权 round-robin。下面的两个
# provider 会按剩余本地容量分配新 session（空闲时约为 20:15），
# 成功后仍由 session affinity 保持同一个 provider；一个 key 可以服务多个 session。
# 要启用这个示例，请把上方 [codex.routing] 的 entry 改成 `relay_pool`。
#
# [codex.providers.input]
# base_url = "https://input.example/v1"
# auth_token_env = "INPUT_API_KEY"
# [codex.providers.input.limits]
# max_concurrent_requests = 20
#
# [codex.providers.ciii]
# base_url = "https://ciii.example/v1"
# auth_token_env = "CIII_API_KEY"
# [codex.providers.ciii.limits]
# max_concurrent_requests = 15
#
# [codex.routing.routes.relay_pool]
# strategy = "round-robin"
# children = ["input", "ciii"]
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

fn toml_schema_version(value: &TomlValue, source_name: &str) -> Result<Option<u64>> {
    let Some(raw_version) = value.get("version") else {
        return Ok(None);
    };
    let Some(version) = raw_version.as_integer() else {
        anyhow::bail!(
            "{source_name} has invalid config version {raw_version:?}; `version` must be a positive integer"
        );
    };
    let Ok(version) = u64::try_from(version) else {
        anyhow::bail!(
            "{source_name} has invalid config version {version}; `version` must be a positive integer"
        );
    };
    if version == 0 {
        anyhow::bail!(
            "{source_name} has invalid config version 0; `version` must be a positive integer"
        );
    }
    Ok(Some(version))
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

const RETIRED_V5_REMOVALS: &[(&[&str], &str)] = &[
    (&["codex", "client_patch"], "codex.client_patch"),
    (&["codex", "compaction"], "codex.compaction"),
    (&["claude", "compaction"], "claude.compaction"),
    (&["ui", "usage_forecast"], "ui.usage_forecast"),
    (
        &["retry", "allow_cross_station_before_first_output"],
        "retry.allow_cross_station_before_first_output",
    ),
];

const LEGACY_FLAT_RETRY_FIELDS: &[&str] = &[
    "max_attempts",
    "backoff_ms",
    "backoff_max_ms",
    "jitter_ms",
    "on_status",
    "on_class",
    "strategy",
];

fn collect_retired_v5_settings(value: &TomlValue) -> Vec<String> {
    let mut retired = Vec::new();
    for &(path, label) in RETIRED_V5_REMOVALS {
        if nested_toml_value(value, path).is_some() {
            retired.push(label.to_string());
        }
    }
    for field in LEGACY_FLAT_RETRY_FIELDS {
        if nested_toml_value(value, &["retry", field]).is_some() {
            retired.push(format!("retry.{field}"));
        }
    }
    collect_retired_profile_settings(value, "codex", &mut retired);
    collect_retired_profile_settings(value, "claude", &mut retired);
    collect_retired_relay_target_settings(value, &mut retired);

    retired.sort();
    retired
}

fn reject_retired_v5_settings(value: &TomlValue) -> Result<()> {
    let retired = collect_retired_v5_settings(value);

    if retired.is_empty() {
        return Ok(());
    }

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

fn remove_toml_path(value: &mut TomlValue, path: &[&str]) -> bool {
    let Some((head, tail)) = path.split_first() else {
        return false;
    };
    let Some(table) = value.as_table_mut() else {
        return false;
    };
    if tail.is_empty() {
        return table.remove(*head).is_some();
    }
    table
        .get_mut(*head)
        .is_some_and(|child| remove_toml_path(child, tail))
}

fn remove_retired_settings(value: &mut TomlValue, notices: &mut Vec<String>) {
    for &(path, label) in RETIRED_V5_REMOVALS {
        if remove_toml_path(value, path) {
            notices.push(format!("removed retired setting `{label}`"));
        }
    }

    for service in ["codex", "claude"] {
        let profile_names = nested_toml_value(value, &[service, "profiles"])
            .and_then(TomlValue::as_table)
            .map(|profiles| profiles.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for profile_name in profile_names {
            let path = [service, "profiles", profile_name.as_str(), "station"];
            if remove_toml_path(value, &path) {
                notices.push(format!(
                    "removed retired setting `{service}.profiles.{}.station`; route graph routing now owns provider selection",
                    toml_path_key(&profile_name)
                ));
            }
        }
    }

    let target_names = value
        .get("relay_targets")
        .and_then(TomlValue::as_table)
        .map(|targets| targets.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for target_name in target_names {
        for field in ["client_preset", "responses_websocket"] {
            let path = ["relay_targets", target_name.as_str(), field];
            if remove_toml_path(value, &path) {
                notices.push(format!(
                    "removed retired setting `relay_targets.{}.{field}`",
                    toml_path_key(&target_name)
                ));
            }
        }
    }
}

fn json_value_to_toml(
    value: serde_json::Value,
    path: &str,
    notices: &mut Vec<String>,
) -> Result<Option<TomlValue>> {
    let converted = match value {
        serde_json::Value::Null => {
            notices.push(format!("ignored JSON null at `{path}`"));
            return Ok(None);
        }
        serde_json::Value::Bool(value) => TomlValue::Boolean(value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                TomlValue::Integer(value)
            } else if let Some(value) = value.as_u64() {
                TomlValue::Integer(i64::try_from(value).with_context(|| {
                    format!("JSON integer at `{path}` exceeds the TOML integer range")
                })?)
            } else {
                TomlValue::Float(value.as_f64().with_context(|| {
                    format!("JSON number at `{path}` is not representable as TOML")
                })?)
            }
        }
        serde_json::Value::String(value) => TomlValue::String(value),
        serde_json::Value::Array(values) => {
            let mut converted = Vec::with_capacity(values.len());
            for (index, value) in values.into_iter().enumerate() {
                let item_path = format!("{path}[{index}]");
                if let Some(value) = json_value_to_toml(value, &item_path, notices)? {
                    converted.push(value);
                }
            }
            TomlValue::Array(converted)
        }
        serde_json::Value::Object(values) => {
            let mut converted = toml::map::Map::new();
            for (key, value) in values {
                let item_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if let Some(value) = json_value_to_toml(value, &item_path, notices)? {
                    converted.insert(key, value);
                }
            }
            TomlValue::Table(converted)
        }
    };
    Ok(Some(converted))
}

fn parse_migration_source(
    format: ConfigMigrationFormat,
    source_name: &str,
    contents: &[u8],
    notices: &mut Vec<String>,
) -> Result<TomlValue> {
    match format {
        ConfigMigrationFormat::Toml => {
            let text = std::str::from_utf8(contents)
                .with_context(|| format!("{source_name} is not valid UTF-8"))?;
            toml::from_str(text).with_context(|| format!("parse legacy {source_name}"))
        }
        ConfigMigrationFormat::Json => {
            let value = serde_json::from_slice(contents)
                .with_context(|| format!("parse legacy {source_name}"))?;
            super::legacy_json_impl::validate_json_migration_source(&value, source_name)?;
            json_value_to_toml(value, "", notices)?.with_context(|| {
                format!("legacy {source_name} contains only null and cannot be migrated")
            })
        }
    }
}

fn service_has_legacy_station_shape(service: &toml::map::Map<String, TomlValue>) -> bool {
    service.contains_key("configs")
        || service
            .get("stations")
            .and_then(TomlValue::as_table)
            .is_some_and(|stations| {
                stations.values().any(|station| {
                    station
                        .as_table()
                        .is_some_and(|station| station.contains_key("upstreams"))
                })
            })
}

fn has_legacy_station_shape(value: &TomlValue) -> bool {
    ["codex", "claude"].iter().any(|service| {
        value
            .get(*service)
            .and_then(TomlValue::as_table)
            .is_some_and(service_has_legacy_station_shape)
    })
}

fn service_has_v2_station_shape(service: &toml::map::Map<String, TomlValue>) -> bool {
    service.contains_key("groups")
        || service.contains_key("active_group")
        || service.contains_key("active_station")
        || service
            .get("stations")
            .and_then(TomlValue::as_table)
            .is_some_and(|stations| {
                stations.values().any(|station| {
                    station
                        .as_table()
                        .is_some_and(|station| station.contains_key("members"))
                })
            })
}

fn has_v2_station_shape(value: &TomlValue) -> bool {
    ["codex", "claude"].iter().any(|service| {
        value
            .get(*service)
            .and_then(TomlValue::as_table)
            .is_some_and(service_has_v2_station_shape)
    })
}

fn service_has_legacy_v3_routing_shape(service: &toml::map::Map<String, TomlValue>) -> bool {
    service
        .get("routing")
        .and_then(TomlValue::as_table)
        .is_some_and(|routing| {
            !routing.contains_key("routes")
                && ["policy", "order", "target", "prefer_tags", "chain", "pools"]
                    .iter()
                    .any(|key| routing.contains_key(*key))
        })
}

fn has_legacy_v3_routing_shape(value: &TomlValue) -> bool {
    ["codex", "claude"].iter().any(|service| {
        value
            .get(*service)
            .and_then(TomlValue::as_table)
            .is_some_and(service_has_legacy_v3_routing_shape)
    })
}

fn toml_config_requires_migration(value: &TomlValue, version: Option<u64>) -> bool {
    version != Some(u64::from(CURRENT_CONFIG_VERSION))
        || !collect_retired_v5_settings(value).is_empty()
        || has_legacy_station_shape(value)
        || has_v2_station_shape(value)
        || has_legacy_v3_routing_shape(value)
}

fn inferred_migration_schema_version(value: &TomlValue) -> Option<u64> {
    if has_legacy_station_shape(value) {
        return Some(1);
    }
    if has_v2_station_shape(value) {
        return Some(2);
    }
    if has_legacy_v3_routing_shape(value) {
        return Some(3);
    }
    if ["codex", "claude"].iter().any(|service| {
        value
            .get(*service)
            .and_then(|service| service.get("routing"))
            .and_then(TomlValue::as_table)
            .is_some_and(|routing| routing.contains_key("entry") || routing.contains_key("routes"))
    }) {
        return Some(4);
    }
    None
}

fn validate_legacy_value<T>(value: &TomlValue, path: &str, expected: &str) -> Result<()>
where
    T: serde::de::DeserializeOwned,
{
    value.clone().try_into::<T>().map(|_| ()).map_err(|source| {
        anyhow::anyhow!(
            "legacy configuration field `{path}` must be {expected}; migration was not written: {source}"
        )
    })
}

fn validate_optional_legacy_field<T>(
    table: &toml::map::Map<String, TomlValue>,
    field: &str,
    parent_path: &str,
    expected: &str,
) -> Result<()>
where
    T: serde::de::DeserializeOwned,
{
    if let Some(value) = table.get(field) {
        validate_legacy_value::<T>(value, &format!("{parent_path}.{field}"), expected)?;
    }
    Ok(())
}

fn reject_ambiguous_legacy_fields(
    table: &toml::map::Map<String, TomlValue>,
    allowed: &[&str],
    parent_path: &str,
) -> Result<()> {
    let mut unknown = table
        .keys()
        .filter(|field| !allowed.contains(&field.as_str()))
        .map(|field| format!("{parent_path}.{}", toml_path_key(field)))
        .collect::<Vec<_>>();
    unknown.sort();
    if !unknown.is_empty() {
        anyhow::bail!(
            "legacy configuration field(s) {} cannot be migrated without guessing their new ownership; remove or migrate them explicitly before retrying",
            unknown
                .iter()
                .map(|path| format!("`{path}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

fn validate_legacy_station_service(service_name: &str, service: &TomlValue) -> Result<()> {
    let service = service
        .as_table()
        .with_context(|| format!("legacy configuration field `{service_name}` must be a table"))?;
    validate_optional_legacy_field::<String>(service, "active", service_name, "a string")?;

    if service.contains_key("configs") && service.contains_key("stations") {
        anyhow::bail!(
            "legacy configuration service `{service_name}` cannot define both `configs` and its `stations` alias"
        );
    }
    if let Some(active) = service.get("active").and_then(TomlValue::as_str) {
        let stations = service
            .get("configs")
            .or_else(|| service.get("stations"))
            .and_then(TomlValue::as_table);
        if !stations.is_some_and(|stations| stations.contains_key(active)) {
            anyhow::bail!(
                "legacy configuration field `{service_name}.active` references missing station {active:?}"
            );
        }
    }
    for container_name in ["configs", "stations"] {
        let Some(stations) = service.get(container_name) else {
            continue;
        };
        let container_path = format!("{service_name}.{container_name}");
        let stations = stations.as_table().with_context(|| {
            format!("legacy configuration field `{container_path}` must be a table")
        })?;
        for (station_name, station) in stations {
            let station_path = format!("{container_path}.{}", toml_path_key(station_name));
            let station = station.as_table().with_context(|| {
                format!("legacy configuration field `{station_path}` must be a table")
            })?;
            reject_ambiguous_legacy_fields(
                station,
                &["name", "alias", "enabled", "level", "upstreams"],
                &station_path,
            )?;
            validate_optional_legacy_field::<String>(station, "name", &station_path, "a string")?;
            validate_optional_legacy_field::<String>(station, "alias", &station_path, "a string")?;
            validate_optional_legacy_field::<bool>(station, "enabled", &station_path, "a boolean")?;
            validate_optional_legacy_field::<u8>(
                station,
                "level",
                &station_path,
                "an integer from 0 through 255",
            )?;
            validate_optional_legacy_field::<Vec<UpstreamConfig>>(
                station,
                "upstreams",
                &station_path,
                "an array of legacy upstream tables",
            )?;
        }
    }
    Ok(())
}

fn validate_v2_service(service_name: &str, service: &TomlValue) -> Result<()> {
    let service = service
        .as_table()
        .with_context(|| format!("legacy configuration field `{service_name}` must be a table"))?;
    if service.contains_key("active_group") && service.contains_key("active_station") {
        anyhow::bail!(
            "legacy configuration service `{service_name}` cannot define both `active_group` and its `active_station` alias"
        );
    }
    validate_optional_legacy_field::<String>(service, "active_group", service_name, "a string")?;
    validate_optional_legacy_field::<String>(service, "active_station", service_name, "a string")?;

    if service.contains_key("groups") && service.contains_key("stations") {
        anyhow::bail!(
            "legacy configuration service `{service_name}` cannot define both `groups` and its `stations` alias"
        );
    }
    let active = service
        .get("active_group")
        .or_else(|| service.get("active_station"))
        .and_then(TomlValue::as_str);
    if let Some(active) = active {
        let groups = service
            .get("groups")
            .or_else(|| service.get("stations"))
            .and_then(TomlValue::as_table);
        if !groups.is_some_and(|groups| groups.contains_key(active)) {
            anyhow::bail!(
                "legacy configuration active group for `{service_name}` references missing group/station {active:?}"
            );
        }
    }
    for container_name in ["groups", "stations"] {
        let Some(groups) = service.get(container_name) else {
            continue;
        };
        let container_path = format!("{service_name}.{container_name}");
        let groups = groups.as_table().with_context(|| {
            format!("legacy configuration field `{container_path}` must be a table")
        })?;
        for (group_name, group) in groups {
            let group_path = format!("{container_path}.{}", toml_path_key(group_name));
            let group = group.as_table().with_context(|| {
                format!("legacy configuration field `{group_path}` must be a table")
            })?;
            reject_ambiguous_legacy_fields(
                group,
                &["alias", "enabled", "level", "members"],
                &group_path,
            )?;
            validate_optional_legacy_field::<String>(group, "alias", &group_path, "a string")?;
            validate_optional_legacy_field::<bool>(group, "enabled", &group_path, "a boolean")?;
            validate_optional_legacy_field::<u8>(
                group,
                "level",
                &group_path,
                "an integer from 0 through 255",
            )?;

            let Some(members) = group.get("members") else {
                continue;
            };
            let members_path = format!("{group_path}.members");
            let members = members.as_array().with_context(|| {
                format!("legacy configuration field `{members_path}` must be an array")
            })?;
            for (index, member) in members.iter().enumerate() {
                let member_path = format!("{members_path}[{index}]");
                let member = member.as_table().with_context(|| {
                    format!("legacy configuration field `{member_path}` must be a table")
                })?;
                reject_ambiguous_legacy_fields(
                    member,
                    &["provider", "endpoint_names", "endpoints", "preferred"],
                    &member_path,
                )?;
                let provider = member.get("provider").with_context(|| {
                    format!("legacy configuration field `{member_path}.provider` is required")
                })?;
                validate_legacy_value::<String>(
                    provider,
                    &format!("{member_path}.provider"),
                    "a string",
                )?;
                if member.contains_key("endpoint_names") && member.contains_key("endpoints") {
                    anyhow::bail!(
                        "legacy configuration member `{member_path}` cannot define both `endpoint_names` and its `endpoints` alias"
                    );
                }
                validate_optional_legacy_field::<Vec<String>>(
                    member,
                    "endpoint_names",
                    &member_path,
                    "an array of strings",
                )?;
                validate_optional_legacy_field::<Vec<String>>(
                    member,
                    "endpoints",
                    &member_path,
                    "an array of strings",
                )?;
                validate_optional_legacy_field::<bool>(
                    member,
                    "preferred",
                    &member_path,
                    "a boolean",
                )?;
            }
        }
    }
    Ok(())
}

const LEGACY_V3_ROUTING_FIELDS: &[&str] = &[
    "policy",
    "order",
    "target",
    "prefer_tags",
    "chain",
    "pools",
    "on_exhausted",
];

fn validate_v3_routing(service_name: &str, service: &TomlValue) -> Result<()> {
    let Some(routing) = service.get("routing") else {
        return Ok(());
    };
    let routing_path = format!("{service_name}.routing");
    let routing = routing
        .as_table()
        .with_context(|| format!("legacy configuration field `{routing_path}` must be a table"))?;
    validate_optional_legacy_field::<String>(routing, "policy", &routing_path, "a string")?;
    validate_optional_legacy_field::<Vec<String>>(
        routing,
        "order",
        &routing_path,
        "an array of strings",
    )?;
    validate_optional_legacy_field::<String>(routing, "target", &routing_path, "a string")?;
    validate_optional_legacy_field::<Vec<BTreeMap<String, String>>>(
        routing,
        "prefer_tags",
        &routing_path,
        "an array of string maps",
    )?;
    validate_optional_legacy_field::<Vec<String>>(
        routing,
        "chain",
        &routing_path,
        "an array of strings",
    )?;
    validate_optional_legacy_field::<String>(routing, "on_exhausted", &routing_path, "a string")?;

    let Some(pools) = routing.get("pools") else {
        return Ok(());
    };
    let pools_path = format!("{routing_path}.pools");
    let pools = pools
        .as_table()
        .with_context(|| format!("legacy configuration field `{pools_path}` must be a table"))?;
    for (pool_name, pool) in pools {
        let pool_path = format!("{pools_path}.{}", toml_path_key(pool_name));
        let pool = pool
            .as_table()
            .with_context(|| format!("legacy configuration field `{pool_path}` must be a table"))?;
        reject_ambiguous_legacy_fields(pool, &["providers"], &pool_path)?;
        validate_optional_legacy_field::<Vec<String>>(
            pool,
            "providers",
            &pool_path,
            "an array of strings",
        )?;
    }
    Ok(())
}

fn validate_legacy_migration_input(value: &TomlValue, explicit_version: Option<u64>) -> Result<()> {
    for service_name in ["codex", "claude"] {
        let Some(service_value) = value.get(service_name) else {
            continue;
        };
        if let Some(service) = service_value.as_table() {
            let service_has_v1_shape = service_has_legacy_station_shape(service);
            let service_has_v2_shape = service_has_v2_station_shape(service);
            let validate_v1 = service_has_v1_shape || explicit_version == Some(1);
            let validate_v2 = service_has_v2_shape || explicit_version == Some(2);

            if validate_v1 {
                let conflicting = [
                    "providers",
                    "routing",
                    "groups",
                    "active_group",
                    "active_station",
                ]
                .into_iter()
                .filter(|field| service.contains_key(*field))
                .collect::<Vec<_>>();
                if !conflicting.is_empty() {
                    anyhow::bail!(
                        "legacy configuration service `{service_name}` mixes v1 station ownership with field(s) {}; migration would overwrite current configuration",
                        conflicting.join(", ")
                    );
                }
            }
            if validate_v2 {
                if service.contains_key("routing") {
                    anyhow::bail!(
                        "legacy configuration service `{service_name}` mixes v2 group ownership with routing; migration would overwrite current configuration"
                    );
                }
                if service_has_v1_shape {
                    anyhow::bail!(
                        "legacy configuration service `{service_name}` mixes v1 station and v2 group ownership; migration was not written"
                    );
                }
            }
            if validate_v1 {
                validate_legacy_station_service(service_name, service_value)?;
            }
            if validate_v2 {
                validate_v2_service(service_name, service_value)?;
            }
            if service_has_legacy_v3_routing_shape(service) || explicit_version == Some(3) {
                validate_v3_routing(service_name, service_value)?;
            }
        } else if matches!(explicit_version, Some(1..=3)) {
            anyhow::bail!("legacy configuration field `{service_name}` must be a table");
        }
    }
    Ok(())
}

const CREDENTIAL_REFERENCE_SCHEMA_VERSION: u64 = 6;
const CREDENTIAL_REFERENCE_FIELDS: &[&str] = &["auth_token_ref", "api_key_ref"];

fn collect_auth_reference_fields(value: &TomlValue, path: &str, references: &mut Vec<String>) {
    let Some(table) = value.as_table() else {
        return;
    };
    for field in CREDENTIAL_REFERENCE_FIELDS {
        if table.contains_key(*field) {
            references.push(format!("{path}.{field}"));
        }
    }
}

fn collect_provider_credential_reference_paths(
    service_name: &str,
    service: &TomlValue,
    references: &mut Vec<String>,
) {
    let Some(service) = service.as_table() else {
        return;
    };

    if let Some(providers) = service.get("providers").and_then(TomlValue::as_table) {
        for (provider_name, provider) in providers {
            let provider_path =
                format!("{service_name}.providers.{}", toml_path_key(provider_name));
            collect_auth_reference_fields(provider, &provider_path, references);
            if let Some(auth) = provider.get("auth") {
                collect_auth_reference_fields(auth, &format!("{provider_path}.auth"), references);
            }
        }
    }

    for container_name in ["configs", "stations"] {
        let Some(stations) = service.get(container_name).and_then(TomlValue::as_table) else {
            continue;
        };
        for (station_name, station) in stations {
            let Some(upstreams) = station.get("upstreams").and_then(TomlValue::as_array) else {
                continue;
            };
            for (index, upstream) in upstreams.iter().enumerate() {
                let upstream_path = format!(
                    "{service_name}.{container_name}.{}.upstreams[{index}]",
                    toml_path_key(station_name)
                );
                collect_auth_reference_fields(upstream, &upstream_path, references);
                let Some(auth) = upstream.get("auth") else {
                    continue;
                };
                collect_auth_reference_fields(auth, &format!("{upstream_path}.auth"), references);
            }
        }
    }
}

fn reject_pre_v6_credential_references(
    value: &TomlValue,
    explicit_version: Option<u64>,
) -> Result<()> {
    if explicit_version.is_some_and(|version| version >= CREDENTIAL_REFERENCE_SCHEMA_VERSION) {
        return Ok(());
    }

    let mut references = Vec::new();
    for service_name in ["codex", "claude"] {
        if let Some(service) = value.get(service_name) {
            collect_provider_credential_reference_paths(service_name, service, &mut references);
        }
    }
    references.sort();
    references.dedup();
    if references.is_empty() {
        return Ok(());
    }

    let source_version = explicit_version
        .map(|version| version.to_string())
        .unwrap_or_else(|| "unversioned".to_string());
    anyhow::bail!(
        "configuration schema {source_version} contains version 6 credential reference field(s) {}; migration was not written because a pre-v6 binary could ignore these fields",
        references
            .iter()
            .map(|path| format!("`{path}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn migrate_flat_retry_settings(value: &mut TomlValue, notices: &mut Vec<String>) -> Result<()> {
    let Some(retry) = value.get_mut("retry") else {
        return Ok(());
    };
    let retry = retry
        .as_table_mut()
        .context("legacy configuration field `retry` must be a table")?;
    let mut flat = toml::map::Map::new();
    for field in LEGACY_FLAT_RETRY_FIELDS {
        if let Some(value) = retry.remove(*field) {
            flat.insert((*field).to_string(), value);
        }
    }
    if flat.is_empty() {
        return Ok(());
    }

    for field in ["max_attempts", "backoff_ms", "backoff_max_ms", "jitter_ms"] {
        validate_optional_legacy_field::<u64>(&flat, field, "retry", "a non-negative integer")?;
    }
    if let Some(max_attempts) = flat.get("max_attempts") {
        validate_legacy_value::<u32>(max_attempts, "retry.max_attempts", "a 32-bit integer")?;
    }
    validate_optional_legacy_field::<String>(&flat, "on_status", "retry", "a string")?;
    validate_optional_legacy_field::<Vec<String>>(
        &flat,
        "on_class",
        "retry",
        "an array of strings",
    )?;
    validate_optional_legacy_field::<RetryStrategy>(
        &flat,
        "strategy",
        "retry",
        "`failover` or `same_upstream`",
    )?;

    if let Some(upstream) = retry.get("upstream") {
        upstream
            .as_table()
            .context("legacy configuration field `retry.upstream` must be a table")?;
        let mut ignored = flat.keys().cloned().collect::<Vec<_>>();
        ignored.sort();
        notices.push(format!(
            "ignored legacy flat retry settings {} because existing `retry.upstream` was the complete historical override",
            ignored.join(", ")
        ));
        return Ok(());
    }

    retry.insert("upstream".to_string(), TomlValue::Table(flat));
    notices.push("migrated legacy flat retry settings into `retry.upstream`".to_string());
    Ok(())
}

fn toml_string(value: Option<&TomlValue>) -> Option<String> {
    value
        .and_then(TomlValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn toml_string_array(value: Option<&TomlValue>) -> Vec<String> {
    value
        .and_then(TomlValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| toml_string(Some(value)))
        .collect()
}

fn toml_table_value(entries: impl IntoIterator<Item = (&'static str, TomlValue)>) -> TomlValue {
    TomlValue::Table(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

fn toml_string_array_value(values: Vec<String>) -> TomlValue {
    TomlValue::Array(values.into_iter().map(TomlValue::String).collect())
}

fn toml_table_string(value: &TomlValue, key: &str) -> Option<String> {
    value
        .as_table()
        .and_then(|table| table.get(key))
        .and_then(|value| toml_string(Some(value)))
}

fn legacy_effective_level(value: Option<&TomlValue>) -> i64 {
    value
        .and_then(TomlValue::as_integer)
        .unwrap_or(1)
        .clamp(1, 10)
}

fn route_name_available(used: &mut BTreeSet<String>, base: &str, suffix: &str) -> String {
    let base = if base.trim().is_empty() {
        suffix.to_string()
    } else {
        base.trim().to_string()
    };
    if used.insert(base.clone()) {
        return base;
    }
    let mut candidate = format!("{base}_{suffix}");
    let mut index = 2usize;
    while !used.insert(candidate.clone()) {
        candidate = format!("{base}_{suffix}_{index}");
        index += 1;
    }
    candidate
}

fn migration_route_entry(
    service: &toml::map::Map<String, TomlValue>,
    route_refs: &[String],
) -> String {
    let mut used = service
        .get("providers")
        .and_then(TomlValue::as_table)
        .into_iter()
        .flat_map(|providers| providers.keys().cloned())
        .chain(route_refs.iter().cloned())
        .collect::<BTreeSet<_>>();
    route_name_available(&mut used, "main", "route")
}

fn migrate_v3_routing_for_service(
    service_name: &str,
    service: &mut toml::map::Map<String, TomlValue>,
    notices: &mut Vec<String>,
) -> Result<()> {
    let Some(mut routing) = service.remove("routing") else {
        return Ok(());
    };
    let Some(legacy) = routing.as_table_mut() else {
        service.insert("routing".to_string(), routing);
        return Ok(());
    };
    if legacy.contains_key("routes") || legacy.contains_key("entry") {
        service.insert("routing".to_string(), routing);
        return Ok(());
    }

    let retained_extensions = legacy
        .iter()
        .filter(|(field, _)| !LEGACY_V3_ROUTING_FIELDS.contains(&field.as_str()))
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect::<toml::map::Map<_, _>>();
    let mut retained_extension_names = retained_extensions.keys().cloned().collect::<Vec<_>>();
    retained_extension_names.sort();

    let providers = service
        .get("providers")
        .and_then(TomlValue::as_table)
        .map(|providers| providers.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let policy = toml_table_string(&TomlValue::Table(legacy.clone()), "policy")
        .unwrap_or_else(|| "ordered-failover".to_string());
    let on_exhausted = toml_table_string(&TomlValue::Table(legacy.clone()), "on_exhausted")
        .unwrap_or_else(|| "continue".to_string());
    let stops_on_exhaustion = on_exhausted == "stop";
    let mut children = toml_string_array(legacy.get("order"));
    if children.is_empty() {
        children = providers.clone();
    }
    let mut root = toml::map::Map::new();
    root.insert("on_exhausted".to_string(), TomlValue::String(on_exhausted));

    match policy.as_str() {
        "ordered-failover" => {
            root.insert(
                "strategy".to_string(),
                TomlValue::String("ordered-failover".to_string()),
            );
        }
        "manual-sticky" => {
            root.insert(
                "strategy".to_string(),
                TomlValue::String("manual-sticky".to_string()),
            );
            let target = toml_string(legacy.get("target"))
                .or_else(|| children.first().cloned())
                .or_else(|| providers.first().cloned());
            if let Some(target) = target {
                root.insert("target".to_string(), TomlValue::String(target));
            }
        }
        "tag-preferred" => {
            root.insert(
                "strategy".to_string(),
                TomlValue::String("tag-preferred".to_string()),
            );
            if let Some(prefer_tags) = legacy.get("prefer_tags") {
                root.insert("prefer_tags".to_string(), prefer_tags.clone());
            }
        }
        "pool-fallback" => {
            root.insert(
                "strategy".to_string(),
                TomlValue::String("ordered-failover".to_string()),
            );
            let chain = {
                let chain = toml_string_array(legacy.get("chain"));
                if chain.is_empty() {
                    legacy
                        .get("pools")
                        .and_then(TomlValue::as_table)
                        .map(|pools| pools.keys().cloned().collect::<Vec<_>>())
                        .unwrap_or_default()
                } else {
                    chain
                }
            };
            let pools = legacy
                .get("pools")
                .and_then(TomlValue::as_table)
                .cloned()
                .unwrap_or_default();
            if chain.is_empty() {
                anyhow::bail!(
                    "[{service_name}] legacy pool-fallback routing requires at least one pool"
                );
            }
            let mut used = providers.iter().cloned().collect::<BTreeSet<_>>();
            let mut branch_names = Vec::new();
            let mut routes = toml::map::Map::new();
            for pool_name in chain {
                let Some(pool) = pools.get(&pool_name).and_then(TomlValue::as_table) else {
                    anyhow::bail!(
                        "[{service_name}] legacy pool-fallback routing references missing pool `{pool_name}`"
                    );
                };
                let pool_children = toml_string_array(pool.get("providers"));
                if pool_children.is_empty() {
                    anyhow::bail!(
                        "[{service_name}] legacy pool `{pool_name}` must contain at least one provider"
                    );
                }
                let route_name = route_name_available(&mut used, &pool_name, "pool");
                let route = toml_table_value([
                    (
                        "strategy",
                        TomlValue::String("ordered-failover".to_string()),
                    ),
                    ("children", toml_string_array_value(pool_children)),
                ]);
                routes.insert(route_name.clone(), route);
                branch_names.push(route_name);
            }
            if stops_on_exhaustion {
                branch_names.truncate(1);
            }
            if !branch_names.is_empty() {
                let entry = route_name_available(&mut used, "main", "route");
                root.insert(
                    "children".to_string(),
                    toml_string_array_value(branch_names),
                );
                let mut graph = toml::map::Map::new();
                graph.insert(entry.clone(), TomlValue::Table(root.clone()));
                graph.extend(routes);
                let mut result = retained_extensions.clone();
                result.insert("entry".to_string(), TomlValue::String(entry));
                result.insert(
                    "affinity_policy".to_string(),
                    TomlValue::String("fallback-sticky".to_string()),
                );
                result.insert("routes".to_string(), TomlValue::Table(graph));
                service.insert("routing".to_string(), TomlValue::Table(result));
                if !retained_extension_names.is_empty() {
                    notices.push(format!(
                        "[{service_name}] retained legacy v3 routing extension field(s) at the version 6 routing scope: {}",
                        retained_extension_names.join(", ")
                    ));
                }
                notices.push(format!(
                    "[{service_name}] converted v3 pool-fallback routing into route graph nodes"
                ));
                return Ok(());
            }
        }
        other => {
            anyhow::bail!(
                "[{service_name}] unsupported legacy routing policy `{other}`; migration was not written"
            );
        }
    }

    root.insert(
        "children".to_string(),
        toml_string_array_value(children.clone()),
    );
    let entry = migration_route_entry(service, &children);
    let mut routes = toml::map::Map::new();
    routes.insert(entry.clone(), TomlValue::Table(root));
    let mut result = retained_extensions;
    result.insert("entry".to_string(), TomlValue::String(entry));
    result.insert(
        "affinity_policy".to_string(),
        TomlValue::String("fallback-sticky".to_string()),
    );
    result.insert("routes".to_string(), TomlValue::Table(routes));
    service.insert("routing".to_string(), TomlValue::Table(result));
    if !retained_extension_names.is_empty() {
        notices.push(format!(
            "[{service_name}] retained legacy v3 routing extension field(s) at the version 6 routing scope: {}",
            retained_extension_names.join(", ")
        ));
    }
    notices.push(format!(
        "[{service_name}] converted legacy v3 `{policy}` routing into the version 6 route graph"
    ));
    Ok(())
}

fn migrate_v3_routing(value: &mut TomlValue, notices: &mut Vec<String>) -> Result<()> {
    for service_name in ["codex", "claude"] {
        let Some(service) = value
            .get_mut(service_name)
            .and_then(TomlValue::as_table_mut)
        else {
            continue;
        };
        migrate_v3_routing_for_service(service_name, service, notices)?;
    }
    Ok(())
}

fn ordered_v2_group_members(
    service_name: &str,
    group_name: &str,
    group: &TomlValue,
    provider_names: &BTreeSet<String>,
) -> Result<Vec<String>> {
    let Some(members) = group.get("members").and_then(TomlValue::as_array) else {
        return Ok(Vec::new());
    };
    let mut members = members
        .iter()
        .enumerate()
        .map(|(index, member)| -> Result<_> {
            let provider = member
                .get("provider")
                .and_then(TomlValue::as_str)
                .context("validated v2 group member is missing its provider")?
                .to_string();
            let preferred = member
                .get("preferred")
                .and_then(TomlValue::as_bool)
                .unwrap_or(false);
            let endpoint_names = member
                .get("endpoint_names")
                .or_else(|| member.get("endpoints"))
                .and_then(TomlValue::as_array)
                .into_iter()
                .flatten()
                .map(|endpoint| {
                    endpoint
                        .as_str()
                        .context("validated v2 group endpoint name is not a string")
                })
                .collect::<Result<Vec<_>>>()?;
            let route_refs = if endpoint_names.is_empty() {
                vec![provider]
            } else {
                if provider.is_empty() || provider.trim() != provider || provider.contains('.') {
                    anyhow::bail!(
                        "[{service_name}] v2 group `{group_name}` has ambiguous endpoint-scoped provider {provider:?}; provider ids must be non-empty, whitespace-exact, and contain no dots to round-trip through provider.endpoint references"
                    );
                }
                let mut route_refs = Vec::with_capacity(endpoint_names.len());
                for endpoint in endpoint_names {
                    if endpoint.is_empty() || endpoint.trim() != endpoint {
                        anyhow::bail!(
                            "[{service_name}] v2 group `{group_name}` has ambiguous endpoint id {endpoint:?}; endpoint ids must be non-empty and whitespace-exact to round-trip through provider.endpoint references"
                        );
                    }
                    let route_ref = format!("{provider}.{endpoint}");
                    if provider_names.contains(&route_ref) {
                        anyhow::bail!(
                            "[{service_name}] v2 group `{group_name}` has ambiguous endpoint reference `{route_ref}` because a provider has the same composite name"
                        );
                    }
                    route_refs.push(route_ref);
                }
                route_refs
            };
            Ok((!preferred, index, route_refs))
        })
        .collect::<Result<Vec<_>>>()?;
    members.sort_by_key(|(preferred, index, _)| (*preferred, *index));
    Ok(members
        .into_iter()
        .flat_map(|(_, _, route_refs)| route_refs)
        .collect())
}

fn insert_ordered_route_graph(
    service: &mut toml::map::Map<String, TomlValue>,
    children: Vec<String>,
) {
    if children.is_empty() {
        return;
    }
    let entry = migration_route_entry(service, &children);
    let root = toml_table_value([
        (
            "strategy",
            TomlValue::String("ordered-failover".to_string()),
        ),
        ("children", toml_string_array_value(children)),
    ]);
    let mut routes = toml::map::Map::new();
    routes.insert(entry.clone(), root);
    let mut routing = toml::map::Map::new();
    routing.insert("entry".to_string(), TomlValue::String(entry));
    routing.insert(
        "affinity_policy".to_string(),
        TomlValue::String("fallback-sticky".to_string()),
    );
    routing.insert("routes".to_string(), TomlValue::Table(routes));
    service.insert("routing".to_string(), TomlValue::Table(routing));
}

fn migrate_v2_service(
    service_name: &str,
    service: &mut toml::map::Map<String, TomlValue>,
    notices: &mut Vec<String>,
) -> Result<()> {
    let provider_names = service
        .get("providers")
        .and_then(TomlValue::as_table)
        .map(|providers| providers.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let Some(groups) = service
        .remove("groups")
        .or_else(|| service.remove("stations"))
    else {
        if !provider_names.is_empty() {
            anyhow::bail!(
                "[{service_name}] v2 service has providers but no routable group members; refusing to route every provider implicitly"
            );
        }
        service.remove("active_group");
        service.remove("active_station");
        return Ok(());
    };
    let Some(groups) = groups.as_table() else {
        anyhow::bail!("[{service_name}] v2 groups/stations must be a table")
    };
    let active = service
        .remove("active_group")
        .or_else(|| service.remove("active_station"))
        .and_then(|value| value.as_str().map(ToOwned::to_owned));

    let mut ordered_groups = groups
        .iter()
        .map(|(name, group)| {
            let enabled = group
                .get("enabled")
                .and_then(TomlValue::as_bool)
                .unwrap_or(true);
            let level = legacy_effective_level(group.get("level"));
            let is_active = active.as_deref() == Some(name.as_str());
            (name.clone(), enabled, level, is_active)
        })
        .collect::<Vec<_>>();
    let fallback_group = ordered_groups
        .iter()
        .filter(|(_, enabled, _, is_active)| !*enabled && !*is_active)
        .map(|(name, _, _, _)| name)
        .min()
        .cloned();
    ordered_groups.retain(|(_, enabled, _, is_active)| *enabled || *is_active);
    if ordered_groups.is_empty()
        && let Some(fallback_group) = fallback_group
        && let Some(group) = groups.get(&fallback_group)
    {
        let level = legacy_effective_level(group.get("level"));
        ordered_groups.push((fallback_group, false, level, false));
    }
    ordered_groups.sort_by(
        |(left_name, _, left_level, left_active), (right_name, _, right_level, right_active)| {
            left_level
                .cmp(right_level)
                .then_with(|| right_active.cmp(left_active))
                .then_with(|| left_name.cmp(right_name))
        },
    );

    let mut children = Vec::new();
    for (group_name, _, _, _) in ordered_groups {
        let Some(group) = groups.get(&group_name) else {
            continue;
        };
        for route_ref in
            ordered_v2_group_members(service_name, &group_name, group, &provider_names)?
        {
            if !children.iter().any(|existing| existing == &route_ref) {
                children.push(route_ref);
            }
        }
    }
    if children.is_empty() && !provider_names.is_empty() {
        anyhow::bail!(
            "[{service_name}] v2 service has no routable group members; refusing to route every provider implicitly"
        );
    }
    insert_ordered_route_graph(service, children);
    notices.push(format!(
        "[{service_name}] flattened v2 stations/groups into one route graph entry while preserving effective level/active order and explicit endpoint scoping; group aliases are not retained"
    ));
    Ok(())
}

fn legacy_provider_name(station_name: &str, index: usize, used: &mut BTreeSet<String>) -> String {
    let mut base = station_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while base.starts_with('-') {
        base.remove(0);
    }
    while base.ends_with('-') {
        base.pop();
    }
    if base.is_empty() {
        base = "station".to_string();
    }
    let candidate = format!("{base}__u{:02}", index + 1);
    if used.insert(candidate.clone()) {
        return candidate;
    }
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}__u{:02}_{suffix}", index + 1);
        if used.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn migrate_legacy_station_service(
    service_name: &str,
    service: &mut toml::map::Map<String, TomlValue>,
    notices: &mut Vec<String>,
    explicit_v1: bool,
) -> Result<bool> {
    let (uses_configs, stations) = if let Some(stations) = service.remove("configs") {
        (true, stations)
    } else if let Some(stations) = service.remove("stations") {
        (false, stations)
    } else {
        return Ok(false);
    };
    let Some(stations) = stations.as_table() else {
        anyhow::bail!("[{service_name}] legacy stations/configs must be a table")
    };
    if !explicit_v1
        && !uses_configs
        && !stations.values().any(|station| {
            station
                .as_table()
                .is_some_and(|station| station.contains_key("upstreams"))
        })
    {
        // This is the v2 stations alias, which is handled by migrate_v2_service.
        service.insert("stations".to_string(), TomlValue::Table(stations.clone()));
        return Ok(false);
    }

    let active = service
        .remove("active")
        .and_then(|value| value.as_str().map(ToOwned::to_owned));
    let mut stations_by_priority = stations
        .iter()
        .map(|(name, station)| {
            let enabled = station
                .get("enabled")
                .and_then(TomlValue::as_bool)
                .unwrap_or(true);
            let level = legacy_effective_level(station.get("level"));
            let is_active = active.as_deref() == Some(name.as_str());
            (name.clone(), enabled, level, is_active)
        })
        .collect::<Vec<_>>();
    let mut included_stations = stations_by_priority
        .iter()
        .filter(|(_, enabled, _, is_active)| *enabled || *is_active)
        .map(|(name, _, _, _)| name.clone())
        .collect::<BTreeSet<_>>();
    if included_stations.is_empty()
        && let Some(fallback) = stations.keys().min()
    {
        included_stations.insert(fallback.clone());
    }
    stations_by_priority.sort_by(
        |(left_name, _, left_level, left_active), (right_name, _, right_level, right_active)| {
            left_level
                .cmp(right_level)
                .then_with(|| right_active.cmp(left_active))
                .then_with(|| left_name.cmp(right_name))
        },
    );

    let mut providers = toml::map::Map::new();
    let mut children = Vec::new();
    let mut used = BTreeSet::new();
    for (station_name, _, _, _) in stations_by_priority {
        let Some(station) = stations.get(&station_name).and_then(TomlValue::as_table) else {
            continue;
        };
        let include = included_stations.contains(&station_name);
        let Some(upstreams) = station.get("upstreams").and_then(TomlValue::as_array) else {
            continue;
        };
        for (index, upstream) in upstreams.iter().enumerate() {
            let Some(upstream) = upstream.as_table() else {
                anyhow::bail!(
                    "[{service_name}] legacy station `{station_name}` contains a non-table upstream"
                );
            };
            let provider_name = legacy_provider_name(&station_name, index, &mut used);
            let mut provider = upstream.clone();
            provider.insert("enabled".to_string(), TomlValue::Boolean(include));
            providers.insert(provider_name.clone(), TomlValue::Table(provider));
            if include {
                children.push(provider_name);
            }
        }
    }
    service.insert("providers".to_string(), TomlValue::Table(providers));
    insert_ordered_route_graph(service, children);
    notices.push(format!(
        "[{service_name}] converted legacy station/config entries into provider keys and one route graph entry"
    ));
    Ok(true)
}

fn migrate_legacy_station_shapes(
    value: &mut TomlValue,
    notices: &mut Vec<String>,
    explicit_v1: bool,
) -> Result<()> {
    for service_name in ["codex", "claude"] {
        let Some(service) = value
            .get_mut(service_name)
            .and_then(TomlValue::as_table_mut)
        else {
            continue;
        };
        migrate_legacy_station_service(service_name, service, notices, explicit_v1)?;
    }
    Ok(())
}

fn migrate_v2_shapes(
    value: &mut TomlValue,
    notices: &mut Vec<String>,
    explicit_v2: bool,
) -> Result<()> {
    for service_name in ["codex", "claude"] {
        let Some(service) = value
            .get_mut(service_name)
            .and_then(TomlValue::as_table_mut)
        else {
            continue;
        };
        if !explicit_v2 && !service_has_v2_station_shape(service) {
            continue;
        }
        migrate_v2_service(service_name, service, notices)?;
    }
    Ok(())
}

fn migration_root_unknown_fields(value: &TomlValue, notices: &mut Vec<String>) {
    let known = [
        "version",
        "codex",
        "claude",
        "retry",
        "notify",
        "default_service",
        "relay_targets",
        "fleet",
        "ui",
    ];
    if let Some(table) = value.as_table() {
        let unknown = table
            .keys()
            .filter(|key| !known.contains(&key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            notices.push(format!(
                "unrecognized root setting(s) were retained verbatim but are not interpreted by the current runtime: {}",
                unknown
                    .iter()
                    .map(|key| format!("'{key}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
}

fn migration_service_unknown_fields(value: &TomlValue, notices: &mut Vec<String>) {
    for service_name in ["codex", "claude"] {
        let Some(service) = value.get(service_name).and_then(TomlValue::as_table) else {
            continue;
        };
        let known = ["default_profile", "profiles", "providers", "routing"];
        let unknown = service
            .keys()
            .filter(|key| !known.contains(&key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            notices.push(format!(
                "[{service_name}] unrecognized setting(s) were retained verbatim but are not interpreted by the current runtime: {}",
                unknown
                    .iter()
                    .map(|key| format!("'{key}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
}

fn redact_migration_preview(rendered: &str) -> String {
    let Ok(mut value) = toml::from_str::<TomlValue>(rendered) else {
        return "# preview omitted because the migrated TOML could not be redacted safely\n"
            .to_string();
    };
    redact_toml_secret_values(&mut value);
    let body = toml::to_string_pretty(&value).unwrap_or_else(|_| {
        "# preview omitted because the migrated TOML could not be rendered safely\n".to_string()
    });
    format!("{CONFIG_TOML_DOC_HEADER}\n{body}")
}

fn redact_toml_secret_values(value: &mut TomlValue) {
    match value {
        TomlValue::Table(table) => {
            for (key, value) in table {
                if matches!(key.as_str(), "auth_token" | "api_key") {
                    *value = TomlValue::String("<redacted>".to_string());
                } else if key == "headers" {
                    redact_all_toml_leaf_values(value);
                } else {
                    redact_toml_secret_values(value);
                }
            }
        }
        TomlValue::Array(values) => {
            for value in values {
                redact_toml_secret_values(value);
            }
        }
        _ => {}
    }
}

fn redact_all_toml_leaf_values(value: &mut TomlValue) {
    match value {
        TomlValue::Table(table) => {
            for (_, value) in table.iter_mut() {
                redact_all_toml_leaf_values(value);
            }
        }
        TomlValue::Array(values) => {
            for value in values {
                redact_all_toml_leaf_values(value);
            }
        }
        _ => *value = TomlValue::String("<redacted>".to_string()),
    }
}

async fn build_config_migration_plan(
    paths: &ResolvedConfigDirectory,
) -> Result<ConfigMigrationPlan> {
    let (format, source_name, source) =
        if let Some(source) = read_existing_config_file(paths, "config.toml").await? {
            (ConfigMigrationFormat::Toml, "config.toml", source)
        } else if let Some(source) = read_existing_config_file(paths, "config.json").await? {
            (ConfigMigrationFormat::Json, "config.json", source)
        } else {
            anyhow::bail!(
                "no config.toml or legacy config.json exists at {}; nothing to migrate",
                paths.logical_path.display()
            );
        };

    if source.entry_is_symlink {
        anyhow::bail!(
            "refusing to migrate {source_name} because it is a symbolic link; dry-run and write mode require the same regular-file source, and neither the link nor its target was modified"
        );
    }

    let mut notices = Vec::new();
    let mut raw = parse_migration_source(format, source_name, &source.contents, &mut notices)?;
    let explicit_version = toml_schema_version(&raw, source_name)?;
    let source_version = explicit_version.or_else(|| inferred_migration_schema_version(&raw));
    if source_version.is_some_and(|version| version > u64::from(CURRENT_CONFIG_VERSION)) {
        anyhow::bail!(
            "{source_name} uses newer unsupported config version {}; migration only supports up to version {}",
            source_version.unwrap_or_default(),
            CURRENT_CONFIG_VERSION
        );
    }
    reject_pre_v6_credential_references(&raw, explicit_version)?;

    let legacy_station_shape = has_legacy_station_shape(&raw);
    let v2_shape = has_v2_station_shape(&raw);
    let v3_shape = has_legacy_v3_routing_shape(&raw);
    let requires_write = matches!(format, ConfigMigrationFormat::Json)
        || toml_config_requires_migration(&raw, explicit_version);
    validate_legacy_migration_input(&raw, explicit_version)?;
    if legacy_station_shape || explicit_version == Some(1) {
        migrate_legacy_station_shapes(&mut raw, &mut notices, explicit_version == Some(1))?;
    }
    if v2_shape || explicit_version == Some(2) {
        migrate_v2_shapes(&mut raw, &mut notices, explicit_version == Some(2))?;
    }
    if v3_shape || source_version == Some(3) {
        migrate_v3_routing(&mut raw, &mut notices)?;
    }
    if explicit_version.is_none() {
        notices.push(
            "source has no explicit schema version; fields matching the current contract were imported and the result was validated as version 6".to_string(),
        );
    } else if source_version == Some(4) {
        notices.push(
            "version 4 route-graph fields were retained and the schema version was advanced to 6"
                .to_string(),
        );
    }

    migrate_flat_retry_settings(&mut raw, &mut notices)?;
    remove_retired_settings(&mut raw, &mut notices);
    migration_root_unknown_fields(&raw, &mut notices);
    migration_service_unknown_fields(&raw, &mut notices);

    let root = raw
        .as_table_mut()
        .context("legacy configuration root must be a TOML/JSON object")?;
    root.insert(
        "version".to_string(),
        TomlValue::Integer(i64::from(CURRENT_CONFIG_VERSION)),
    );
    let raw_body = toml::to_string_pretty(&raw).context("serialize migrated configuration")?;
    let candidate = toml::from_str::<HelperConfig>(&raw_body)
        .context("parse migrated configuration against the current v6 schema")?;
    validate_helper_config(&candidate).context("validate migrated configuration")?;
    // Retain every raw field that was not explicitly transformed or retired.
    // Typed parsing above validates known settings without silently erasing
    // unknown nested values during migration.
    let candidate_body = raw_body;
    let rendered = format!("{CONFIG_TOML_DOC_HEADER}\n{candidate_body}");
    let preview = redact_migration_preview(rendered.as_str());

    let backup_name = if matches!(format, ConfigMigrationFormat::Json) {
        "config.json.bak"
    } else {
        "config.toml.bak"
    };
    Ok(ConfigMigrationPlan {
        source_name,
        source_version,
        requires_write,
        source,
        target_path: paths.logical_file("config.toml"),
        backup_path: paths.logical_file(backup_name),
        rendered,
        preview,
        notices,
    })
}

async fn apply_config_migration_plan(
    paths: &ResolvedConfigDirectory,
    plan: &ConfigMigrationPlan,
) -> Result<()> {
    apply_config_migration_plan_with_race_hooks(paths, plan, || Ok(()), || Ok(())).await
}

async fn apply_config_migration_plan_with_race_hooks<B, A>(
    paths: &ResolvedConfigDirectory,
    plan: &ConfigMigrationPlan,
    before_backup_publication_check: B,
    after_backup_publication_check: A,
) -> Result<()>
where
    B: FnOnce() -> io::Result<()> + Send + 'static,
    A: FnOnce() -> io::Result<()> + Send + 'static,
{
    paths.ensure_unchanged().await?;
    if !plan.requires_write {
        return Ok(());
    }
    let current = read_existing_config_file(paths, plan.source_name)
        .await?
        .with_context(|| format!("{} disappeared during migration", plan.source_name))?;
    if current.entry_is_symlink {
        anyhow::bail!(
            "refusing to migrate {} because it is a symbolic link; the source and target were not modified",
            plan.source_name
        );
    }
    if current.contents != plan.source.contents {
        anyhow::bail!(
            "{} changed while migration was being prepared; the source and target were not modified",
            plan.source_name
        );
    }
    if !current.metadata.matches(&plan.source.metadata) {
        anyhow::bail!(
            "{} ownership or permissions changed while migration was being prepared; no backup or migrated target was written",
            plan.source_name
        );
    }
    if plan.source_name == "config.json"
        && read_existing_config_file(paths, "config.toml")
            .await?
            .is_some()
    {
        anyhow::bail!("config.toml appeared while migrating config.json; no files were modified");
    }

    let backup_name = plan
        .backup_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml.bak");
    let previous_backup = read_existing_config_file(paths, backup_name).await?;
    if previous_backup
        .as_ref()
        .is_some_and(|backup| backup.entry_is_symlink)
    {
        anyhow::bail!(
            "refusing to migrate because {} is a symbolic link and cannot be restored safely if target publication is rejected",
            plan.backup_path.display()
        );
    }

    let verified_metadata = current.metadata.clone();
    let backup_destination = paths.resolved_file(backup_name);
    let backup_source_path = paths.resolved_file(plan.source_name);
    let backup_source_name = plan.source_name;
    let backup_expected_contents = plan.source.contents.clone();
    let backup_expected_metadata = verified_metadata.clone();
    let staged_backup_metadata = verified_metadata.clone();
    let expected_previous_backup = previous_backup.clone();
    let backup_destination_for_check = backup_destination.clone();
    let target_destination = paths.resolved_file("config.toml");
    let logical_directory = paths.logical_path.clone();
    let resolved_directory = paths.resolved_path.clone();
    // Finalize every source and target precondition at the backup publication boundary. A
    // rejected precondition must not replace an existing backup.
    write_bytes_file_async_with_permissions_and_before_replace(
        &backup_destination,
        &plan.source.contents,
        verified_metadata.permissions.clone(),
        move |staged_path, _destination| {
            staged_backup_metadata.apply_to_staged_file(staged_path)?;
            before_backup_publication_check()?;
            verify_config_directory_binding(&logical_directory, &resolved_directory)?;
            verify_migration_source_before_replace(
                &backup_source_path,
                &target_destination,
                backup_source_name,
                &backup_expected_contents,
                &backup_expected_metadata,
            )?;
            verify_migration_backup_snapshot(
                &backup_destination_for_check,
                expected_previous_backup.as_ref(),
            )
        },
    )
    .await
    .with_context(|| {
        format!(
            "back up {} to {}",
            plan.source_name,
            plan.backup_path.display()
        )
    })?;

    let destination = paths.resolved_file("config.toml");
    let target_source_path = paths.resolved_file(plan.source_name);
    let target_source_name = plan.source_name;
    let target_expected_contents = plan.source.contents.clone();
    let target_expected_metadata = verified_metadata.clone();
    let staged_target_metadata = verified_metadata.clone();
    let target_logical_directory = paths.logical_path.clone();
    let target_resolved_directory = paths.resolved_path.clone();
    // The successful path remains backup-first. A rejected target precondition restores the
    // previous backup, but a crash between backup commit and target commit or rollback can leave
    // the new backup beside the old target; these replacements are not a cross-file transaction.
    let target_write = write_bytes_file_async_with_permissions_and_before_replace(
        &destination,
        plan.rendered.as_bytes(),
        staged_target_metadata.permissions.clone(),
        move |staged_path, destination| {
            staged_target_metadata.apply_to_staged_file(staged_path)?;
            after_backup_publication_check()?;
            verify_config_directory_binding(&target_logical_directory, &target_resolved_directory)?;
            verify_migration_source_before_replace(
                &target_source_path,
                destination,
                target_source_name,
                &target_expected_contents,
                &target_expected_metadata,
            )
        },
    )
    .await;

    match target_write {
        Ok(()) => Ok(()),
        Err(error @ AtomicWriteError::BeforeCommit { .. }) => {
            if let Err(rollback_error) = rollback_migration_backup(
                paths,
                &backup_destination,
                previous_backup.as_ref(),
                &plan.source.contents,
                &verified_metadata,
            )
            .await
            {
                anyhow::bail!(
                    "write migrated configuration to {} failed before commit: {error}; restoring the prior backup also failed: {rollback_error:#}",
                    plan.target_path.display()
                );
            }
            Err(error).with_context(|| {
                format!(
                    "write migrated configuration to {}",
                    plan.target_path.display()
                )
            })
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "write migrated configuration to {}",
                plan.target_path.display()
            )
        }),
    }
}

fn verify_config_directory_binding(logical_path: &Path, resolved_path: &Path) -> io::Result<()> {
    let current = std::fs::canonicalize(logical_path)?;
    if current != resolved_path {
        return Err(io::Error::other(format!(
            "config directory {} changed target during migration; expected {}, found {}",
            logical_path.display(),
            resolved_path.display(),
            current.display()
        )));
    }
    Ok(())
}

fn verify_migration_backup_snapshot(
    backup_path: &Path,
    expected: Option<&ExistingConfigToml>,
) -> io::Result<()> {
    let Some(expected) = expected else {
        return match std::fs::symlink_metadata(backup_path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(io::Error::other(
                "migration backup appeared while migration was being prepared",
            )),
            Err(error) => Err(error),
        };
    };

    verify_migration_source_snapshot(
        backup_path,
        "migration backup",
        &expected.contents,
        &expected.metadata,
    )
}

async fn rollback_migration_backup(
    paths: &ResolvedConfigDirectory,
    backup_path: &Path,
    previous: Option<&ExistingConfigToml>,
    published_contents: &[u8],
    published_metadata: &ConfigFileMetadata,
) -> Result<()> {
    let logical_directory = paths.logical_path.clone();
    let resolved_directory = paths.resolved_path.clone();
    let backup_path = backup_path.to_path_buf();
    let expected_contents = published_contents.to_vec();
    let expected_metadata = published_metadata.clone();

    if let Some(previous) = previous {
        let staged_metadata = previous.metadata.clone();
        return write_bytes_file_async_with_permissions_and_before_replace(
            &backup_path,
            &previous.contents,
            previous.metadata.permissions.clone(),
            move |staged_path, destination| {
                staged_metadata.apply_to_staged_file(staged_path)?;
                verify_config_directory_binding(&logical_directory, &resolved_directory)?;
                verify_migration_source_snapshot(
                    destination,
                    "published migration backup",
                    &expected_contents,
                    &expected_metadata,
                )
            },
        )
        .await
        .context("restore the previous migration backup");
    }

    // There is no portable conditional unlink. Verify that the entry is still our published
    // backup immediately before this best-effort removal, without claiming race-free rollback.
    tokio::task::spawn_blocking(move || {
        verify_config_directory_binding(&logical_directory, &resolved_directory)?;
        verify_migration_source_snapshot(
            &backup_path,
            "published migration backup",
            &expected_contents,
            &expected_metadata,
        )?;
        std::fs::remove_file(&backup_path)
    })
    .await
    .context("join migration backup rollback")?
    .context("remove the newly published migration backup")
}

fn verify_migration_source_snapshot(
    source_path: &Path,
    source_name: &str,
    expected_contents: &[u8],
    expected_metadata: &ConfigFileMetadata,
) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(source_path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(io::Error::other(format!(
            "{source_name} is no longer the regular source file used to prepare migration"
        )));
    }
    let current_metadata = ConfigFileMetadata::capture(source_path, &metadata)?;
    if !current_metadata.matches(expected_metadata) {
        return Err(io::Error::other(format!(
            "{source_name} ownership or permissions changed while migration was being prepared"
        )));
    }
    if std::fs::read(source_path)? != expected_contents {
        return Err(io::Error::other(format!(
            "{source_name} changed while migration was being prepared"
        )));
    }
    Ok(())
}

fn verify_migration_source_before_replace(
    source_path: &Path,
    destination: &Path,
    source_name: &str,
    expected_contents: &[u8],
    expected_metadata: &ConfigFileMetadata,
) -> io::Result<()> {
    verify_migration_source_snapshot(
        source_path,
        source_name,
        expected_contents,
        expected_metadata,
    )?;
    if source_name == "config.json" {
        match std::fs::symlink_metadata(destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(io::Error::other(
                    "config.toml appeared while migrating config.json",
                ));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn config_permissions_match(
    current: &std::fs::Permissions,
    expected: &std::fs::Permissions,
) -> bool {
    use std::os::unix::fs::PermissionsExt;
    current.mode() == expected.mode()
}

#[cfg(not(unix))]
fn config_permissions_match(
    current: &std::fs::Permissions,
    expected: &std::fs::Permissions,
) -> bool {
    current.readonly() == expected.readonly()
}

fn unsupported_legacy_json_error(path: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "{} is a legacy config source and cannot be overwritten by a typed save. Start codex-helper once to migrate it automatically, or run `codex-helper config migrate --write --yes`; migration targets version = {} and keeps config.json unchanged after creating config.json.bak.",
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
    fn try_acquire(paths: &ResolvedConfigDirectory) -> Result<Self> {
        let path = paths.resolved_file("config.toml.lock");
        let file = open_config_mutation_lock_file(&path)?;
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

    async fn acquire_waiting(paths: &ResolvedConfigDirectory) -> Result<Self> {
        let path = paths.resolved_file("config.toml.lock");
        tokio::task::spawn_blocking(move || {
            let file = open_config_mutation_lock_file(&path)?;
            file.lock()
                .with_context(|| format!("wait for config mutation lock {}", path.display()))?;
            Ok(Self { _file: file })
        })
        .await
        .context("join config mutation lock wait task")?
    }
}

fn open_config_mutation_lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("open config mutation lock {}", path.display()))
}

#[derive(Debug, Clone)]
struct ExistingConfigToml {
    entry_is_symlink: bool,
    metadata: ConfigFileMetadata,
    contents: Vec<u8>,
}

impl ExistingConfigToml {
    fn text(&self) -> Result<&str> {
        std::str::from_utf8(&self.contents).context("config.toml is not valid UTF-8")
    }
}

#[derive(Debug, Clone)]
struct ConfigFileMetadata {
    permissions: std::fs::Permissions,
    platform: PlatformConfigFileMetadata,
}

impl ConfigFileMetadata {
    fn capture(path: &Path, metadata: &std::fs::Metadata) -> io::Result<Self> {
        Ok(Self {
            permissions: metadata.permissions(),
            platform: PlatformConfigFileMetadata::capture(path, metadata)?,
        })
    }

    fn matches(&self, other: &Self) -> bool {
        config_permissions_match(&self.permissions, &other.permissions)
            && self.platform == other.platform
    }

    fn apply_to_staged_file(&self, path: &Path) -> io::Result<()> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        self.platform.apply(path, &file)?;
        file.set_permissions(self.permissions.clone())?;
        file.sync_all()?;
        let metadata = std::fs::symlink_metadata(path)?;
        let applied = Self::capture(path, &metadata)?;
        if !applied.matches(self) {
            return Err(io::Error::other(
                "staged config ownership or permissions do not match the source snapshot",
            ));
        }
        Ok(())
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlatformConfigFileMetadata {
    uid: u32,
    gid: u32,
    acl: Option<Vec<u8>>,
}

#[cfg(unix)]
impl PlatformConfigFileMetadata {
    fn capture(path: &Path, metadata: &std::fs::Metadata) -> io::Result<Self> {
        use std::os::unix::fs::MetadataExt;

        Ok(Self {
            uid: metadata.uid(),
            gid: metadata.gid(),
            acl: capture_config_file_acl(path)?,
        })
    }

    fn apply(&self, _path: &Path, file: &File) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;

        let metadata = file.metadata()?;
        if metadata.uid() != self.uid || metadata.gid() != self.gid {
            std::os::unix::fs::fchown(file, Some(self.uid), Some(self.gid))?;
        }
        apply_config_file_acl(file, self.acl.as_deref())
    }
}

#[cfg(target_os = "linux")]
fn capture_config_file_acl(path: &Path) -> io::Result<Option<Vec<u8>>> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "config path contains an embedded null",
        )
    })?;
    let name = c"system.posix_acl_access";
    for _ in 0..3 {
        // SAFETY: Both C strings are NUL-terminated and a null value pointer with size zero is a
        // supported size query.
        let size = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
        if size < 0 {
            let error = io::Error::last_os_error();
            if linux_acl_is_absent(&error) {
                return Ok(None);
            }
            return Err(error);
        }
        let size = usize::try_from(size)
            .map_err(|_| io::Error::other("config ACL size does not fit in memory"))?;
        if size > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "config ACL exceeds the supported 64 KiB metadata bound",
            ));
        }
        let mut acl = vec![0_u8; size];
        // SAFETY: The buffer is writable for `size` bytes and both C strings remain live.
        let read = unsafe {
            libc::getxattr(
                path.as_ptr(),
                name.as_ptr(),
                acl.as_mut_ptr().cast(),
                acl.len(),
            )
        };
        if read >= 0 {
            acl.truncate(usize::try_from(read).unwrap_or(acl.len()));
            return Ok(Some(acl));
        }
        let error = io::Error::last_os_error();
        if linux_acl_is_absent(&error) {
            return Ok(None);
        }
        if error.raw_os_error() != Some(libc::ERANGE) {
            return Err(error);
        }
    }
    Err(io::Error::other(
        "config ACL changed repeatedly while it was being captured",
    ))
}

#[cfg(target_os = "linux")]
fn apply_config_file_acl(file: &File, acl: Option<&[u8]>) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let name = c"system.posix_acl_access";
    let result = match acl {
        Some(acl) => {
            // SAFETY: The file descriptor, name, and ACL byte slice remain valid for the call.
            unsafe {
                libc::fsetxattr(
                    file.as_raw_fd(),
                    name.as_ptr(),
                    acl.as_ptr().cast(),
                    acl.len(),
                    0,
                )
            }
        }
        None => {
            // A staging file may inherit an ACL from its parent even when the source has none.
            // SAFETY: The file descriptor and NUL-terminated attribute name are valid.
            unsafe { libc::fremovexattr(file.as_raw_fd(), name.as_ptr()) }
        }
    };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if acl.is_none() && linux_acl_is_absent(&error) {
        return Ok(());
    }
    Err(error)
}

#[cfg(target_os = "linux")]
fn linux_acl_is_absent(error: &io::Error) -> bool {
    error
        .raw_os_error()
        .is_some_and(|code| code == libc::ENODATA || code == libc::ENOTSUP)
}

#[cfg(target_os = "macos")]
const MACOS_ACL_TYPE_EXTENDED: std::ffi::c_int = 0x0000_0100;

#[cfg(target_os = "macos")]
const MACOS_ACL_FIRST_ENTRY: std::ffi::c_int = 0;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn acl_get_file(
        path: *const std::ffi::c_char,
        acl_type: std::ffi::c_int,
    ) -> *mut std::ffi::c_void;
    fn acl_get_entry(
        acl: *mut std::ffi::c_void,
        entry_id: std::ffi::c_int,
        entry: *mut *mut std::ffi::c_void,
    ) -> std::ffi::c_int;
    fn acl_init(count: std::ffi::c_int) -> *mut std::ffi::c_void;
    fn acl_copy_ext(
        buffer: *mut std::ffi::c_void,
        acl: *mut std::ffi::c_void,
        size: isize,
    ) -> isize;
    fn acl_copy_int(buffer: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn acl_size(acl: *mut std::ffi::c_void) -> isize;
    fn acl_set_fd_np(
        file_descriptor: std::ffi::c_int,
        acl: *mut std::ffi::c_void,
        acl_type: std::ffi::c_int,
    ) -> std::ffi::c_int;
    fn acl_free(value: *mut std::ffi::c_void) -> std::ffi::c_int;
}

#[cfg(target_os = "macos")]
struct OwnedMacosAcl(*mut std::ffi::c_void);

#[cfg(target_os = "macos")]
impl Drop for OwnedMacosAcl {
    fn drop(&mut self) {
        // SAFETY: The pointer was returned by a macOS ACL allocation API and remains owned here.
        unsafe {
            acl_free(self.0);
        }
    }
}

#[cfg(target_os = "macos")]
fn capture_config_file_acl(path: &Path) -> io::Result<Option<Vec<u8>>> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "config path contains an embedded null",
        )
    })?;
    // SAFETY: The path is NUL-terminated and the ACL type is supported by macOS.
    let acl = unsafe { acl_get_file(path.as_ptr(), MACOS_ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        if error
            .raw_os_error()
            .is_some_and(|code| code == libc::ENOENT || code == libc::ENOTSUP)
        {
            return Ok(None);
        }
        return Err(error);
    }
    let acl = OwnedMacosAcl(acl);
    let mut entry = std::ptr::null_mut();
    // SAFETY: The ACL guard owns a valid ACL and the output pointer is valid for the call.
    if unsafe { acl_get_entry(acl.0, MACOS_ACL_FIRST_ENTRY, &mut entry) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINVAL) {
            return Ok(None);
        }
        return Err(error);
    }
    // SAFETY: The ACL guard owns a valid ACL.
    let size = unsafe { acl_size(acl.0) };
    if size < 0 {
        return Err(io::Error::last_os_error());
    }
    let size = usize::try_from(size)
        .map_err(|_| io::Error::other("config ACL size does not fit in memory"))?;
    if size > 64 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "config ACL exceeds the supported 64 KiB metadata bound",
        ));
    }
    let mut external = vec![0_u8; size];
    // SAFETY: The output buffer is writable for `size` bytes and the ACL guard remains live.
    let copied = unsafe { acl_copy_ext(external.as_mut_ptr().cast(), acl.0, size as isize) };
    if copied < 0 {
        return Err(io::Error::last_os_error());
    }
    external.truncate(usize::try_from(copied).unwrap_or(external.len()));
    Ok(Some(external))
}

#[cfg(target_os = "macos")]
fn apply_config_file_acl(file: &File, acl: Option<&[u8]>) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    let acl = match acl {
        Some(external) => {
            // SAFETY: The bytes were produced by acl_copy_ext and remain live for this call.
            unsafe { acl_copy_int(external.as_ptr().cast()) }
        }
        None => {
            // SAFETY: Zero creates a valid empty ACL used to clear inherited entries.
            unsafe { acl_init(0) }
        }
    };
    if acl.is_null() {
        return Err(io::Error::last_os_error());
    }
    let acl = OwnedMacosAcl(acl);
    // SAFETY: The descriptor and owned ACL remain valid for the duration of the call.
    if unsafe { acl_set_fd_np(file.as_raw_fd(), acl.0, MACOS_ACL_TYPE_EXTENDED) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn capture_config_file_acl(_path: &Path) -> io::Result<Option<Vec<u8>>> {
    Ok(None)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn apply_config_file_acl(_file: &File, _acl: Option<&[u8]>) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
#[derive(Clone)]
struct PlatformConfigFileMetadata {
    descriptor: Vec<usize>,
    descriptor_len: usize,
    owner_sid: Option<Vec<u8>>,
    group_sid: Option<Vec<u8>>,
    dacl: WindowsDaclSnapshot,
    dacl_protected: bool,
}

#[cfg(windows)]
#[derive(Clone, PartialEq, Eq)]
enum WindowsDaclSnapshot {
    NotPresent,
    Null,
    Present(Vec<u8>),
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum WindowsSidKind {
    Owner,
    Group,
}

#[cfg(windows)]
impl PartialEq for PlatformConfigFileMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.owner_sid == other.owner_sid
            && self.group_sid == other.group_sid
            && self.dacl == other.dacl
            && self.dacl_protected == other.dacl_protected
    }
}

#[cfg(windows)]
impl Eq for PlatformConfigFileMetadata {}

#[cfg(windows)]
impl std::fmt::Debug for PlatformConfigFileMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlatformConfigFileMetadata")
            .field("descriptor_len", &self.descriptor_len)
            .field("owner_present", &self.owner_sid.is_some())
            .field("group_present", &self.group_sid.is_some())
            .field("dacl", &self.dacl.kind_name())
            .field("dacl_protected", &self.dacl_protected)
            .finish_non_exhaustive()
    }
}

#[cfg(windows)]
impl WindowsDaclSnapshot {
    fn kind_name(&self) -> &'static str {
        match self {
            Self::NotPresent => "not_present",
            Self::Null => "null",
            Self::Present(_) => "present",
        }
    }
}

#[cfg(windows)]
impl PlatformConfigFileMetadata {
    fn capture(path: &Path, _metadata: &std::fs::Metadata) -> io::Result<Self> {
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetFileSecurityW,
            GetSecurityDescriptorControl, GetSecurityDescriptorDacl, OWNER_SECURITY_INFORMATION,
            SE_DACL_PROTECTED,
        };

        let path = windows_wide_path(path)?;
        let requested =
            OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        let mut required_bytes = 0_u32;
        // SAFETY: The path is NUL-terminated and the size-query output pointer is valid.
        unsafe {
            GetFileSecurityW(
                path.as_ptr(),
                requested,
                std::ptr::null_mut(),
                0,
                &mut required_bytes,
            );
        }
        if required_bytes == 0 {
            return Err(io::Error::last_os_error());
        }

        let word_size = std::mem::size_of::<usize>();
        let descriptor_words = usize::try_from(required_bytes)
            .unwrap_or(usize::MAX)
            .saturating_add(word_size.saturating_sub(1))
            / word_size;
        let mut descriptor = vec![0_usize; descriptor_words];
        let descriptor_capacity = descriptor
            .len()
            .checked_mul(word_size)
            .and_then(|bytes| u32::try_from(bytes).ok())
            .ok_or_else(|| io::Error::other("Windows security descriptor is too large"))?;
        // SAFETY: The aligned descriptor buffer has the advertised capacity and all pointers live
        // for the duration of the call.
        if unsafe {
            GetFileSecurityW(
                path.as_ptr(),
                requested,
                descriptor.as_mut_ptr().cast(),
                descriptor_capacity,
                &mut required_bytes,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }

        let mut control = 0_u16;
        let mut revision = 0_u32;
        // SAFETY: GetFileSecurityW initialized a self-relative security descriptor in the buffer.
        if unsafe {
            GetSecurityDescriptorControl(
                descriptor.as_mut_ptr().cast(),
                &mut control,
                &mut revision,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }

        let descriptor_pointer = descriptor.as_mut_ptr().cast();
        let owner_sid = windows_security_descriptor_sid(
            &descriptor,
            usize::try_from(required_bytes).unwrap_or(usize::MAX),
            descriptor_pointer,
            WindowsSidKind::Owner,
        )?;
        let group_sid = windows_security_descriptor_sid(
            &descriptor,
            usize::try_from(required_bytes).unwrap_or(usize::MAX),
            descriptor_pointer,
            WindowsSidKind::Group,
        )?;
        let mut dacl_present = 0;
        let mut dacl_defaulted = 0;
        let mut dacl = std::ptr::null_mut();
        // SAFETY: GetFileSecurityW initialized the descriptor and all output pointers are valid.
        if unsafe {
            GetSecurityDescriptorDacl(
                descriptor_pointer,
                &mut dacl_present,
                &mut dacl,
                &mut dacl_defaulted,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let dacl = if dacl_present == 0 {
            WindowsDaclSnapshot::NotPresent
        } else if dacl.is_null() {
            WindowsDaclSnapshot::Null
        } else {
            // SAFETY: GetSecurityDescriptorDacl returned a valid ACL pointer in the descriptor.
            let dacl_len = usize::from(unsafe { (*dacl).AclSize });
            WindowsDaclSnapshot::Present(windows_descriptor_region(
                &descriptor,
                usize::try_from(required_bytes).unwrap_or(usize::MAX),
                dacl.cast(),
                dacl_len,
            )?)
        };

        Ok(Self {
            descriptor,
            descriptor_len: usize::try_from(required_bytes).unwrap_or(usize::MAX),
            owner_sid,
            group_sid,
            dacl,
            dacl_protected: control & SE_DACL_PROTECTED != 0,
        })
    }

    fn apply(&self, path: &Path, file: &File) -> io::Result<()> {
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
            UNPROTECTED_DACL_SECURITY_INFORMATION,
        };

        if self.descriptor_len == 0
            || self.descriptor_len > self.descriptor.len() * std::mem::size_of::<usize>()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid Windows security descriptor snapshot",
            ));
        }
        let staged = Self::capture(path, &file.metadata()?)?;
        let path = windows_wide_path(path)?;
        let mut requested = 0;
        if self.owner_sid != staged.owner_sid {
            requested |= OWNER_SECURITY_INFORMATION;
        }
        if self.group_sid != staged.group_sid {
            requested |= GROUP_SECURITY_INFORMATION;
        }
        if self.dacl != staged.dacl || self.dacl_protected != staged.dacl_protected {
            requested |= DACL_SECURITY_INFORMATION;
            requested |= if self.dacl_protected {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
        }
        if requested == 0 {
            return Ok(());
        }
        // SAFETY: The path is NUL-terminated and the aligned descriptor buffer remains live.
        if unsafe {
            SetFileSecurityW(
                path.as_ptr(),
                requested,
                self.descriptor.as_ptr().cast_mut().cast(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
fn windows_security_descriptor_sid(
    descriptor: &[usize],
    descriptor_len: usize,
    descriptor_pointer: *mut std::ffi::c_void,
    kind: WindowsSidKind,
) -> io::Result<Option<Vec<u8>>> {
    use windows_sys::Win32::Security::{
        GetLengthSid, GetSecurityDescriptorGroup, GetSecurityDescriptorOwner,
    };

    let mut sid = std::ptr::null_mut();
    let mut defaulted = 0;
    // SAFETY: The descriptor and output pointers are valid for the selected accessor call.
    let succeeded = unsafe {
        match kind {
            WindowsSidKind::Owner => {
                GetSecurityDescriptorOwner(descriptor_pointer, &mut sid, &mut defaulted)
            }
            WindowsSidKind::Group => {
                GetSecurityDescriptorGroup(descriptor_pointer, &mut sid, &mut defaulted)
            }
        }
    };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    if sid.is_null() {
        return Ok(None);
    }
    // SAFETY: The accessor returned a valid SID pointer in the descriptor.
    let sid_len = usize::try_from(unsafe { GetLengthSid(sid) }).unwrap_or(usize::MAX);
    Ok(Some(windows_descriptor_region(
        descriptor,
        descriptor_len,
        sid.cast(),
        sid_len,
    )?))
}

#[cfg(windows)]
fn windows_descriptor_region(
    descriptor: &[usize],
    descriptor_len: usize,
    region: *const u8,
    region_len: usize,
) -> io::Result<Vec<u8>> {
    let descriptor_start = descriptor.as_ptr() as usize;
    let descriptor_end = descriptor_start
        .checked_add(descriptor_len)
        .ok_or_else(|| io::Error::other("Windows security descriptor range overflow"))?;
    let region_start = region as usize;
    let region_end = region_start
        .checked_add(region_len)
        .ok_or_else(|| io::Error::other("Windows security descriptor component overflow"))?;
    if region_start < descriptor_start || region_end > descriptor_end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows security descriptor component is out of bounds",
        ));
    }
    // SAFETY: The bounds check proves the component lies within the initialized descriptor.
    Ok(unsafe { std::slice::from_raw_parts(region, region_len) }.to_vec())
}

#[cfg(windows)]
fn windows_wide_path(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    if wide[..wide.len().saturating_sub(1)].contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains an embedded null",
        ));
    }
    Ok(wide)
}

#[cfg(not(any(unix, windows)))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlatformConfigFileMetadata;

#[cfg(not(any(unix, windows)))]
impl PlatformConfigFileMetadata {
    fn capture(_path: &Path, _metadata: &std::fs::Metadata) -> io::Result<Self> {
        Ok(Self)
    }

    fn apply(&self, _path: &Path, _file: &File) -> io::Result<()> {
        Ok(())
    }
}

async fn read_existing_config_file(
    paths: &ResolvedConfigDirectory,
    name: &str,
) -> Result<Option<ExistingConfigToml>> {
    let logical_path = paths.logical_file(name);
    let entry_path = paths.resolved_file(name);
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
        metadata: ConfigFileMetadata::capture(&source_path, &source_metadata)
            .with_context(|| format!("capture config metadata {}", logical_path.display()))?,
        contents,
    }))
}

async fn read_existing_config_toml(
    paths: &ResolvedConfigDirectory,
) -> Result<Option<ExistingConfigToml>> {
    read_existing_config_file(paths, "config.toml").await
}

fn parse_toml_value_with_location(text: &str, source_label: &str) -> Result<TomlValue> {
    toml::from_str::<TomlValue>(text).map_err(|source| {
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
        anyhow::anyhow!("parse {source_label} at {location}: {}", source.message())
    })
}

fn validate_current_config_toml(text: &str) -> Result<TomlValue> {
    let raw_config = parse_toml_value_with_location(text, "current config.toml")?;
    let version = toml_schema_version(&raw_config, "config.toml")?;
    if version != Some(u64::from(CURRENT_CONFIG_VERSION)) {
        return Err(unsupported_config_error("config.toml", version));
    }

    reject_retired_v5_settings(&raw_config)?;
    if has_legacy_station_shape(&raw_config)
        || has_v2_station_shape(&raw_config)
        || has_legacy_v3_routing_shape(&raw_config)
    {
        anyhow::bail!(
            "config.toml still contains a recognizable legacy configuration shape; load the configuration or run `codex-helper config migrate --write --yes` before saving current settings"
        );
    }
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
    preserve_legacy_migration_backup: bool,
) -> Result<()> {
    paths.ensure_unchanged().await?;
    if preserve_legacy_migration_backup
        && existing_backup_is_legacy_migration_source(paths)
            .await
            .with_context(|| {
                format!(
                    "back up config.toml to {}",
                    paths.logical_file("config.toml.bak").display()
                )
            })?
    {
        return Ok(());
    }
    let backup_path = paths.resolved_file("config.toml.bak");
    let source_path = paths.resolved_file("config.toml");
    let expected_contents = existing.contents.clone();
    let expected_metadata = existing.metadata.clone();
    let staged_metadata = existing.metadata.clone();
    write_bytes_file_async_with_permissions_and_before_replace(
        &backup_path,
        &existing.contents,
        existing.metadata.permissions.clone(),
        move |staged_path, _destination| {
            staged_metadata.apply_to_staged_file(staged_path)?;
            verify_migration_source_snapshot(
                &source_path,
                "config.toml",
                &expected_contents,
                &expected_metadata,
            )
        },
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

async fn existing_backup_is_legacy_migration_source(
    paths: &ResolvedConfigDirectory,
) -> Result<bool> {
    let Some(backup) = read_existing_config_file(paths, "config.toml.bak").await? else {
        return Ok(false);
    };
    let Ok(text) = backup.text() else {
        return Ok(false);
    };
    let Ok(raw) = toml::from_str::<TomlValue>(text) else {
        return Ok(false);
    };
    let Ok(version) = toml_schema_version(&raw, "config.toml.bak") else {
        return Ok(false);
    };
    Ok(toml_config_requires_migration(&raw, version))
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
    Ok(init_config_toml_with_outcome(force).await?.path)
}

pub async fn init_config_toml_with_outcome(force: bool) -> Result<ConfigInitOutcome> {
    let paths = ResolvedConfigDirectory::prepare().await?;
    init_config_toml_at_paths(&paths, force).await
}

async fn init_config_toml_at_paths(
    paths: &ResolvedConfigDirectory,
    force: bool,
) -> Result<ConfigInitOutcome> {
    let _lock = ConfigMutationLock::try_acquire(paths)?;
    paths.ensure_unchanged().await?;
    let path = paths.logical_file("config.toml");

    let existing = read_existing_config_toml(paths).await?;
    if existing.is_some() && !force {
        anyhow::bail!(
            "config.toml already exists at {:?}; use --force to overwrite",
            path
        );
    }

    if existing.is_none()
        && read_existing_config_file(paths, "config.json")
            .await?
            .is_some()
    {
        let plan = build_config_migration_plan(paths).await?;
        apply_config_migration_plan(paths, &plan).await?;
        return Ok(ConfigInitOutcome {
            path,
            migration_report: Some(plan.report(true)),
        });
    }

    if let Some(existing) = existing.as_ref() {
        reject_symlink_config_mutation(existing, "initialize")?;
        write_config_backup(paths, existing, false).await?;
    }

    paths.ensure_unchanged().await?;
    write_bytes_file_async(
        &paths.resolved_file("config.toml"),
        CONFIG_TOML_TEMPLATE.as_bytes(),
    )
    .await?;
    paths.ensure_unchanged().await?;
    Ok(ConfigInitOutcome {
        path,
        migration_report: None,
    })
}

pub async fn load_config() -> Result<HelperConfig> {
    Ok(load_config_with_source().await?.source)
}

async fn auto_migrate_legacy_config(paths: &ResolvedConfigDirectory) -> Result<()> {
    let _lock = ConfigMutationLock::acquire_waiting(paths).await?;
    paths.ensure_unchanged().await?;
    if !automatic_config_migration_required(paths).await? {
        return Ok(());
    }
    let plan = build_config_migration_plan(paths).await?;

    apply_config_migration_plan(paths, &plan).await?;
    if plan.notices.is_empty() {
        tracing::info!(
            source = plan.source_name,
            source_version = ?plan.source_version,
            "automatically migrated helper configuration to the current TOML contract"
        );
    } else {
        tracing::warn!(
            source = plan.source_name,
            source_version = ?plan.source_version,
            notices = ?plan.notices,
            "automatically migrated helper configuration with operator-visible changes"
        );
    }
    Ok(())
}

async fn automatic_config_migration_required(paths: &ResolvedConfigDirectory) -> Result<bool> {
    if let Some(existing) = read_existing_config_toml(paths).await? {
        let text = existing.text()?;
        let raw = parse_toml_value_with_location(
            text,
            &format!(
                "current config.toml at {}",
                paths.logical_file("config.toml").display()
            ),
        )?;
        let version = toml_schema_version(&raw, "config.toml")?;
        if version.is_some_and(|version| version > u64::from(CURRENT_CONFIG_VERSION)) {
            return Err(unsupported_config_error("config.toml", version));
        }
        return Ok(toml_config_requires_migration(&raw, version));
    }

    Ok(read_existing_config_file(paths, "config.json")
        .await?
        .is_some())
}

async fn load_current_config_from_paths(paths: &ResolvedConfigDirectory) -> Result<LoadedConfig> {
    let existing = read_existing_config_toml(paths)
        .await?
        .context("canonical config.toml disappeared after migration")?;
    let text = existing.text()?;
    validate_current_config_toml(text)?;
    let config_source = toml::from_str::<HelperConfig>(text)?;
    validate_helper_config(&config_source)?;
    paths.ensure_unchanged().await?;
    Ok(LoadedConfig {
        source: config_source,
    })
}

pub async fn load_config_with_source() -> Result<LoadedConfig> {
    let Some(paths) = ResolvedConfigDirectory::inspect().await? else {
        let source = HelperConfig::default();
        validate_helper_config(&source)?;
        return Ok(LoadedConfig { source });
    };

    if let Some(existing) = read_existing_config_toml(&paths).await? {
        let text = existing.text()?;
        let raw = parse_toml_value_with_location(
            text,
            &format!(
                "current config.toml at {}",
                paths.logical_file("config.toml").display()
            ),
        )?;
        let version = toml_schema_version(&raw, "config.toml")?;
        if version.is_some_and(|version| version > u64::from(CURRENT_CONFIG_VERSION)) {
            return Err(unsupported_config_error("config.toml", version));
        }
        if toml_config_requires_migration(&raw, version) {
            auto_migrate_legacy_config(&paths).await?;
            return load_current_config_from_paths(&paths).await;
        }
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
        Ok(_) => {
            auto_migrate_legacy_config(&paths).await?;
            return load_current_config_from_paths(&paths).await;
        }
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

fn unsupported_config_error(source: &str, source_version: Option<u64>) -> anyhow::Error {
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

async fn write_helper_config_locked(
    paths: &ResolvedConfigDirectory,
    existing: Option<&ExistingConfigToml>,
    cfg: &HelperConfig,
) -> Result<PathBuf> {
    let mut normalized = cfg.clone();
    normalized.version = CURRENT_CONFIG_VERSION;
    validate_helper_config(&normalized)?;

    let path = paths.logical_file("config.toml");
    let body = toml::to_string_pretty(&normalized)?;
    let text = format!("{CONFIG_TOML_DOC_HEADER}\n{body}");
    let data = text.into_bytes();

    if let Some(existing) = existing {
        write_config_backup(paths, existing, true).await?;
    }

    paths.ensure_unchanged().await?;
    let destination = paths.resolved_file("config.toml");
    if let Some(existing) = existing {
        let source_path = destination.clone();
        let expected_contents = existing.contents.clone();
        let expected_metadata = existing.metadata.clone();
        let staged_metadata = existing.metadata.clone();
        write_bytes_file_async_with_permissions_and_before_replace(
            &destination,
            &data,
            existing.metadata.permissions.clone(),
            move |staged_path, _destination| {
                staged_metadata.apply_to_staged_file(staged_path)?;
                verify_migration_source_snapshot(
                    &source_path,
                    "config.toml",
                    &expected_contents,
                    &expected_metadata,
                )
            },
        )
        .await?;
    } else {
        write_bytes_file_async(&destination, &data).await?;
    }
    paths.ensure_unchanged().await?;
    Ok(path)
}

#[cfg(test)]
pub(crate) async fn mutate_helper_config<T>(
    mutate: impl FnOnce(&mut HelperConfig) -> Result<T>,
) -> Result<(PathBuf, T)> {
    let paths = ResolvedConfigDirectory::prepare().await?;
    let _lock = ConfigMutationLock::try_acquire(&paths)?;
    paths.ensure_unchanged().await?;
    let existing = preflight_existing_config_before_save(&paths).await?;
    let mut config = match existing.as_ref() {
        Some(existing) => toml::from_str::<HelperConfig>(existing.text()?)?,
        None => HelperConfig::default(),
    };
    validate_helper_config(&config)?;
    let output = mutate(&mut config)?;
    let path = write_helper_config_locked(&paths, existing.as_ref(), &config).await?;
    Ok((path, output))
}

pub async fn save_helper_config(cfg: &HelperConfig) -> Result<PathBuf> {
    let paths = ResolvedConfigDirectory::prepare().await?;
    let _lock = ConfigMutationLock::try_acquire(&paths)?;
    paths.ensure_unchanged().await?;
    let existing = preflight_existing_config_before_save(&paths).await?;
    write_helper_config_locked(&paths, existing.as_ref(), cfg).await
}

#[cfg(test)]
#[path = "config/tests/storage_migration.rs"]
mod migration_tests;
