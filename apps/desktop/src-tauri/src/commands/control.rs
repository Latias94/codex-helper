use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use codex_helper_core::codex_switch::{
    self, CodexSwitchIntent, CodexSwitchPhase, CodexSwitchStatus, ValidatedCodexBaseUrl,
};
#[cfg(test)]
use codex_helper_core::runtime_manager::RuntimeOwnerKind;
use codex_helper_core::runtime_manager::{
    ProxyLifecycleMode, RuntimeOwnerMarker, read_owner_marker_best_effort,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::admin_api::{admin_endpoint_config, control_plane_client};
use crate::error::{CommandError, DesktopError};

const SERVICE_NAME: &str = "codex";

pub(crate) const CODEX_SWITCH_ON_CONFIRMATION: &str = "SWITCH CODEX";
pub(crate) const CODEX_SWITCH_OFF_CONFIRMATION: &str = "SWITCH OFF CODEX";

const CLI_PATH_ENV: &str = "CODEX_HELPER_CLI_PATH";
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
    pub owner: Option<RuntimeOwnerMarker>,
    pub codex_switch: CodexSwitchSnapshot,
    pub can_start: bool,
    pub can_attach: bool,
    pub can_switch_on: bool,
    pub can_switch_off: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSwitchSnapshot {
    pub phase: Option<CodexSwitchPhase>,
    pub enabled: bool,
    pub managed: bool,
    pub base_url: Option<String>,
    pub recovery_reason: Option<String>,
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SwitchCodexPayload {
    pub enabled: bool,
    pub confirmation: String,
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

    let endpoint = admin_endpoint_config()?;
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
        message: format!("已启动桌面托管代理进程 pid={child_id}；退出桌面端后代理仍会运行。"),
        state: Some(state),
    })
}

#[tauri::command]
pub async fn switch_codex(
    payload: SwitchCodexPayload,
) -> Result<DesktopActionResult, CommandError> {
    let (action, intent) = if payload.enabled {
        require_confirmation(payload.confirmation.as_str(), CODEX_SWITCH_ON_CONFIRMATION)?;
        let endpoint = admin_endpoint_config()?;
        (
            "switch-codex-on",
            CodexSwitchIntent::On {
                validated_base_url: ValidatedCodexBaseUrl::local(endpoint.proxy_port),
            },
        )
    } else {
        require_confirmation(payload.confirmation.as_str(), CODEX_SWITCH_OFF_CONFIRMATION)?;
        ("switch-codex-off", CodexSwitchIntent::Off)
    };
    let outcome =
        codex_switch::apply(intent).map_err(|err| DesktopError::Switch(err.to_string()))?;
    let message = format!(
        "Codex 本地 switch：{}（phase: {}）。",
        outcome.change.as_str(),
        outcome.status.phase.as_str()
    );
    let state = load_control_state().await?;
    Ok(result_with_state(action, message, state))
}

async fn load_control_state() -> Result<DesktopControlState, CommandError> {
    let endpoint = admin_endpoint_config()?;
    let client = control_plane_client(&endpoint)?;
    let reachable = client.operator_read_model().await.is_ok();
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
        owner,
        codex_switch,
        can_start: !reachable,
        can_attach: reachable && connection_mode != DesktopRuntimeConnectionMode::DesktopOwned,
        can_switch_on: reachable,
        can_switch_off: true,
    })
}

fn classify_connection_mode(
    reachable: bool,
    owner: Option<&RuntimeOwnerMarker>,
) -> DesktopRuntimeConnectionMode {
    match (reachable, owner.map(|marker| marker.owner.lifecycle_mode())) {
        (true, Some(ProxyLifecycleMode::DesktopOwned)) => {
            DesktopRuntimeConnectionMode::DesktopOwned
        }
        (true, _) => DesktopRuntimeConnectionMode::Attached,
        (false, Some(ProxyLifecycleMode::DesktopOwned)) => DesktopRuntimeConnectionMode::Unknown,
        (false, _) => DesktopRuntimeConnectionMode::Stopped,
    }
}

fn codex_switch_snapshot() -> CodexSwitchSnapshot {
    match codex_switch::inspect() {
        Ok(status) => CodexSwitchSnapshot::from_status(status),
        Err(err) => CodexSwitchSnapshot {
            phase: None,
            enabled: false,
            managed: false,
            base_url: None,
            recovery_reason: None,
            error_message: Some(err.to_string()),
        },
    }
}

