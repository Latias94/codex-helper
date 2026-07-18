use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::config::{NotifyConfig, NotifyPolicyConfig, load_config, proxy_home_dir};
use crate::dashboard_core::OperatorRequestSummary;
use crate::file_replace::write_bytes_file_async;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum CodexNotificationType {
    AgentTurnComplete,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
struct CodexNotificationInput {
    r#type: CodexNotificationType,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    input_messages: Option<Vec<String>>,
    #[serde(default)]
    last_assistant_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct QueuedEvent {
    thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    duration_ms: u64,
    ended_at_ms: u64,
    queued_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_assistant_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NotifyState {
    version: u32,
    #[serde(default)]
    pending: Vec<QueuedEvent>,
    #[serde(default)]
    last_toast_ms: Option<u64>,
    #[serde(default)]
    per_thread_last_toast_ms: HashMap<String, u64>,
    #[serde(default)]
    suppressed_since_last_toast: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn read_payload(notification_json: Option<String>) -> std::io::Result<Option<String>> {
    if let Some(s) = notification_json {
        return Ok(Some(s));
    }

    if atty::is(atty::Stream::Stdin) {
        return Ok(None);
    }

    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    let buf = buf.trim().to_string();
    if buf.is_empty() {
        Ok(None)
    } else {
        Ok(Some(buf))
    }
}

fn shorten(input: &str, max_chars: usize) -> String {
    let s = input.trim();
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push_str("...");
    out
}

fn notify_state_path() -> PathBuf {
    proxy_home_dir().join("notify_state.json")
}

fn notify_lock_path() -> PathBuf {
    proxy_home_dir().join("notify_state.lock")
}

fn codex_proxy_base_url_from_codex_config_text(text: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(text).ok()?;
    let table = value.as_table()?;
    let providers = table.get("model_providers")?.as_table()?;
    let proxy = providers.get("codex_proxy")?.as_table()?;
    proxy
        .get("base_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

async fn get_proxy_base_url() -> Option<String> {
    if let Ok(v) = std::env::var("CODEX_HELPER_NOTIFY_PROXY_BASE_URL")
        && !v.trim().is_empty()
    {
        return Some(v);
    }

    let codex_cfg_path = crate::config::codex_config_path();
    let text = tokio::fs::read_to_string(codex_cfg_path).await.ok()?;
    codex_proxy_base_url_from_codex_config_text(&text)
}

fn pick_best_recent_request(
    thread_id: &str,
    now_ms: u64,
    policy: &NotifyPolicyConfig,
    recent: &[OperatorRequestSummary],
) -> Option<OperatorRequestSummary> {
    let min_ended_at = now_ms.saturating_sub(policy.recent_search_window_ms);
    let session_key = crate::dashboard_core::operator_summary::operator_session_key(thread_id);

    recent
        .iter()
        .filter(|r| r.service == "codex")
        .filter(|r| r.ended_at_ms >= min_ended_at)
        .filter(|r| r.session_key.as_deref() == Some(session_key.as_str()))
        .cloned()
        .max_by_key(|r| (request_path_score(&r.path), r.ended_at_ms))
}

fn request_path_score(path: &str) -> u8 {
    let p = path.to_ascii_lowercase();
    if p.contains("responses") {
        2
    } else if p.contains("chat") {
        1
    } else {
        0
    }
}

async fn fetch_recent_finished(
    proxy_base_url: &str,
    timeout_ms: u64,
) -> anyhow::Result<Vec<OperatorRequestSummary>> {
    let admin_base_url = notify_admin_base_url(proxy_base_url)?;
    let endpoint = crate::control_plane_client::ControlPlaneEndpoint::new(
        admin_base_url,
        crate::control_plane_client::configured_local_admin_token_env(),
    )?;
    let client = crate::control_plane_client::ControlPlaneClient::new_with_timeout(
        endpoint,
        Duration::from_millis(timeout_ms),
    )?;
    let model = client.operator_read_model().await?;
    model
        .data
        .map(|data| data.recent_requests)
        .ok_or_else(|| anyhow::anyhow!("operator read model has no ready data"))
}

fn notify_admin_base_url(proxy_base_url: &str) -> anyhow::Result<String> {
    let proxy_base_url = crate::control_plane_client::normalize_base_url(proxy_base_url)
        .ok_or_else(|| anyhow::anyhow!("notify proxy base URL is invalid"))?;
    let admin_base_url = crate::proxy::admin_base_url_from_proxy_base_url(&proxy_base_url)
        .ok_or_else(|| anyhow::anyhow!("notify proxy base URL has no derivable admin origin"))?;
    if !crate::control_plane_client::is_loopback_control_plane_base_url(&admin_base_url) {
        anyhow::bail!("notify accepts only a loopback proxy origin");
    }
    Ok(admin_base_url)
}

async fn queue_event_and_spawn_flush(
    cfg: &NotifyConfig,
    event: QueuedEvent,
    force_toast: bool,
) -> anyhow::Result<()> {
    let _lock = acquire_notify_lock().await?;

    let mut state = load_state().await.unwrap_or_default();
    if state.version == 0 {
        state.version = 1;
    }

    // Drop very old pending items to avoid unbounded growth.
    let cutoff = now_ms().saturating_sub(30 * 60_000);
    state.pending.retain(|e| e.queued_at_ms >= cutoff);
    state.pending.push(event);
    save_state(&state).await?;

    if cfg.enabled && (cfg.system.enabled || (cfg.exec.enabled && !cfg.exec.command.is_empty())) {
        spawn_flush_process(force_toast)?;
    }
    Ok(())
}

pub async fn handle_codex_notify(
    notification_json: Option<String>,
    no_toast: bool,
    force_toast: bool,
) -> anyhow::Result<()> {
    let Some(payload) = read_payload(notification_json)? else {
        return Ok(());
    };

    let cfg = load_config().await?;
    let notify_cfg = cfg.notify;
    let system_enabled =
        force_toast || (notify_cfg.enabled && notify_cfg.system.enabled && !no_toast);
    let exec_enabled =
        notify_cfg.enabled && notify_cfg.exec.enabled && !notify_cfg.exec.command.is_empty();

    if !system_enabled && !exec_enabled {
        return Ok(());
    }

    let payload: CodexNotificationInput = match serde_json::from_str(&payload) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("codex-helper notify: failed to parse notification JSON: {err}");
            return Ok(());
        }
    };

    if payload.r#type != CodexNotificationType::AgentTurnComplete {
        return Ok(());
    }

    let Some(thread_id) = payload
        .thread_id
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    else {
        return Ok(());
    };

    let proxy_base_url = match get_proxy_base_url().await {
        Some(v) => v,
        None => {
            // Without proxy access we cannot compute duration_ms reliably, so skip (D strategy).
            return Ok(());
        }
    };

    let recent = match fetch_recent_finished(
        &proxy_base_url,
        notify_cfg.policy.recent_endpoint_timeout_ms,
    )
    .await
    {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let now = now_ms();
    let best = pick_best_recent_request(thread_id, now, &notify_cfg.policy, &recent);
    let Some(best) = best else {
        return Ok(());
    };

    if best.duration_ms < notify_cfg.policy.min_duration_ms {
        return Ok(());
    }

    let preview = payload
        .last_assistant_message
        .as_deref()
        .map(|s| shorten(s, 160))
        .filter(|s| !s.trim().is_empty());

    let event = QueuedEvent {
        thread_id: thread_id.to_string(),
        turn_id: payload.turn_id.clone(),
        cwd: payload.cwd.clone(),
        duration_ms: best.duration_ms,
        ended_at_ms: best.ended_at_ms,
        queued_at_ms: now,
        last_assistant_preview: preview,
    };

    // If user forces toast for this invocation, we still rely on config for policy.
    // We reuse cfg.notify for queue/flush; system notifications can be enabled only for this run.
    let mut cfg_for_queue = notify_cfg.clone();
    if force_toast {
        cfg_for_queue.enabled = true;
        cfg_for_queue.system.enabled = true;
    }
    if no_toast {
        cfg_for_queue.system.enabled = false;
    }

    queue_event_and_spawn_flush(&cfg_for_queue, event, force_toast).await
}

pub async fn handle_codex_flush() -> anyhow::Result<()> {
    let cfg = load_config().await?;
    let notify_cfg = cfg.notify;
    let force_toast = matches!(
        std::env::var("CODEX_HELPER_NOTIFY_FORCE_TOAST"),
        Ok(v) if v == "1" || v.eq_ignore_ascii_case("true")
    );

    if !notify_cfg.enabled && !force_toast {
        return Ok(());
    }

    for _ in 0..20 {
        let _lock = acquire_notify_lock().await?;
        let mut state = load_state().await.unwrap_or_default();
        if state.pending.is_empty() {
            return Ok(());
        }

        let now = now_ms();
        let first_pending = state
            .pending
            .iter()
            .map(|e| e.queued_at_ms)
            .min()
            .unwrap_or(now);
        let due_ms = first_pending.saturating_add(notify_cfg.policy.merge_window_ms);

        if now < due_ms {
            drop(state);
            sleep(Duration::from_millis((due_ms - now).min(60_000))).await;
            continue;
        }

        if let Some(last) = state.last_toast_ms
            && now.saturating_sub(last) < notify_cfg.policy.global_cooldown_ms
        {
            let wait = notify_cfg.policy.global_cooldown_ms - now.saturating_sub(last);
            drop(state);
            sleep(Duration::from_millis(wait.min(60_000))).await;
            continue;
        }

        // Apply per-thread cooldown and prepare toast batch.
        state.pending.sort_by_key(|e| e.ended_at_ms);
        let mut send: Vec<QueuedEvent> = Vec::new();
        let mut suppressed = 0u64;
        for e in state.pending.iter() {
            let last = state.per_thread_last_toast_ms.get(&e.thread_id).copied();
            if let Some(last) = last
                && now.saturating_sub(last) < notify_cfg.policy.per_thread_cooldown_ms
            {
                suppressed = suppressed.saturating_add(1);
                continue;
            }
            send.push(e.clone());
        }

        if send.is_empty() {
            state.pending.clear();
            state.suppressed_since_last_toast =
                state.suppressed_since_last_toast.saturating_add(suppressed);
            save_state(&state).await?;
            return Ok(());
        }

        let system_enabled = notify_cfg.system.enabled || force_toast;
        let exec_enabled = notify_cfg.exec.enabled && !notify_cfg.exec.command.is_empty();
        if !system_enabled && !exec_enabled {
            state.pending.clear();
            save_state(&state).await?;
            return Ok(());
        }

        let title = render_title(send.len(), suppressed, state.suppressed_since_last_toast);
        let body = render_body(&send);
        let aggregated = serde_json::json!({
            "type": "codex-helper-merged-agent-turn-complete",
            "count": send.len(),
            "suppressed_in_batch": suppressed,
            "suppressed_since_last_toast": state.suppressed_since_last_toast,
            "generated_at_ms": now,
            "events": send,
        })
        .to_string();

        if system_enabled && let Err(err) = send_system_notification(&title, &body) {
            eprintln!("codex-helper notify: failed to show system notification: {err}");
        }
        if exec_enabled && let Err(err) = run_exec_callback(&notify_cfg.exec.command, &aggregated) {
            eprintln!("codex-helper notify: exec callback failed: {err}");
        }

        state.last_toast_ms = Some(now);
        for e in send.iter() {
            state
                .per_thread_last_toast_ms
                .insert(e.thread_id.clone(), now);
        }
        state.suppressed_since_last_toast = 0;
        state.pending.clear();
        save_state(&state).await?;
        return Ok(());
    }

    Ok(())
}

fn render_title(count: usize, suppressed_in_batch: u64, suppressed_since_last: u64) -> String {
    let mut title = if count == 1 {
        "Codex: turn complete".to_string()
    } else {
        format!("Codex: {count} turns complete")
    };
    let total_suppressed = suppressed_in_batch.saturating_add(suppressed_since_last);
    if total_suppressed > 0 {
        title.push_str(&format!(" (+{total_suppressed} suppressed)"));
    }
    title
}

fn render_body(events: &[QueuedEvent]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for e in events.iter().rev().take(3) {
        let dur_s = (e.duration_ms as f64 / 1000.0).max(0.0);
        let cwd = e
            .cwd
            .as_deref()
            .and_then(|p| Path::new(p).file_name().and_then(|s| s.to_str()))
            .unwrap_or("-");
        if let Some(preview) = e.last_assistant_preview.as_deref() {
            lines.push(format!("{cwd} ({dur_s:.1}s): {}", shorten(preview, 90)));
        } else {
            lines.push(format!("{cwd} ({dur_s:.1}s)"));
        }
    }
    if events.len() > 3 {
        lines.push(format!("+{} more", events.len() - 3));
    }
    lines.join("\n")
}

fn run_exec_callback(command: &[String], input_json: &str) -> anyhow::Result<()> {
    if command.is_empty() {
        return Ok(());
    }
    let mut cmd = Command::new(&command[0]);
    if command.len() > 1 {
        cmd.args(&command[1..]);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(input_json.as_bytes())?;
    }
    let _ = child.wait();
    Ok(())
}

fn spawn_flush_process(force_toast: bool) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("notify").arg("flush-codex");
    if force_toast {
        cmd.env("CODEX_HELPER_NOTIFY_FORCE_TOAST", "1");
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    let _ = cmd.spawn()?;
    Ok(())
}

async fn load_state() -> anyhow::Result<NotifyState> {
    let path = notify_state_path();
    if !path.exists() {
        return Ok(NotifyState {
            version: 1,
            ..Default::default()
        });
    }
    let bytes = tokio::fs::read(path).await?;
    let mut state = serde_json::from_slice::<NotifyState>(&bytes)?;
    if state.version == 0 {
        state.version = 1;
    }
    Ok(state)
}

async fn save_state(state: &NotifyState) -> anyhow::Result<()> {
    let dir = proxy_home_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let path = notify_state_path();
    let data = serde_json::to_vec_pretty(state)?;
    write_bytes_file_async(&path, &data).await?;
    Ok(())
}

struct NotifyLockGuard {
    path: PathBuf,
}

impl Drop for NotifyLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn acquire_notify_lock() -> anyhow::Result<NotifyLockGuard> {
    let path = notify_lock_path();
    let dir = proxy_home_dir();
    tokio::fs::create_dir_all(&dir).await?;

    for _ in 0..200 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                use std::io::Write;
                let _ = writeln!(f, "{}", now_ms());
                return Ok(NotifyLockGuard { path });
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // Best-effort stale lock cleanup (2 minutes).
                if let Ok(meta) = std::fs::metadata(&path)
                    && let Ok(modified) = meta.modified()
                    && let Ok(age) = SystemTime::now().duration_since(modified)
                    && age > Duration::from_secs(120)
                {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                sleep(Duration::from_millis(10)).await;
            }
            Err(err) => return Err(err.into()),
        }
    }

    anyhow::bail!("failed to acquire notify lock: {:?}", path);
}

fn send_system_notification(title: &str, body: &str) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        windows_toast::notify(title, body)?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        macos_notification::notify(title, body)?;
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        // No-op fallback: print a short line for non-supported platforms.
        println!("{title}: {body}");
        Ok(())
    }
}

