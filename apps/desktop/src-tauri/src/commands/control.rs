use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use codex_helper_core::codex_integration::{
    self, CodexPatchMode, CodexSwitchOptions, CodexSwitchStatus,
};
use codex_helper_core::proxy::RuntimeStatusResponse;
use codex_helper_core::runtime_manager::{
    RuntimeConnectionMode, RuntimeOwnerKind, RuntimeOwnerMarker, RuntimeStopAction,
    RuntimeStopIntent, decide_runtime_stop_action, read_owner_marker_best_effort,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use super::admin_api::{
    admin_client, admin_endpoint_config, get_json, post_empty, post_json, post_json_no_response,
};
use crate::error::{CommandError, DesktopError};

const SERVICE_NAME: &str = "codex";
const RUNTIME_STATUS_PATH: &str = "/__codex_helper/api/v1/runtime/status";
const RUNTIME_RELOAD_PATH: &str = "/__codex_helper/api/v1/runtime/reload";
const RUNTIME_SHUTDOWN_PATH: &str = "/__codex_helper/api/v1/runtime/shutdown";
const STATIONS_PROBE_PATH: &str = "/__codex_helper/api/v1/stations/probe";
const PROVIDERS_BALANCES_REFRESH_PATH: &str = "/__codex_helper/api/v1/providers/balances/refresh";
const PROVIDERS_RUNTIME_PATH: &str = "/__codex_helper/api/v1/providers/runtime";
const GLOBAL_ROUTE_OVERRIDE_PATH: &str = "/__codex_helper/api/v1/overrides/global-route";
const SESSION_OVERRIDES_PATH: &str = "/__codex_helper/api/v1/overrides/session";
const SESSION_OVERRIDES_RESET_PATH: &str = "/__codex_helper/api/v1/overrides/session/reset";

pub(crate) const OWNED_STOP_CONFIRMATION: &str = "STOP OWNED PROXY";
pub(crate) const ATTACHED_STOP_CONFIRMATION: &str = "STOP ATTACHED PROXY";
pub(crate) const CODEX_SWITCH_ON_CONFIRMATION: &str = "SWITCH CODEX";
pub(crate) const CODEX_SWITCH_OFF_CONFIRMATION: &str = "SWITCH OFF CODEX";

const CLI_PATH_ENV: &str = "CODEX_HELPER_CLI_PATH";
const CLI_PATH_ENV_LEGACY: &str = "CODEX_HELPER_CLI";
const STARTUP_POLL_TIMEOUT: Duration = Duration::from_millis(5_000);
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesktopRuntimeConnectionMode {
    DesktopOwned,
    Attached,
    Stopped,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopControlState {
    pub connection_mode: DesktopRuntimeConnectionMode,
    pub proxy_port: u16,
    pub admin_port: u16,
    pub proxy_base_url: String,
    pub admin_base_url: String,
    pub reachable: bool,
    pub shutdown_available: bool,
    pub owner: Option<RuntimeOwnerMarker>,
    pub codex_switch: CodexSwitchSnapshot,
    pub can_start: bool,
    pub can_attach: bool,
    pub can_stop_owned: bool,
    pub can_remote_stop: bool,
    pub can_switch_on: bool,
    pub can_switch_off: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSwitchSnapshot {
    pub enabled: bool,
    pub model_provider: Option<String>,
    pub provider_name: Option<String>,
    pub base_url: Option<String>,
    pub preset: Option<String>,
    pub requires_openai_auth: Option<bool>,
    pub supports_websockets: Option<bool>,
    pub remote_compaction_v2_enabled: bool,
    pub has_switch_state: bool,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopActionResult {
    pub ok: bool,
    pub action: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<DesktopControlState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopProxyPayload {
    pub scope: StopProxyScope,
    pub confirmation: String,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopProxyScope {
    Owned,
    Attached,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchCodexPayload {
    pub enabled: bool,
    #[serde(default)]
    pub preset: Option<CodexPatchMode>,
    #[serde(default)]
    pub responses_websocket: bool,
    pub confirmation: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationProbePayload {
    pub station_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBalanceRefreshPayload {
    #[serde(default)]
    pub station_name: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRuntimeOverridePayload {
    pub provider_name: String,
    #[serde(default)]
    pub endpoint_name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub clear_enabled: bool,
    #[serde(default)]
    pub runtime_state: Option<ProviderRuntimeState>,
    #[serde(default)]
    pub clear_runtime_state: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderRuntimeState {
    Normal,
    Draining,
    BreakerOpen,
    HalfOpen,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalRouteOverridePayload {
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOverridePayload {
    pub session_id: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub station_name: Option<String>,
    #[serde(default)]
    pub route_target: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub clear: Vec<SessionOverrideDimension>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOverrideDimension {
    Model,
    ReasoningEffort,
    StationName,
    RouteTarget,
    ServiceTier,
    All,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetSessionOverridesPayload {
    pub session_id: String,
}

#[tauri::command]
pub async fn get_desktop_control_state() -> Result<DesktopControlState, CommandError> {
    load_control_state().await
}

#[tauri::command]
pub async fn attach_existing_proxy() -> Result<DesktopActionResult, CommandError> {
    let state = load_control_state().await?;
    if !state.reachable {
        return Err(DesktopError::Lifecycle(
            "没有可附加的本地代理；请先启动 codex-helper serve --codex。".to_string(),
        )
        .into());
    }

    Ok(result_with_state(
        "attach-existing-proxy",
        "已附加到当前本地代理；普通退出只会关闭桌面窗口，不会停止该代理。",
        state,
    ))
}

#[tauri::command]
pub async fn start_desktop_proxy(app: AppHandle) -> Result<DesktopActionResult, CommandError> {
    let before = load_control_state().await?;
    if before.reachable {
        return Ok(result_with_state(
            "start-desktop-proxy",
            "本地代理已经可用；桌面端已附加到现有运行时。",
            before,
        ));
    }

    let endpoint = admin_endpoint_config();
    let cli_path = resolve_cli_path(&app)?;
    let mut command = Command::new(&cli_path);
    let port_arg = endpoint.proxy_port.to_string();
    command
        .args([
            "serve",
            "--codex",
            "--host",
            "127.0.0.1",
            "--port",
            port_arg.as_str(),
            "--no-tui",
            "--desktop-managed",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }

    let child = command.spawn().map_err(|err| {
        DesktopError::Lifecycle(format!(
            "无法启动 codex-helper CLI {}: {err}",
            cli_path.display()
        ))
    })?;
    let child_id = child.id();
    drop(child);

    let state = wait_for_runtime_reachable().await?;
    Ok(DesktopActionResult {
        ok: true,
        action: "start-desktop-proxy".to_string(),
        message: format!(
            "已启动桌面托管代理进程 pid={child_id}；后续普通关闭窗口不会执行远程停止。"
        ),
        state: Some(state),
        payload: None,
    })
}

#[tauri::command]
pub async fn stop_proxy(payload: StopProxyPayload) -> Result<DesktopActionResult, CommandError> {
    let state = load_control_state().await?;
    let expected = match payload.scope {
        StopProxyScope::Owned => OWNED_STOP_CONFIRMATION,
        StopProxyScope::Attached => ATTACHED_STOP_CONFIRMATION,
    };
    require_confirmation(payload.confirmation.as_str(), expected)?;

    let action = explicit_stop_action(&state);

    match (payload.scope, action) {
        (StopProxyScope::Owned, RuntimeStopAction::StopOwnedRuntime) => {
            shutdown_runtime("stop-owned-proxy").await
        }
        (StopProxyScope::Attached, RuntimeStopAction::ShutdownAttachedRuntime) => {
            shutdown_runtime("remote-stop-attached-proxy").await
        }
        (StopProxyScope::Owned, RuntimeStopAction::Noop) => Ok(DesktopActionResult {
            ok: true,
            action: "stop-owned-proxy".to_string(),
            message: "当前没有运行中的桌面托管代理，无需停止。".to_string(),
            state: Some(state),
            payload: None,
        }),
        (StopProxyScope::Owned, _) => Err(DesktopError::Lifecycle(
            "当前运行时不是桌面托管代理，不能用 owned stop；如需停止外部代理，请使用显式 remote stop。"
                .to_string(),
        )
        .into()),
        (StopProxyScope::Attached, RuntimeStopAction::DetachOnly) => Ok(DesktopActionResult {
            ok: true,
            action: "detach-only".to_string(),
            message: "该附加运行时不支持远程 shutdown；桌面端只会 detach，不会杀死外部进程。".to_string(),
            state: Some(state),
            payload: None,
        }),
        (StopProxyScope::Attached, RuntimeStopAction::Noop) => Ok(DesktopActionResult {
            ok: true,
            action: "remote-stop-attached-proxy".to_string(),
            message: "当前没有运行中的本地代理，无需停止。".to_string(),
            state: Some(state),
            payload: None,
        }),
        (StopProxyScope::Attached, RuntimeStopAction::StopOwnedRuntime) => Err(
            DesktopError::Lifecycle("当前运行时属于桌面托管，请使用 owned stop。".to_string()).into(),
        ),
    }
}

#[tauri::command]
pub async fn switch_codex(
    payload: SwitchCodexPayload,
) -> Result<DesktopActionResult, CommandError> {
    let endpoint = admin_endpoint_config();
    if payload.enabled {
        require_confirmation(payload.confirmation.as_str(), CODEX_SWITCH_ON_CONFIRMATION)?;
        let mode = payload.preset.unwrap_or(CodexPatchMode::Default);
        codex_integration::switch_on_with_options(
            endpoint.proxy_port,
            mode,
            CodexSwitchOptions {
                responses_websocket: payload.responses_websocket,
            },
        )
        .map_err(|err| DesktopError::Switch(err.to_string()))?;
        let state = load_control_state().await?;
        Ok(result_with_state(
            "switch-codex-on",
            format!("已将 Codex 切换到本地代理预设 {}。", mode.as_preset_str()),
            state,
        ))
    } else {
        require_confirmation(payload.confirmation.as_str(), CODEX_SWITCH_OFF_CONFIRMATION)?;
        codex_integration::switch_off().map_err(|err| DesktopError::Switch(err.to_string()))?;
        let state = load_control_state().await?;
        Ok(result_with_state(
            "switch-codex-off",
            "已恢复 Codex 配置，不再强制走本地代理。",
            state,
        ))
    }
}

#[tauri::command]
pub async fn reload_runtime() -> Result<DesktopActionResult, CommandError> {
    let client = admin_client()?;
    let endpoint = admin_endpoint_config();
    let payload: Value = post_json(
        &client,
        &endpoint.admin_base_url,
        RUNTIME_RELOAD_PATH,
        &json!({}),
    )
    .await?;
    let state = load_control_state().await?;
    Ok(result_with_payload(
        "reload-runtime",
        "已请求运行时重新加载配置。",
        state,
        payload,
    ))
}

#[tauri::command]
pub async fn probe_station(
    payload: StationProbePayload,
) -> Result<DesktopActionResult, CommandError> {
    let client = admin_client()?;
    let endpoint = admin_endpoint_config();
    let request = json!({ "station_name": payload.station_name });
    let response: Value = post_json(
        &client,
        &endpoint.admin_base_url,
        STATIONS_PROBE_PATH,
        &request,
    )
    .await?;
    let state = load_control_state().await?;
    Ok(result_with_payload(
        "probe-station",
        "已发起 station 探测。",
        state,
        response,
    ))
}

#[tauri::command]
pub async fn refresh_provider_balances(
    payload: ProviderBalanceRefreshPayload,
) -> Result<DesktopActionResult, CommandError> {
    let client = admin_client()?;
    let endpoint = admin_endpoint_config();
    let path = provider_balance_refresh_path(payload);
    let response: Value = post_json(&client, &endpoint.admin_base_url, &path, &json!({})).await?;
    let state = load_control_state().await?;
    Ok(result_with_payload(
        "refresh-provider-balances",
        "已刷新供应商余额。",
        state,
        response,
    ))
}

#[tauri::command]
pub async fn apply_provider_runtime_override(
    payload: ProviderRuntimeOverridePayload,
) -> Result<DesktopActionResult, CommandError> {
    if payload.enabled.is_none()
        && payload.runtime_state.is_none()
        && !payload.clear_enabled
        && !payload.clear_runtime_state
    {
        return Err(
            DesktopError::Lifecycle("至少需要一个 provider override 动作。".to_string()).into(),
        );
    }
    let client = admin_client()?;
    let endpoint = admin_endpoint_config();
    let body = json!({
        "provider_name": payload.provider_name,
        "endpoint_name": clean_optional(payload.endpoint_name),
        "enabled": payload.enabled,
        "clear_enabled": payload.clear_enabled,
        "runtime_state": payload.runtime_state,
        "clear_runtime_state": payload.clear_runtime_state,
    });
    post_empty_with_json(
        &client,
        &endpoint.admin_base_url,
        PROVIDERS_RUNTIME_PATH,
        &body,
    )
    .await?;
    let state = load_control_state().await?;
    Ok(result_with_state(
        "apply-provider-runtime-override",
        "已更新 provider 运行时覆盖。",
        state,
    ))
}

#[tauri::command]
pub async fn set_global_route_override(
    payload: GlobalRouteOverridePayload,
) -> Result<DesktopActionResult, CommandError> {
    let client = admin_client()?;
    let endpoint = admin_endpoint_config();
    let body = json!({ "target": clean_optional(payload.target) });
    post_empty_with_json(
        &client,
        &endpoint.admin_base_url,
        GLOBAL_ROUTE_OVERRIDE_PATH,
        &body,
    )
    .await?;
    let state = load_control_state().await?;
    Ok(result_with_state(
        "set-global-route-override",
        "已更新全局路由覆盖。",
        state,
    ))
}

#[tauri::command]
pub async fn apply_session_overrides(
    payload: SessionOverridePayload,
) -> Result<DesktopActionResult, CommandError> {
    if payload.session_id.trim().is_empty() {
        return Err(DesktopError::Lifecycle("session_id 不能为空。".to_string()).into());
    }
    let client = admin_client()?;
    let endpoint = admin_endpoint_config();
    let body = json!({
        "session_id": payload.session_id,
        "model": clean_optional(payload.model),
        "reasoning_effort": clean_optional(payload.reasoning_effort),
        "station_name": clean_optional(payload.station_name),
        "route_target": clean_optional(payload.route_target),
        "service_tier": clean_optional(payload.service_tier),
        "clear": payload.clear,
    });
    let response: Value = post_json(
        &client,
        &endpoint.admin_base_url,
        SESSION_OVERRIDES_PATH,
        &body,
    )
    .await?;
    let state = load_control_state().await?;
    Ok(result_with_payload(
        "apply-session-overrides",
        "已更新会话覆盖。",
        state,
        response,
    ))
}

#[tauri::command]
pub async fn reset_session_overrides(
    payload: ResetSessionOverridesPayload,
) -> Result<DesktopActionResult, CommandError> {
    if payload.session_id.trim().is_empty() {
        return Err(DesktopError::Lifecycle("session_id 不能为空。".to_string()).into());
    }
    let client = admin_client()?;
    let endpoint = admin_endpoint_config();
    let body = json!({ "session_id": payload.session_id });
    post_empty_with_json(
        &client,
        &endpoint.admin_base_url,
        SESSION_OVERRIDES_RESET_PATH,
        &body,
    )
    .await?;
    let state = load_control_state().await?;
    Ok(result_with_state(
        "reset-session-overrides",
        "已清除该会话覆盖。",
        state,
    ))
}

async fn load_control_state() -> Result<DesktopControlState, CommandError> {
    let endpoint = admin_endpoint_config();
    let client = admin_client()?;
    let runtime_status =
        get_json::<RuntimeStatusResponse>(&client, &endpoint.admin_base_url, RUNTIME_STATUS_PATH)
            .await
            .ok();
    let reachable = runtime_status.is_some();
    let shutdown_available = runtime_status
        .as_ref()
        .is_some_and(|status| status.shutdown_available);
    let owner = read_owner_marker_best_effort(SERVICE_NAME, endpoint.proxy_port);
    let connection_mode = classify_connection_mode(reachable, owner.as_ref());
    let codex_switch = codex_switch_snapshot();

    Ok(DesktopControlState {
        connection_mode,
        proxy_port: endpoint.proxy_port,
        admin_port: endpoint.admin_port,
        proxy_base_url: endpoint.proxy_base_url,
        admin_base_url: endpoint.admin_base_url,
        reachable,
        shutdown_available,
        owner,
        codex_switch,
        can_start: !reachable,
        can_attach: reachable && connection_mode != DesktopRuntimeConnectionMode::DesktopOwned,
        can_stop_owned: connection_mode == DesktopRuntimeConnectionMode::DesktopOwned
            && shutdown_available,
        can_remote_stop: connection_mode == DesktopRuntimeConnectionMode::Attached
            && shutdown_available,
        can_switch_on: reachable,
        can_switch_off: true,
    })
}

fn classify_connection_mode(
    reachable: bool,
    owner: Option<&RuntimeOwnerMarker>,
) -> DesktopRuntimeConnectionMode {
    match (reachable, owner.map(|marker| marker.owner)) {
        (true, Some(RuntimeOwnerKind::Desktop)) => DesktopRuntimeConnectionMode::DesktopOwned,
        (true, Some(RuntimeOwnerKind::ManualCli | RuntimeOwnerKind::Supervisor)) => {
            DesktopRuntimeConnectionMode::Attached
        }
        (true, None) => DesktopRuntimeConnectionMode::Attached,
        (false, Some(RuntimeOwnerKind::Desktop)) => DesktopRuntimeConnectionMode::Unknown,
        (false, _) => DesktopRuntimeConnectionMode::Stopped,
    }
}

fn runtime_connection_for_stop(state: &DesktopControlState) -> RuntimeConnectionMode {
    match state.connection_mode {
        DesktopRuntimeConnectionMode::DesktopOwned => RuntimeConnectionMode::Owned,
        DesktopRuntimeConnectionMode::Attached => RuntimeConnectionMode::Attached,
        DesktopRuntimeConnectionMode::Stopped | DesktopRuntimeConnectionMode::Unknown => {
            RuntimeConnectionMode::Stopped
        }
    }
}

pub(crate) fn explicit_stop_action(state: &DesktopControlState) -> RuntimeStopAction {
    decide_runtime_stop_action(
        runtime_connection_for_stop(state),
        RuntimeStopIntent::ExplicitStop,
        state.shutdown_available,
    )
}

async fn shutdown_runtime(action: &str) -> Result<DesktopActionResult, CommandError> {
    let client = admin_client()?;
    let endpoint = admin_endpoint_config();
    post_empty(&client, &endpoint.admin_base_url, RUNTIME_SHUTDOWN_PATH).await?;
    let state = wait_for_runtime_stopped()
        .await
        .unwrap_or(load_control_state().await?);
    Ok(result_with_state(
        action,
        "已发送 runtime shutdown 请求。",
        state,
    ))
}

fn codex_switch_snapshot() -> CodexSwitchSnapshot {
    match codex_integration::codex_switch_status() {
        Ok(status) => CodexSwitchSnapshot::from_status(status),
        Err(err) => CodexSwitchSnapshot {
            enabled: false,
            model_provider: None,
            provider_name: None,
            base_url: None,
            preset: None,
            requires_openai_auth: None,
            supports_websockets: None,
            remote_compaction_v2_enabled: false,
            has_switch_state: false,
            error_message: Some(err.to_string()),
        },
    }
}

impl CodexSwitchSnapshot {
    fn from_status(status: CodexSwitchStatus) -> Self {
        Self {
            enabled: status.enabled,
            model_provider: status.model_provider,
            provider_name: status.provider_name,
            base_url: status.base_url,
            preset: status
                .patch_mode
                .map(|mode| mode.as_preset_str().to_string()),
            requires_openai_auth: status.requires_openai_auth,
            supports_websockets: status.supports_websockets,
            remote_compaction_v2_enabled: status.remote_compaction_v2_enabled,
            has_switch_state: status.has_switch_state,
            error_message: None,
        }
    }
}

fn require_confirmation(actual: &str, expected: &str) -> Result<(), CommandError> {
    if actual.trim() == expected {
        return Ok(());
    }
    Err(DesktopError::Lifecycle(format!("请输入确认短语 {expected}。")).into())
}

async fn wait_for_runtime_reachable() -> Result<DesktopControlState, CommandError> {
    let started = Instant::now();
    loop {
        let state = load_control_state().await?;
        if state.reachable {
            return Ok(state);
        }
        if started.elapsed() >= STARTUP_POLL_TIMEOUT {
            return Err(DesktopError::Lifecycle(
                "已启动进程，但 5 秒内没有连上本地 admin API；请检查日志或端口占用。".to_string(),
            )
            .into());
        }
        tokio::time::sleep(STARTUP_POLL_INTERVAL).await;
    }
}

async fn wait_for_runtime_stopped() -> Result<DesktopControlState, CommandError> {
    let started = Instant::now();
    loop {
        let state = load_control_state().await?;
        if !state.reachable {
            return Ok(state);
        }
        if started.elapsed() >= STARTUP_POLL_TIMEOUT {
            return Ok(state);
        }
        tokio::time::sleep(STARTUP_POLL_INTERVAL).await;
    }
}

fn result_with_state(
    action: impl Into<String>,
    message: impl Into<String>,
    state: DesktopControlState,
) -> DesktopActionResult {
    DesktopActionResult {
        ok: true,
        action: action.into(),
        message: message.into(),
        state: Some(state),
        payload: None,
    }
}

fn result_with_payload(
    action: impl Into<String>,
    message: impl Into<String>,
    state: DesktopControlState,
    payload: Value,
) -> DesktopActionResult {
    DesktopActionResult {
        ok: true,
        action: action.into(),
        message: message.into(),
        state: Some(state),
        payload: Some(payload),
    }
}

async fn post_empty_with_json<B: Serialize + ?Sized>(
    client: &reqwest::Client,
    base_url: &str,
    path: &str,
    body: &B,
) -> Result<(), CommandError> {
    post_json_no_response(client, base_url, path, body).await
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn provider_balance_refresh_path(payload: ProviderBalanceRefreshPayload) -> String {
    let mut url = reqwest::Url::parse("http://localhost")
        .expect("static localhost URL should parse for query encoding");
    url.set_path(PROVIDERS_BALANCES_REFRESH_PATH);
    {
        let mut query = url.query_pairs_mut();
        if let Some(station_name) = clean_optional(payload.station_name) {
            query.append_pair("station_name", station_name.as_str());
        }
        if let Some(provider_id) = clean_optional(payload.provider_id) {
            query.append_pair("provider_id", provider_id.as_str());
        }
    }
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
}

fn resolve_cli_path(app: &AppHandle) -> Result<PathBuf, CommandError> {
    let resource_dir = app.path().resource_dir().ok();
    let current = std::env::current_exe()
        .map_err(|err| DesktopError::Lifecycle(format!("无法定位当前桌面进程路径: {err}")))?;
    resolve_cli_path_from_sources(resource_dir.as_deref(), &current, env_cli_path())
}

fn resolve_cli_path_from_sources(
    resource_dir: Option<&Path>,
    current: &Path,
    env_path: Option<PathBuf>,
) -> Result<PathBuf, CommandError> {
    if let Some(resource_dir) = resource_dir {
        for candidate in cli_candidates_from_resource_dir(resource_dir) {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    for candidate in cli_candidates_from_current_exe(&current) {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    if let Some(path) = env_path {
        return ensure_executable_path(path, "CODEX_HELPER_CLI_PATH");
    }

    Err(DesktopError::Lifecycle(
        "未找到 codex-helper CLI sidecar；请运行 pnpm tauri:build 生成安装包，或在开发环境设置 CODEX_HELPER_CLI_PATH。".to_string(),
    )
    .into())
}

fn env_cli_path() -> Option<PathBuf> {
    std::env::var(CLI_PATH_ENV)
        .ok()
        .or_else(|| std::env::var(CLI_PATH_ENV_LEGACY).ok())
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn ensure_executable_path(path: PathBuf, source: &str) -> Result<PathBuf, CommandError> {
    if path.is_file() {
        return Ok(path);
    }
    Err(DesktopError::Lifecycle(format!(
        "{source} 指向的 codex-helper CLI 不存在: {}",
        path.display()
    ))
    .into())
}

fn cli_candidates_from_current_exe(current: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = current.parent() {
        candidates.push(dir.join(cli_executable_name()));
        if dir.file_name().is_some_and(|name| name == "deps") {
            if let Some(parent) = dir.parent() {
                candidates.push(parent.join(cli_executable_name()));
            }
        }
    }
    candidates
}

fn cli_candidates_from_resource_dir(resource_dir: &Path) -> Vec<PathBuf> {
    vec![resource_dir.join(cli_executable_name())]
}

fn cli_executable_name() -> &'static str {
    if cfg!(windows) {
        "codex-helper.exe"
    } else {
        "codex-helper"
    }
}

#[cfg(test)]
mod tests {
    use super::super::admin_api::DEFAULT_PROXY_PORT;
    use super::*;

    #[test]
    fn classifies_desktop_owned_runtime_from_owner_marker() {
        let marker = RuntimeOwnerMarker::new_with_pid(
            RuntimeOwnerKind::Desktop,
            SERVICE_NAME,
            DEFAULT_PROXY_PORT,
            7,
            1,
        );

        assert_eq!(
            classify_connection_mode(true, Some(&marker)),
            DesktopRuntimeConnectionMode::DesktopOwned
        );
    }

    #[test]
    fn reachable_runtime_without_marker_is_attached() {
        assert_eq!(
            classify_connection_mode(true, None),
            DesktopRuntimeConnectionMode::Attached
        );
    }

    #[test]
    fn explicit_confirmations_are_exact_and_action_specific() {
        assert!(require_confirmation("STOP OWNED PROXY", OWNED_STOP_CONFIRMATION).is_ok());
        assert!(require_confirmation("STOP ATTACHED PROXY", ATTACHED_STOP_CONFIRMATION).is_ok());
        assert!(require_confirmation("STOP PROXY", OWNED_STOP_CONFIRMATION).is_err());
        assert!(require_confirmation("STOP OWNED PROXY", ATTACHED_STOP_CONFIRMATION).is_err());
    }

    #[test]
    fn cli_candidates_use_sibling_binary() {
        let current = if cfg!(windows) {
            PathBuf::from(r"C:\repo\target\debug\codex-helper-desktop.exe")
        } else {
            PathBuf::from("/repo/target/debug/codex-helper-desktop")
        };
        let candidates = cli_candidates_from_current_exe(&current);
        assert!(candidates.iter().any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("codex-helper"))
        }));
    }

    #[test]
    fn cli_resolution_prefers_packaged_sidecar_over_env_override() {
        let root = unique_temp_dir("packaged-sidecar-order");
        let resource_dir = root.join("resources");
        let env_dir = root.join("env");
        std::fs::create_dir_all(&resource_dir).expect("create resource dir");
        std::fs::create_dir_all(&env_dir).expect("create env dir");

        let packaged_cli = resource_dir.join(cli_executable_name());
        let env_cli = env_dir.join(cli_executable_name());
        std::fs::write(&packaged_cli, b"packaged").expect("write packaged sidecar");
        std::fs::write(&env_cli, b"env").expect("write env sidecar");

        let current = root.join("codex-helper-desktop.exe");
        let resolved = resolve_cli_path_from_sources(Some(&resource_dir), &current, Some(env_cli))
            .expect("resolve packaged sidecar");

        assert_eq!(resolved, packaged_cli);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cli_resolution_uses_env_only_after_packaged_and_sibling_candidates() {
        let root = unique_temp_dir("env-sidecar-fallback");
        let resource_dir = root.join("resources");
        let env_dir = root.join("env");
        std::fs::create_dir_all(&resource_dir).expect("create resource dir");
        std::fs::create_dir_all(&env_dir).expect("create env dir");

        let env_cli = env_dir.join(cli_executable_name());
        std::fs::write(&env_cli, b"env").expect("write env sidecar");

        let current = root.join("codex-helper-desktop.exe");
        let resolved =
            resolve_cli_path_from_sources(Some(&resource_dir), &current, Some(env_cli.clone()))
                .expect("resolve env fallback");

        assert_eq!(resolved, env_cli);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_stop_distinguishes_owned_and_attached_runtime() {
        let base = DesktopControlState {
            connection_mode: DesktopRuntimeConnectionMode::Attached,
            proxy_port: DEFAULT_PROXY_PORT,
            admin_port: DEFAULT_PROXY_PORT + 1000,
            proxy_base_url: "http://127.0.0.1:3211".to_string(),
            admin_base_url: "http://127.0.0.1:4211".to_string(),
            reachable: true,
            shutdown_available: true,
            owner: None,
            codex_switch: CodexSwitchSnapshot {
                enabled: false,
                model_provider: None,
                provider_name: None,
                base_url: None,
                preset: None,
                requires_openai_auth: None,
                supports_websockets: None,
                remote_compaction_v2_enabled: false,
                has_switch_state: false,
                error_message: None,
            },
            can_start: false,
            can_attach: true,
            can_stop_owned: false,
            can_remote_stop: true,
            can_switch_on: true,
            can_switch_off: true,
        };

        assert_eq!(
            explicit_stop_action(&base),
            RuntimeStopAction::ShutdownAttachedRuntime
        );

        let owned = DesktopControlState {
            connection_mode: DesktopRuntimeConnectionMode::DesktopOwned,
            can_attach: false,
            can_stop_owned: true,
            can_remote_stop: false,
            ..base
        };
        assert_eq!(
            explicit_stop_action(&owned),
            RuntimeStopAction::StopOwnedRuntime
        );
    }

    #[test]
    fn provider_balance_refresh_path_encodes_optional_filters() {
        let path = provider_balance_refresh_path(ProviderBalanceRefreshPayload {
            station_name: Some("route alpha".to_string()),
            provider_id: Some("provider/one".to_string()),
        });

        assert_eq!(
            path,
            "/__codex_helper/api/v1/providers/balances/refresh?station_name=route+alpha&provider_id=provider%2Fone"
        );
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let unique = format!(
            "codex-helper-desktop-{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time after epoch")
                .as_nanos()
        );
        dir.push(unique);
        dir
    }
}