impl CodexSwitchSnapshot {
    fn from_status(status: CodexSwitchStatus) -> Self {
        Self {
            phase: Some(status.phase),
            enabled: status.enabled,
            managed: status.managed,
            base_url: status.base_url,
            recovery_reason: status.recovery_reason,
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

    for candidate in cli_candidates_from_current_exe(current) {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    if let Some(path) = env_path {
        return ensure_executable_path(path, CLI_PATH_ENV);
    }

    Err(DesktopError::Lifecycle(
        format!(
            "未找到 codex-helper CLI sidecar；请运行 pnpm tauri:build 生成安装包，开发环境仅支持通过 {CLI_PATH_ENV} 指定 CLI 路径。"
        ),
    )
    .into())
}

fn env_cli_path() -> Option<PathBuf> {
    std::env::var_os(CLI_PATH_ENV)
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
        if dir.file_name().is_some_and(|name| name == "deps")
            && let Some(parent) = dir.parent()
        {
            candidates.push(parent.join(cli_executable_name()));
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
    fn reachable_system_service_runtime_is_attached() {
        let marker = RuntimeOwnerMarker::new_with_pid(
            RuntimeOwnerKind::SystemService,
            SERVICE_NAME,
            DEFAULT_PROXY_PORT,
            7,
            1,
        );

        assert_eq!(
            classify_connection_mode(true, Some(&marker)),
            DesktopRuntimeConnectionMode::Attached
        );
    }

    #[test]
    fn switch_confirmations_are_exact_and_action_specific() {
        assert!(require_confirmation("SWITCH CODEX", CODEX_SWITCH_ON_CONFIRMATION).is_ok());
        assert!(require_confirmation("SWITCH OFF CODEX", CODEX_SWITCH_OFF_CONFIRMATION).is_ok());
        assert!(require_confirmation("SWITCH", CODEX_SWITCH_ON_CONFIRMATION).is_err());
        assert!(require_confirmation("SWITCH CODEX", CODEX_SWITCH_OFF_CONFIRMATION).is_err());
    }

    #[test]
    fn switch_payload_rejects_removed_preset_fields() {
        let payload = serde_json::json!({
            "enabled": true,
            "preset": "chatgpt-bridge",
            "confirmation": CODEX_SWITCH_ON_CONFIRMATION,
        });

        assert!(serde_json::from_value::<SwitchCodexPayload>(payload).is_err());
    }

    #[test]
    fn switch_snapshot_serializes_recovery_phase_and_reason_without_presets() {
        let snapshot =
            CodexSwitchSnapshot::from_status(codex_helper_core::codex_switch::CodexSwitchStatus {
                phase: codex_helper_core::codex_switch::CodexSwitchPhase::RecoveryRequired,
                enabled: false,
                managed: true,
                base_url: Some("http://127.0.0.1:3211/v1".to_string()),
                client_facade: Some(codex_helper_core::codex_switch::CodexClientFacade::Compatible),
                recovery_reason: Some("Codex config changed after switch on".to_string()),
                config_path: "/tmp/codex/config.toml".into(),
                state_path: "/tmp/helper/state/codex-switch.json".into(),
            });
        let value = serde_json::to_value(snapshot).expect("serialize Codex switch snapshot");

        assert_eq!(value["phase"], "recovery_required");
        assert_eq!(value["managed"], true);
        assert_eq!(
            value["recoveryReason"],
            "Codex config changed after switch on"
        );
        assert!(value.get("preset").is_none());
        assert!(value.get("requiresOpenaiAuth").is_none());
        assert!(value.get("supportsWebsockets").is_none());
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
    fn cli_resolution_prefers_packaged_sidecar_over_cli_path_override() {
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
    fn cli_resolution_uses_cli_path_override_after_packaged_and_sibling_candidates() {
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
    fn cli_resolution_error_names_the_only_supported_env_override() {
        let root = unique_temp_dir("missing-cli-sidecar");
        let resource_dir = root.join("resources");
        let current = root.join("codex-helper-desktop.exe");

        let error = resolve_cli_path_from_sources(Some(&resource_dir), &current, None)
            .expect_err("missing sidecar should fail");

        assert_eq!(
            error.message,
            format!(
                "desktop lifecycle action failed: 未找到 codex-helper CLI sidecar；请运行 pnpm tauri:build 生成安装包，开发环境仅支持通过 {CLI_PATH_ENV} 指定 CLI 路径。"
            )
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