#[cfg(windows)]
mod windows_toast {
    use std::io;
    use std::process::{Command, Stdio};

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;

    const APP_ID: &str = "codex-helper";
    const POWERSHELL_EXE: &str = "powershell.exe";

    pub fn notify(title: &str, body: &str) -> io::Result<()> {
        let encoded_title = encode_argument(title);
        let encoded_body = encode_argument(body);
        let encoded_command = build_encoded_command(&encoded_title, &encoded_body);

        let mut command = Command::new(POWERSHELL_EXE);
        command
            .arg("-NoProfile")
            .arg("-NoLogo")
            .arg("-EncodedCommand")
            .arg(encoded_command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let status = command.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "{POWERSHELL_EXE} exited with status {status}"
            )))
        }
    }

    fn build_encoded_command(encoded_title: &str, encoded_body: &str) -> String {
        let script = build_ps_script(encoded_title, encoded_body);
        encode_script_for_powershell(&script)
    }

    fn build_ps_script(encoded_title: &str, encoded_body: &str) -> String {
        format!(
            r#"
$encoding = [System.Text.Encoding]::UTF8
$titleText = $encoding.GetString([System.Convert]::FromBase64String("{encoded_title}"))
$bodyText = $encoding.GetString([System.Convert]::FromBase64String("{encoded_body}"))
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
$doc = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$textNodes = $doc.GetElementsByTagName("text")
$textNodes.Item(0).AppendChild($doc.CreateTextNode($titleText)) | Out-Null
$textNodes.Item(1).AppendChild($doc.CreateTextNode($bodyText)) | Out-Null
$toast = [Windows.UI.Notifications.ToastNotification]::new($doc)
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{app_id}').Show($toast)
"#,
            app_id = APP_ID
        )
    }

    fn encode_script_for_powershell(script: &str) -> String {
        let mut wide: Vec<u8> = Vec::with_capacity((script.len() + 1) * 2);
        for unit in script.encode_utf16() {
            wide.extend_from_slice(&unit.to_le_bytes());
        }
        BASE64.encode(wide)
    }

    fn encode_argument(value: &str) -> String {
        BASE64.encode(escape_for_xml(value))
    }

    fn escape_for_xml(input: &str) -> String {
        let mut escaped = String::with_capacity(input.len());
        for ch in input.chars() {
            match ch {
                '&' => escaped.push_str("&amp;"),
                '<' => escaped.push_str("&lt;"),
                '>' => escaped.push_str("&gt;"),
                '"' => escaped.push_str("&quot;"),
                '\'' => escaped.push_str("&apos;"),
                _ => escaped.push(ch),
            }
        }
        escaped
    }

    #[cfg(test)]
    mod tests {
        use super::escape_for_xml;

        #[test]
        fn escapes_xml_entities() {
            assert_eq!(escape_for_xml("a & b"), "a &amp; b");
            assert_eq!(escape_for_xml("5 > 3"), "5 &gt; 3");
            assert_eq!(escape_for_xml("<tag>"), "&lt;tag&gt;");
            assert_eq!(escape_for_xml("\"quoted\""), "&quot;quoted&quot;");
            assert_eq!(escape_for_xml("single 'quote'"), "single &apos;quote&apos;");
        }
    }
}

