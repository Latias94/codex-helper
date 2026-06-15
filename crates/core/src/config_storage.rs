use super::bootstrap_impl::bootstrap_from_codex;
use super::*;
use crate::file_replace::{write_bytes_file_async, write_text_file};
use toml_edit::{
    Document as EditableTomlDocument, Item as EditableTomlItem, Table as EditableTomlTable,
    value as editable_toml_value,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexClientPatchConfig {
    pub preset: crate::codex_integration::CodexPatchMode,
    pub options: crate::codex_integration::CodexSwitchOptions,
    pub translate_models: bool,
}

impl Default for CodexClientPatchConfig {
    fn default() -> Self {
        Self {
            preset: crate::codex_integration::CodexPatchMode::Default,
            options: crate::codex_integration::CodexSwitchOptions::default(),
            translate_models: false,
        }
    }
}

fn parse_codex_client_patch_preset(
    field_name: &str,
    value: &str,
) -> Result<crate::codex_integration::CodexPatchMode> {
    match value.trim() {
        "default" => Ok(crate::codex_integration::CodexPatchMode::Default),
        "chatgpt-bridge" | "chatgpt_bridge" => {
            Ok(crate::codex_integration::CodexPatchMode::ChatGptBridge)
        }
        "imagegen-bridge" | "imagegen_bridge" => {
            Ok(crate::codex_integration::CodexPatchMode::ImagegenBridge)
        }
        "official-relay" | "official_relay" | "official-relay-bridge" | "official_relay_bridge" => {
            Ok(crate::codex_integration::CodexPatchMode::OfficialRelayBridge)
        }
        "official-imagegen"
        | "official_imagegen"
        | "official-imagegen-bridge"
        | "official_imagegen_bridge" => {
            Ok(crate::codex_integration::CodexPatchMode::OfficialImagegenBridge)
        }
        other => anyhow::bail!(
            "unsupported codex.client_patch.{} '{}'; expected 'default', 'chatgpt-bridge', 'imagegen-bridge', 'official-relay', or 'official-imagegen'. Legacy mode values are still accepted for reading. Use codex.client_patch.compaction or codex.client_patch.responses_websocket for orthogonal behavior instead of adding another preset.",
            field_name,
            other,
        ),
    }
}

fn parse_codex_compaction_strategy(
    value: &str,
) -> Result<crate::codex_integration::CodexCompactionStrategy> {
    match value.trim() {
        "" | "auto" => Ok(crate::codex_integration::CodexCompactionStrategy::Auto),
        "local" => Ok(crate::codex_integration::CodexCompactionStrategy::Local),
        "remote-v1" | "remote_v1" => {
            Ok(crate::codex_integration::CodexCompactionStrategy::RemoteV1)
        }
        "remote-v2" | "remote_v2" => {
            Ok(crate::codex_integration::CodexCompactionStrategy::RemoteV2)
        }
        other => anyhow::bail!(
            "unsupported codex.client_patch.compaction '{}'; expected 'auto', 'local', 'remote-v1', or 'remote-v2'",
            other,
        ),
    }
}

fn codex_client_patch_preset_from_toml_value(
    value: &TomlValue,
) -> Result<crate::codex_integration::CodexPatchMode> {
    let patch = value
        .get("codex")
        .and_then(|codex| codex.get("client_patch"));
    let preset = patch
        .and_then(|patch| patch.get("preset"))
        .and_then(TomlValue::as_str)
        .map(str::trim)
        .filter(|preset| !preset.is_empty());
    let legacy_mode = patch
        .and_then(|patch| patch.get("mode"))
        .and_then(TomlValue::as_str)
        .map(str::trim)
        .filter(|mode| !mode.is_empty());

    match (preset, legacy_mode) {
        (Some(preset), Some(mode)) => {
            let preset = parse_codex_client_patch_preset("preset", preset)?;
            let legacy_mode = parse_codex_client_patch_preset("mode", mode)?;
            if preset != legacy_mode {
                anyhow::bail!(
                    "conflicting codex.client_patch preset/mode values; keep only preset = \"{}\"",
                    preset.as_preset_str()
                );
            }
            Ok(preset)
        }
        (Some(preset), None) => parse_codex_client_patch_preset("preset", preset),
        (None, Some(mode)) => parse_codex_client_patch_preset("mode", mode),
        (None, None) => Ok(crate::codex_integration::CodexPatchMode::Default),
    }
}

fn codex_client_patch_config_from_toml_value(value: &TomlValue) -> Result<CodexClientPatchConfig> {
    let preset = codex_client_patch_preset_from_toml_value(value)?;
    let patch = value
        .get("codex")
        .and_then(|codex| codex.get("client_patch"));
    let responses_websocket = patch
        .and_then(|patch| patch.get("responses_websocket"))
        .and_then(TomlValue::as_bool)
        .unwrap_or(false);
    let compaction = patch
        .and_then(|patch| patch.get("compaction"))
        .and_then(TomlValue::as_str)
        .map(parse_codex_compaction_strategy)
        .transpose()?
        .unwrap_or_default();
    let translate_models = patch
        .and_then(|patch| patch.get("translate_models"))
        .and_then(TomlValue::as_bool)
        .unwrap_or(false);

    Ok(CodexClientPatchConfig {
        preset,
        options: crate::codex_integration::CodexSwitchOptions {
            responses_websocket,
            compaction,
        },
        translate_models,
    })
}

pub fn codex_client_patch_config_from_config_file() -> Result<CodexClientPatchConfig> {
    let path = config_file_path();
    if !path.exists() || path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
        return Ok(CodexClientPatchConfig::default());
    }

    let text = stdfs::read_to_string(&path).with_context(|| format!("read {:?}", path))?;
    let value: TomlValue = toml::from_str(&text).with_context(|| format!("parse {:?}", path))?;
    codex_client_patch_config_from_toml_value(&value)
}