#[cfg(target_os = "macos")]
mod macos_notification {
    use std::io;
    use std::process::{Command, Stdio};

    pub fn notify(title: &str, body: &str) -> io::Result<()> {
        let script = format!(
            "display notification {} with title {}",
            apple_quote(body),
            apple_quote(title)
        );
        let status = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "osascript exited with status {status}"
            )))
        }
    }

    fn apple_quote(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('\"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::response::Redirect;
    use axum::routing::get;
    use tokio::net::TcpListener;

    use super::*;

    const ADMIN_DISCOVERY_PATH: &str = "/.well-known/codex-helper-admin";
    const OPERATOR_READ_MODEL_PATH: &str = "/__codex_helper/api/v1/operator/read-model";
    const NOTIFY_TEST_PROXY_URL_ENV: &str = "CODEX_HELPER_TEST_NOTIFY_PROXY_URL";
    const NOTIFY_TEST_EXPECT_SUCCESS_ENV: &str = "CODEX_HELPER_TEST_NOTIFY_EXPECT_SUCCESS";
    const NOTIFY_TEST_EXPECT_PATH_ENV: &str = "CODEX_HELPER_TEST_NOTIFY_EXPECT_PATH";
    const NOTIFY_TEST_TOKEN: &str = "notify-admin-token";

    async fn spawn_server(app: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind server");
        spawn_server_on(listener, app)
    }

    fn spawn_server_on(
        listener: TcpListener,
        app: Router,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let addr = listener.local_addr().expect("server address");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test app");
        });
        (addr, handle)
    }

    async fn bind_proxy_admin_pair() -> (TcpListener, TcpListener) {
        for _ in 0..128 {
            let proxy = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind proxy candidate");
            let proxy_port = proxy.local_addr().expect("proxy address").port();
            let admin_port = crate::proxy::admin_port_for_proxy_port(proxy_port);
            if let Ok(admin) = TcpListener::bind(("127.0.0.1", admin_port)).await {
                return (proxy, admin);
            }
        }
        panic!("failed to bind paired proxy and admin test listeners");
    }

    async fn spawn_connection_counter(
        bind_addr: SocketAddr,
    ) -> (SocketAddr, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind(bind_addr)
            .await
            .expect("bind connection counter");
        let addr = listener.local_addr().expect("connection counter address");
        let hits = Arc::new(AtomicUsize::new(0));
        let recorded_hits = hits.clone();
        let handle = tokio::spawn(async move {
            while let Ok((_stream, _peer)) = listener.accept().await {
                recorded_hits.fetch_add(1, Ordering::SeqCst);
            }
        });
        (addr, hits, handle)
    }

    fn recent_router(path: &'static str, tokens: Arc<Mutex<Vec<String>>>) -> Router {
        let response = notify_operator_read_model(path);
        Router::new().route(
            OPERATOR_READ_MODEL_PATH,
            get(move |headers: axum::http::HeaderMap| {
                let tokens = tokens.clone();
                let response = response.clone();
                async move {
                    if let Some(token) = headers
                        .get(crate::proxy::ADMIN_TOKEN_HEADER)
                        .and_then(|value| value.to_str().ok())
                    {
                        tokens
                            .lock()
                            .expect("recent token lock")
                            .push(token.to_string());
                    }
                    axum::Json(response)
                }
            }),
        )
    }

    fn notify_request_summary(
        thread_id: &str,
        path: &str,
        duration_ms: u64,
        ended_at_ms: u64,
    ) -> OperatorRequestSummary {
        OperatorRequestSummary {
            id: 1,
            session_key: Some(
                crate::dashboard_core::operator_summary::operator_session_key(thread_id),
            ),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            route_path: Vec::new(),
            upstream_origin: None,
            usage: None,
            cost: Default::default(),
            retry: None,
            provider_signal_codes: Vec::new(),
            policy_action_codes: Vec::new(),
            observability: crate::dashboard_core::OperatorRequestObservability {
                duration_ms: Some(duration_ms),
                ttfb_ms: None,
                generation_ms: None,
                output_tokens_per_second: None,
                attempt_count: 1,
                route_attempt_count: 1,
                retried: false,
                cross_provider_failover: false,
                same_provider_retry: false,
                fast_mode: false,
                streaming: false,
            },
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: path.to_string(),
            status_code: 200,
            duration_ms,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms,
        }
    }

    fn notify_operator_read_model(path: &str) -> crate::dashboard_core::OperatorReadModel {
        use crate::dashboard_core::{
            ApiV1OperatorSummary, OperatorReadData, OperatorReadModel, OperatorRevisionBundle,
        };

        OperatorReadModel::ready(
            "codex",
            42,
            OperatorRevisionBundle {
                runtime_revision: 1,
                runtime_digest: "runtime-1".to_string(),
                route_digest: "route-1".to_string(),
                catalog_revision: "catalog-1".to_string(),
                pricing_revision: "pricing-1".to_string(),
                operator_pricing_revision: "operator-pricing-1".to_string(),
                policy_revision: 1,
                ledger_revision: "operator-ledger-v1:test-store:1".to_string(),
            },
            OperatorReadData {
                summary: ApiV1OperatorSummary {
                    api_version: 1,
                    service_name: "codex".to_string(),
                    runtime: Default::default(),
                    counts: Default::default(),
                    retry: Default::default(),
                    credential_readiness: None,
                    sessions: Vec::new(),
                    profiles: Vec::new(),
                    providers: Vec::new(),
                },
                routing: None,
                active_requests: Vec::new(),
                recent_requests: vec![notify_request_summary("thread-1", path, 1_200, 42)],
                usage_summaries: Vec::new(),
                usage_day: Default::default(),
                usage_rollup: Default::default(),
                quota_analytics: Default::default(),
                stats_5m: Default::default(),
                stats_1h: Default::default(),
                pricing_catalog: Default::default(),
                provider_balances: Vec::new(),
            },
        )
    }

    async fn run_notify_fetch_subprocess(
        proxy_url: String,
        expected_path: Option<&str>,
    ) -> std::process::Output {
        let test_exe = std::env::current_exe().expect("current test executable");
        let expect_success = if expected_path.is_some() { "1" } else { "0" };
        let expected_path = expected_path.unwrap_or_default().to_string();
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(test_exe)
                .args([
                    "--exact",
                    "notify::tests::notify_fetch_recent_finished_subprocess",
                    "--ignored",
                    "--nocapture",
                ])
                .env(NOTIFY_TEST_PROXY_URL_ENV, proxy_url)
                .env(NOTIFY_TEST_EXPECT_SUCCESS_ENV, expect_success)
                .env(NOTIFY_TEST_EXPECT_PATH_ENV, expected_path)
                .env(crate::proxy::ADMIN_TOKEN_ENV_VAR, NOTIFY_TEST_TOKEN)
                .output()
        })
        .await
        .expect("join notify subprocess")
        .expect("run notify subprocess")
    }

    fn assert_subprocess_success(output: &std::process::Output) {
        assert!(
            output.status.success(),
            "notify subprocess failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn parses_agent_turn_complete_payload_with_thread_id() {
        let payload = r#"{
            "type": "agent-turn-complete",
            "thread-id": "th1",
            "turn-id": "t1",
            "cwd": "/tmp/x",
            "input-messages": ["run tests"],
            "last-assistant-message": "ok"
        }"#;
        let parsed: CodexNotificationInput = serde_json::from_str(payload).expect("parse");
        assert_eq!(parsed.r#type, CodexNotificationType::AgentTurnComplete);
        assert_eq!(parsed.thread_id.as_deref(), Some("th1"));
        assert_eq!(parsed.turn_id.as_deref(), Some("t1"));
    }

    #[test]
    fn picks_best_recent_request_prefers_responses_path() {
        let policy = NotifyPolicyConfig::default();
        let now = 1_000_000u64;
        let recent = vec![
            notify_request_summary("th1", "/v1/chat/completions", 10_000, now - 1_000),
            notify_request_summary("th1", "/v1/responses", 20_000, now - 10_000),
        ];
        let best = pick_best_recent_request("th1", now, &policy, &recent).expect("best");
        assert_eq!(best.path, "/v1/responses");
    }

    #[test]
    fn notify_admin_origin_is_derived_from_loopback_proxy_origin() {
        assert_eq!(
            notify_admin_base_url("http://127.0.0.1:3211/v1")
                .expect("derive loopback admin origin"),
            "http://127.0.0.1:4211"
        );
        assert_eq!(
            notify_admin_base_url("http://localhost:3211/backend-api/codex")
                .expect("derive localhost admin origin"),
            "http://localhost:4211"
        );
    }

    #[test]
    fn notify_admin_origin_rejects_non_loopback_proxy_before_connecting() {
        for proxy_url in [
            "http://192.0.2.10:3211/v1",
            "https://relay.example:3211/v1",
            "http://user:secret@127.0.0.1:3211/v1",
            "http://127.0.0.1:3211/v1?token=secret",
        ] {
            assert!(
                notify_admin_base_url(proxy_url).is_err(),
                "unexpectedly trusted {proxy_url}"
            );
        }
    }

    #[tokio::test]
    async fn notify_never_discovers_a_replacement_for_configured_origin() {
        let admin_tokens = Arc::new(Mutex::new(Vec::<String>::new()));
        let (admin_addr, admin_handle) =
            spawn_server(recent_router("/v1/responses-safe", admin_tokens.clone())).await;

        let discovery_hits = Arc::new(AtomicUsize::new(0));
        let discovery_token_hits = Arc::new(AtomicUsize::new(0));
        let hits = discovery_hits.clone();
        let token_hits = discovery_token_hits.clone();
        let admin_url = format!("http://{admin_addr}");
        let proxy = Router::new().route(
            ADMIN_DISCOVERY_PATH,
            get(move |headers: axum::http::HeaderMap| {
                let hits = hits.clone();
                let token_hits = token_hits.clone();
                let admin_url = admin_url.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    if headers.contains_key(crate::proxy::ADMIN_TOKEN_HEADER) {
                        token_hits.fetch_add(1, Ordering::SeqCst);
                    }
                    axum::Json(serde_json::json!({
                        "api_version": 1,
                        "service_name": "codex",
                        "admin_base_url": admin_url
                    }))
                }
            }),
        );
        let (proxy_addr, proxy_handle) = spawn_server(proxy).await;

        let output = run_notify_fetch_subprocess(format!("http://{proxy_addr}"), None).await;

        assert_subprocess_success(&output);
        assert_eq!(discovery_hits.load(Ordering::SeqCst), 0);
        assert_eq!(discovery_token_hits.load(Ordering::SeqCst), 0);
        assert!(admin_tokens.lock().expect("admin token lock").is_empty());
        proxy_handle.abort();
        admin_handle.abort();
    }

    #[tokio::test]
    async fn notify_configured_admin_origin_is_not_replaced_by_discovery() {
        let (proxy_listener, admin_listener) = bind_proxy_admin_pair().await;
        let configured_tokens = Arc::new(Mutex::new(Vec::<String>::new()));
        let (configured_addr, configured_handle) = spawn_server_on(
            admin_listener,
            recent_router("/v1/responses-configured", configured_tokens.clone()),
        );

        let discovered_tokens = Arc::new(Mutex::new(Vec::<String>::new()));
        let (discovered_addr, discovered_handle) = spawn_server(recent_router(
            "/v1/responses-discovered",
            discovered_tokens.clone(),
        ))
        .await;
        let discovery_hits = Arc::new(AtomicUsize::new(0));
        let hits = discovery_hits.clone();
        let discovered_url = format!("http://{discovered_addr}");
        let discovery = Router::new().route(
            ADMIN_DISCOVERY_PATH,
            get(move || {
                let hits = hits.clone();
                let discovered_url = discovered_url.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    axum::Json(serde_json::json!({
                        "api_version": 1,
                        "service_name": "codex",
                        "admin_base_url": discovered_url
                    }))
                }
            }),
        );
        let (proxy_addr, proxy_handle) = spawn_server_on(proxy_listener, discovery);
        assert_eq!(
            configured_addr.port(),
            crate::proxy::admin_port_for_proxy_port(proxy_addr.port())
        );

        let output = run_notify_fetch_subprocess(
            format!("http://{proxy_addr}"),
            Some("/v1/responses-configured"),
        )
        .await;

        assert_subprocess_success(&output);
        assert_eq!(discovery_hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            configured_tokens
                .lock()
                .expect("configured token lock")
                .as_slice(),
            [NOTIFY_TEST_TOKEN]
        );
        assert!(
            discovered_tokens
                .lock()
                .expect("discovered token lock")
                .is_empty()
        );
        proxy_handle.abort();
        configured_handle.abort();
        discovered_handle.abort();
    }

    #[tokio::test]
    async fn notify_configured_origin_bypasses_discovery_redirect() {
        let (proxy_listener, admin_listener) = bind_proxy_admin_pair().await;
        let fallback_tokens = Arc::new(Mutex::new(Vec::<String>::new()));
        let (admin_addr, admin_handle) = spawn_server_on(
            admin_listener,
            recent_router("/v1/responses-redirect-fallback", fallback_tokens.clone()),
        );

        let redirect_hits = Arc::new(AtomicUsize::new(0));
        let redirect_token_hits = Arc::new(AtomicUsize::new(0));
        let hits = redirect_hits.clone();
        let token_hits = redirect_token_hits.clone();
        let redirect_target = Router::new().route(
            ADMIN_DISCOVERY_PATH,
            get(move |headers: axum::http::HeaderMap| {
                let hits = hits.clone();
                let token_hits = token_hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    if headers.contains_key(crate::proxy::ADMIN_TOKEN_HEADER) {
                        token_hits.fetch_add(1, Ordering::SeqCst);
                    }
                    axum::Json(serde_json::json!({ "unexpected": true }))
                }
            }),
        );
        let (redirect_addr, redirect_handle) = spawn_server(redirect_target).await;
        let redirect_url = format!("http://{redirect_addr}{ADMIN_DISCOVERY_PATH}");
        let discovery = Router::new().route(
            ADMIN_DISCOVERY_PATH,
            get(move || {
                let redirect_url = redirect_url.clone();
                async move { Redirect::temporary(&redirect_url) }
            }),
        );
        let (proxy_addr, proxy_handle) = spawn_server_on(proxy_listener, discovery);
        assert_eq!(
            admin_addr.port(),
            crate::proxy::admin_port_for_proxy_port(proxy_addr.port())
        );

        let output = run_notify_fetch_subprocess(
            format!("http://{proxy_addr}"),
            Some("/v1/responses-redirect-fallback"),
        )
        .await;

        assert_subprocess_success(&output);
        assert_eq!(redirect_hits.load(Ordering::SeqCst), 0);
        assert_eq!(redirect_token_hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            fallback_tokens
                .lock()
                .expect("fallback token lock")
                .as_slice(),
            [NOTIFY_TEST_TOKEN]
        );
        proxy_handle.abort();
        admin_handle.abort();
        redirect_handle.abort();
    }

    #[tokio::test]
    async fn notify_configured_origin_bypasses_private_discovery_target() {
        let (proxy_listener, admin_listener) = bind_proxy_admin_pair().await;
        let fallback_tokens = Arc::new(Mutex::new(Vec::<String>::new()));
        let (_admin_addr, admin_handle) = spawn_server_on(
            admin_listener,
            recent_router("/v1/responses-private-fallback", fallback_tokens.clone()),
        );
        let (private_addr, private_hits, private_handle) =
            spawn_connection_counter(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)).await;
        let private_url = format!("https://{private_addr}");
        let discovery = Router::new().route(
            ADMIN_DISCOVERY_PATH,
            get(move || {
                let private_url = private_url.clone();
                async move {
                    axum::Json(serde_json::json!({
                        "api_version": 1,
                        "service_name": "codex",
                        "admin_base_url": private_url
                    }))
                }
            }),
        );
        let (proxy_addr, proxy_handle) = spawn_server_on(proxy_listener, discovery);

        let output = run_notify_fetch_subprocess(
            format!("http://{proxy_addr}"),
            Some("/v1/responses-private-fallback"),
        )
        .await;

        assert_subprocess_success(&output);
        assert_eq!(private_hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            fallback_tokens
                .lock()
                .expect("fallback token lock")
                .as_slice(),
            [NOTIFY_TEST_TOKEN]
        );
        proxy_handle.abort();
        admin_handle.abort();
        private_handle.abort();
    }

    #[tokio::test]
    #[ignore = "subprocess helper for notify discovery tests"]
    async fn notify_fetch_recent_finished_subprocess() {
        let Ok(proxy_url) = std::env::var(NOTIFY_TEST_PROXY_URL_ENV) else {
            return;
        };
        let expect_success =
            std::env::var(NOTIFY_TEST_EXPECT_SUCCESS_ENV).expect("expected result mode") == "1";
        let expected_path = std::env::var(NOTIFY_TEST_EXPECT_PATH_ENV).unwrap_or_default();
        let result = fetch_recent_finished(&proxy_url, 250).await;

        if expect_success {
            let recent = result.expect("notify recent request should succeed");
            assert_eq!(recent.len(), 1);
            assert_eq!(recent[0].path, expected_path);
        } else {
            assert!(
                result.is_err(),
                "untrusted discovery must not create a candidate"
            );
        }
    }
}