pub fn codex_client_patch_preset_from_config_file()
-> Result<crate::codex_integration::CodexPatchMode> {
    Ok(codex_client_patch_config_from_config_file()?.preset)
}

pub fn codex_client_patch_mode_from_config_file() -> Result<crate::codex_integration::CodexPatchMode>
{
    codex_client_patch_preset_from_config_file()
}

fn existing_codex_client_patch_item() -> Option<EditableTomlItem> {
    let path = config_toml_path();
    let text = stdfs::read_to_string(path).ok()?;
    let doc = text.parse::<EditableTomlDocument>().ok()?;
    doc.as_table()
        .get("codex")
        .and_then(EditableTomlItem::as_table)
        .and_then(|codex| codex.get("client_patch"))
        .cloned()
        .map(normalize_existing_codex_client_patch_item)
}

fn normalize_existing_codex_client_patch_item(mut item: EditableTomlItem) -> EditableTomlItem {
    let Some(table) = item.as_table_mut() else {
        return item;
    };
    let preset = table
        .get("preset")
        .and_then(EditableTomlItem::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            table
                .get("mode")
                .and_then(EditableTomlItem::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .and_then(|value| parse_codex_client_patch_preset("preset", value).ok());

    if let Some(preset) = preset {
        table.remove("mode");
        table.insert("preset", editable_toml_value(preset.as_preset_str()));
    }

    item
}

fn preserve_existing_codex_client_patch(text: String) -> String {
    let Some(client_patch) = existing_codex_client_patch_item() else {
        return text;
    };
    let Ok(mut doc) = text.parse::<EditableTomlDocument>() else {
        return text;
    };

    let root = doc.as_table_mut();
    if !root.contains_key("codex") {
        root.insert("codex", EditableTomlItem::Table(EditableTomlTable::new()));
    }
    let Some(codex) = root
        .get_mut("codex")
        .and_then(EditableTomlItem::as_table_mut)
    else {
        return text;
    };
    codex.insert("client_patch", client_patch);
    doc.to_string()
}

fn codex_client_patch_item_needs_normalization(item: &EditableTomlItem) -> bool {
    let Some(table) = item.as_table() else {
        return false;
    };
    let active_mode = table
        .get("mode")
        .and_then(EditableTomlItem::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if active_mode.is_some() {
        return true;
    }

    let active_preset = table
        .get("preset")
        .and_then(EditableTomlItem::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    active_preset
        .and_then(|value| {
            parse_codex_client_patch_preset("preset", value)
                .ok()
                .map(|preset| (value, preset))
        })
        .is_some_and(|(value, preset)| value != preset.as_preset_str())
}

fn normalize_codex_client_patch_doc(doc: &mut EditableTomlDocument) -> bool {
    let Some(codex) = doc
        .as_table_mut()
        .get_mut("codex")
        .and_then(EditableTomlItem::as_table_mut)
    else {
        return false;
    };
    let Some(existing) = codex.get("client_patch").cloned() else {
        return false;
    };
    if !codex_client_patch_item_needs_normalization(&existing) {
        return false;
    }

    let normalized = normalize_existing_codex_client_patch_item(existing);
    codex.insert("client_patch", normalized);
    true
}

fn normalize_route_graph_affinity_doc(
    doc: &mut EditableTomlDocument,
    schema_version: Option<u32>,
) -> bool {
    if !schema_version.is_some_and(is_supported_route_graph_config_version) {
        return false;
    }

    let mut changed = false;
    for service_name in ["codex", "claude"] {
        let Some(service) = doc
            .as_table_mut()
            .get_mut(service_name)
            .and_then(EditableTomlItem::as_table_mut)
        else {
            continue;
        };
        let Some(routing) = service
            .get_mut("routing")
            .and_then(EditableTomlItem::as_table_mut)
        else {
            continue;
        };
        if !routing.contains_key("affinity_policy") {
            routing.insert("affinity_policy", editable_toml_value("fallback-sticky"));
            changed = true;
        }
    }
    changed
}

fn normalize_config_toml_authoring_text(text: &str) -> Result<Option<String>> {
    let mut doc = text.parse::<EditableTomlDocument>()?;
    let mut changed = normalize_codex_client_patch_doc(&mut doc);
    changed |= normalize_route_graph_affinity_doc(&mut doc, toml_schema_version_or_shape(text));
    if !changed {
        return Ok(None);
    }
    let normalized_text = doc.to_string();
    if normalized_text == text {
        Ok(None)
    } else {
        Ok(Some(normalized_text))
    }
}

pub fn normalize_config_toml_authoring() -> Result<Option<PathBuf>> {
    let path = config_toml_path();
    if !path.exists() {
        return Ok(None);
    }

    let text = stdfs::read_to_string(&path).with_context(|| format!("read {:?}", path))?;
    let Some(normalized) = normalize_config_toml_authoring_text(&text)? else {
        return Ok(None);
    };

    let backup_path = config_toml_backup_path();
    if let Err(err) = stdfs::copy(&path, &backup_path) {
        warn!("failed to backup {:?} to {:?}: {}", path, backup_path, err);
    }
    write_text_file(&path, &normalized)?;
    Ok(Some(path))
}

pub fn normalize_config_toml_client_patch() -> Result<Option<PathBuf>> {
    normalize_config_toml_authoring()
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

const CONFIG_VERSION: u32 = CURRENT_ROUTE_GRAPH_CONFIG_VERSION;

#[derive(Debug, Clone)]
pub struct LoadedProxyConfig {
    pub runtime: ProxyConfig,
    pub v4: Option<ProxyConfigV4>,
}

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

version = 5

# 省略 --codex/--claude 时默认使用哪个服务。
# default_service = "codex"
# default_service = "claude"

# --- Codex 客户端 patch 预设（可选） ---
#
# default：保持历史行为，只把 ~/.codex/config.toml 的 model_provider 指到本地代理。
# chatgpt-bridge：保留 Codex/ChatGPT 登录态用于移动端/桌面端账号能力，同时模型请求进入 codex-helper。
# imagegen-bridge：实验模式；把 auth.json 临时写成空对象 {}，利用 Codex 默认 ChatGPT
#                  auth 解析暴露 hosted image_generation；实际上游凭据仍来自 codex-helper routing / env key。
# official-relay：实验模式；把本地 codex_proxy 声明为 OpenAI Responses provider，
#                 默认让 Codex 使用远程压缩路径，例如 /responses/compact。
#                 真实上游凭据仍来自 codex-helper routing / env key。
# official-imagegen：实验模式；同时启用 official-relay 的 OpenAI provider
#                    标识和 imagegen-bridge 的 {} auth facade；默认同时尝试
#                    远程压缩与 hosted image_generation。
# 启用 chatgpt-bridge 时，`switch on --preset chatgpt-bridge` 会把 ~/.codex/auth.json 的
# auth_mode 改为 "chatgpt"，OPENAI_API_KEY 改为 null，其它字段不动。
# 启用 imagegen-bridge / official-imagegen 时，`switch on --preset ...` 会临时把 ~/.codex/auth.json
# 改为 {} facade，并在 `switch off` 或切回 default 时安全恢复。
# 该预设启用前会校验 Codex 至少有一个已启用上游，且当前进程能读到其上游凭据。
# responses_websocket：正交传输开关；为 true 时会写 Codex provider 的
#                      supports_websockets = true，让 Codex 可选择 Responses WebSocket v2。
#                      只应与 official-relay / official-imagegen 搭配，
#                      且仅在 helper 与所选中转都支持 WebSocket relay 时开启。
# compaction：正交压缩策略。auto 保持 preset 默认：default / imagegen-bridge
#             更偏向本地压缩，official-relay / official-imagegen 默认让 Codex
#             走远程压缩路径；local 强制本地压缩；remote-v1 强制 /responses/compact；
#             remote-v2 强制 remote_compaction_v2，并依赖 helper 的 v2->v1 降级兜底。
# translate_models：默认 false。false 时 helper 只解码 /models 响应压缩体，
#                   不把 OpenAI data 列表翻译成 Codex models catalog，让 Codex 使用
#                   自带 models.json / fallback 元数据；true 仅用于确实需要 helper
#                   合成模型目录的中转，因为 Codex 会把合成后的字段当成权威。
# 请求体 Content-Encoding 默认自动归一化（zstd / gzip / br / deflate），并会把
# body.prompt_cache_key 作为缺省 session affinity 信号。极少数中转若必须接收
# 原始 Codex 压缩体，请在启动 helper 的环境里设置：
# CODEX_HELPER_REQUEST_BODY_ENCODING=passthrough
# 兼容性：旧配置键 mode 仍会被读取；保存/生成配置时统一写 preset。
#
# [codex.client_patch]
# preset = "default"
# preset = "chatgpt-bridge"
# preset = "imagegen-bridge"
# preset = "official-relay"
# preset = "official-imagegen"
# responses_websocket = false
# compaction = "auto"
# translate_models = false

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
# admin_url = "http://nas.local:4211"
# admin_token_env = "CODEX_HELPER_NAS_ADMIN_TOKEN"
# client_preset = "official-relay"
# responses_websocket = false
#
# 常用命令：
#   ch relay add nas --proxy-url http://nas.local:3211 --admin-url http://nas.local:4211 --admin-token-env CODEX_HELPER_NAS_ADMIN_TOKEN --preset official-relay
#   ch relay nas
#   ch relay nas --no-tui
#   ch relay nas --attach-only
#   ch relay off

# --- TUI 用量预测（可选） ---
#
# TUI Stats 页会按最近窗口的已计价请求估算 USD/h，并外推到下次配额刷新时间。
# 如果你的中转站余额每天本地 0 点刷新，保留下面默认即可；如果按其它时区结算，改 reset_utc_offset。
#
# [ui.usage_forecast]
# enabled = true
# rate_window_minutes = 60
# min_priced_requests = 2
# reset_time = "00:00"
# reset_utc_offset = "+08:00"

# --- 自动导入（可选） ---
#
# 如果你的机器上已配置 Codex CLI（存在 `~/.codex/config.toml`），`codex-helper config init`
# 会尝试自动把 Codex providers / routing 导入到本文件中，避免你手动抄写 base_url/env_key。
#
# 如果你只想生成纯模板（不导入），请使用：
#   codex-helper config init --no-import

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
# on_status = "429,500-502,504-528,530-599"
# on_class = ["upstream_transport_error", "cloudflare_timeout", "cloudflare_challenge", "upstream_rate_limited", "upstream_overloaded"]
#
# [retry.provider]
# max_attempts = 2
# strategy = "failover"
# on_status = "401,403,404,408,429,500-599,524"
# on_class = ["upstream_transport_error", "upstream_rate_limited", "upstream_overloaded"]

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

fn insert_after_version_block(template: &str, insert: &str) -> String {
    let needle = "version = 5\n\n";
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

    let has_v4_routing = ["codex", "claude"].iter().any(|service| {
        value
            .get(*service)
            .and_then(|service| service.get("routing"))
            .and_then(|routing| routing.get("entry").or_else(|| routing.get("routes")))
            .is_some()
    });
    if has_v4_routing {
        Some(4)
    } else {
        let has_legacy_routing = ["codex", "claude"].iter().any(|service| {
            value
                .get(*service)
                .and_then(|service| service.get("routing"))
                .is_some()
        });
        if has_legacy_routing { Some(3) } else { None }
    }
}

fn codex_bootstrap_snippet() -> Result<Option<String>> {
    #[derive(Serialize)]
    struct CodexOnly<'a> {
        codex: &'a ServiceViewV4,
    }

    let mut cfg = ProxyConfig::default();
    ensure_config_version(&mut cfg);
    if bootstrap_from_codex(&mut cfg).is_err() {
        return Ok(None);
    }
    if !cfg.codex.has_stations() {
        return Ok(None);
    }

    let migrated = migrate_legacy_to_v4(&cfg)?;
    let mut migrated = migrated;
    if let Some(routing) = migrated.codex.routing.as_mut() {
        routing.affinity_policy = RoutingAffinityPolicyV5::FallbackSticky;
    }
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
    Ok(load_config_with_v4_source().await?.runtime)
}

pub async fn load_config_with_v4_source() -> Result<LoadedProxyConfig> {
    let toml_path = config_toml_path();
    if toml_path.exists() {
        let text = fs::read_to_string(&toml_path).await?;
        let version = toml_schema_version_or_shape(&text);

        let mut loaded_v4 = None;
        let mut cfg = if version.is_some_and(is_supported_route_graph_config_version) {
            let cfg_v4 = toml::from_str::<ProxyConfigV4>(&text)?;
            let runtime = compile_v4_to_runtime(&cfg_v4)?;
            loaded_v4 = Some(cfg_v4);
            runtime
        } else if version == Some(3) {
            let cfg_legacy = toml::from_str::<crate::config::legacy::ProxyConfigV3Legacy>(&text)?;
            let migrated = crate::config::legacy::migrate_v3_legacy_to_v4(&cfg_legacy)?;
            let runtime = compile_v4_to_runtime(&migrated.config)?;
            loaded_v4 = Some(migrated.config);
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
        if version != Some(CURRENT_ROUTE_GRAPH_CONFIG_VERSION) {
            if let Some(cfg_v4) = loaded_v4.as_mut() {
                auto_migrate_loaded_v4_config(cfg_v4, "config.toml", version).await;
                cfg_v4.version = CURRENT_ROUTE_GRAPH_CONFIG_VERSION;
                cfg.version = Some(CURRENT_ROUTE_GRAPH_CONFIG_VERSION);
            } else {
                auto_migrate_loaded_config(&mut cfg, "config.toml", version).await;
            }
        } else if let Some(cfg_v4) = loaded_v4.as_ref() {
            auto_compact_loaded_v4_config(cfg_v4, "config.toml").await;
        }
        auto_normalize_loaded_config_toml_authoring("config.toml").await;
        return Ok(LoadedProxyConfig {
            runtime: cfg,
            v4: loaded_v4,
        });
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
        return Ok(LoadedProxyConfig {
            runtime: cfg,
            v4: None,
        });
    }

    let mut cfg = ProxyConfig::default();
    ensure_config_version(&mut cfg);
    normalize_proxy_config(&mut cfg);
    validate_proxy_config(&cfg)?;
    Ok(LoadedProxyConfig {
        runtime: cfg,
        v4: None,
    })
}

async fn auto_migrate_loaded_config(
    cfg: &mut ProxyConfig,
    source: &str,
    source_version: Option<u32>,
) {
    match save_config(cfg).await {
        Ok(()) => {
            cfg.version = Some(CURRENT_ROUTE_GRAPH_CONFIG_VERSION);
            info!(
                "auto-migrated {} from version {:?} to version {}",
                source, source_version, CURRENT_ROUTE_GRAPH_CONFIG_VERSION
            );
        }
        Err(err) => {
            warn!(
                "failed to auto-migrate {} from version {:?} to version {}: {}",
                source, source_version, CURRENT_ROUTE_GRAPH_CONFIG_VERSION, err
            );
        }
    }
}

async fn auto_migrate_loaded_v4_config(
    cfg: &ProxyConfigV4,
    source: &str,
    source_version: Option<u32>,
) {
    match save_config_v4(cfg).await {
        Ok(_) => {
            info!(
                "auto-migrated {} from version {:?} to version {}",
                source, source_version, CURRENT_ROUTE_GRAPH_CONFIG_VERSION
            );
        }
        Err(err) => {
            warn!(
                "failed to auto-migrate {} from version {:?} to version {}: {}",
                source, source_version, CURRENT_ROUTE_GRAPH_CONFIG_VERSION, err
            );
        }
    }
}

fn runtime_service_manager_value(mgr: &ServiceConfigManager) -> Result<JsonValue> {
    serde_json::to_value(mgr).context("serialize runtime service manager")
}

fn v4_service_has_import_metadata(view: &ServiceViewV4) -> bool {
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

async fn auto_compact_loaded_v4_config(cfg: &ProxyConfigV4, source: &str) {
    if !v4_service_has_import_metadata(&cfg.codex) && !v4_service_has_import_metadata(&cfg.claude) {
        return;
    }

    match save_config_v4(cfg).await {
        Ok(_) => {
            info!(
                "auto-compacted {} v4 provider config metadata for authoring format",
                source
            );
        }
        Err(err) => {
            warn!(
                "failed to auto-compact {} v4 provider config metadata: {}",
                source, err
            );
        }
    }
}

async fn auto_normalize_loaded_config_toml_authoring(source: &str) {
    let path = config_toml_path();
    let text = match fs::read_to_string(&path).await {
        Ok(text) => text,
        Err(err) => {
            warn!(
                "failed to read {} while normalizing config authoring fields: {}",
                source, err
            );
            return;
        }
    };
    let normalized = match normalize_config_toml_authoring_text(&text) {
        Ok(Some(normalized)) => normalized,
        Ok(None) => return,
        Err(err) => {
            warn!(
                "failed to normalize {} config authoring fields: {}",
                source, err
            );
            return;
        }
    };

    let backup_path = config_toml_backup_path();
    if path.exists()
        && let Err(err) = fs::copy(&path, &backup_path).await
    {
        warn!("failed to backup {:?} to {:?}: {}", path, backup_path, err);
    }

    match write_bytes_file_async(&path, normalized.as_bytes()).await {
        Ok(()) => {
            info!("auto-normalized {} config authoring fields", source);
        }
        Err(err) => {
            warn!(
                "failed to auto-normalize {} config authoring fields: {}",
                source, err
            );
        }
    }
}

async fn save_existing_v4_if_only_runtime_metadata_changed(
    cfg: &ProxyConfig,
) -> Result<Option<PathBuf>> {
    let path = config_toml_path();
    if !path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&path).await?;
    if !toml_schema_version_or_shape(&text).is_some_and(is_supported_route_graph_config_version) {
        return Ok(None);
    }

    let mut requested = cfg.clone();
    normalize_proxy_config(&mut requested);
    validate_proxy_config(&requested)?;

    let mut existing = toml::from_str::<ProxyConfigV4>(&text)?;
    let mut existing_runtime = compile_v4_to_runtime(&existing)?;
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
    existing.relay_targets = requested.relay_targets;
    existing.ui = requested.ui;
    save_config_v4(&existing).await.map(Some)
}

pub async fn save_config(cfg: &ProxyConfig) -> Result<()> {
    if cfg
        .version
        .is_some_and(is_supported_route_graph_config_version)
    {
        if save_existing_v4_if_only_runtime_metadata_changed(cfg)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let migrated = migrate_legacy_to_v4(cfg)?;
        save_config_v4(&migrated).await?;
        return Ok(());
    }

    let migrated = migrate_legacy_to_v4(cfg)?;
    save_config_v4(&migrated).await?;
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
    let text = preserve_existing_codex_client_patch(format!(
        "{CONFIG_TOML_DOC_HEADER}
{body}"
    ));
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

pub async fn save_config_v4(cfg: &ProxyConfigV4) -> Result<PathBuf> {
    let mut normalized = cfg.clone();
    normalized.version = CURRENT_ROUTE_GRAPH_CONFIG_VERSION;
    compact_v4_config_for_write(&mut normalized);
    let mut runtime = compile_v4_to_runtime(&normalized)?;
    normalize_proxy_config(&mut runtime);
    validate_proxy_config(&runtime)?;

    let dir = config_dir();
    fs::create_dir_all(&dir).await?;
    let path = config_toml_path();
    let (backup_source_path, backup_path) = config_backup_source_and_path();
    let body = toml::to_string_pretty(&normalized)?;
    let text = preserve_existing_codex_client_patch(format!(
        "{CONFIG_TOML_DOC_HEADER}
{body}"
    ));
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
