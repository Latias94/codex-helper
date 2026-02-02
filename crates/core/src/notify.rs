use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::config::{NotifyConfig, NotifyPolicyConfig, load_config, proxy_home_dir};

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

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct FinishedRequestLite {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    service: String,
    method: String,
    path: String,
    status_code: u16,
    duration_ms: u64,
    ended_at_ms: u64,
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
    let value: toml::Value = text.parse().ok()?;
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
    cwd: Option<&str>,
    now_ms: u64,
    policy: &NotifyPolicyConfig,
    recent: &[FinishedRequestLite],
) -> Option<FinishedRequestLite> {
    let min_ended_at = now_ms.saturating_sub(policy.recent_search_window_ms);

    let mut candidates = recent
        .iter()
        .filter(|r| r.service == "codex")
        .filter(|r| r.ended_at_ms >= min_ended_at)
        .filter(|r| r.session_id.as_deref() == Some(thread_id))
        .cloned()
        .collect::<Vec<_>>();

    if candidates.is_empty()
        && let Some(cwd) = cwd
    {
        candidates = recent
            .iter()
            .filter(|r| r.service == "codex")
            .filter(|r| r.ended_at_ms >= min_ended_at)
            .filter(|r| r.cwd.as_deref() == Some(cwd))
            .cloned()
            .collect::<Vec<_>>();
    }

    candidates
        .into_iter()
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
) -> anyhow::Result<Vec<FinishedRequestLite>> {
    let url = format!(
        "{}/__codex_helper/status/recent?limit=200",
        proxy_base_url.trim_end_matches('/')
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()?;
    let resp = client.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("proxy status/recent returned {}", status.as_u16());
    }
    let items = resp.json::<Vec<FinishedRequestLite>>().await?;
    Ok(items)
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
    let best = pick_best_recent_request(
        thread_id,
        payload.cwd.as_deref(),
        now,
        &notify_cfg.policy,
        &recent,
    );
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
    let tmp = dir.join("notify_state.json.tmp");
    let data = serde_json::to_vec_pretty(state)?;
    tokio::fs::write(&tmp, &data).await?;
    tokio::fs::rename(&tmp, &path).await?;
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
    use super::*;

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
            FinishedRequestLite {
                session_id: Some("th1".to_string()),
                cwd: Some("/p".to_string()),
                service: "codex".to_string(),
                method: "POST".to_string(),
                path: "/v1/chat/completions".to_string(),
                status_code: 200,
                duration_ms: 10_000,
                ended_at_ms: now - 1_000,
            },
            FinishedRequestLite {
                session_id: Some("th1".to_string()),
                cwd: Some("/p".to_string()),
                service: "codex".to_string(),
                method: "POST".to_string(),
                path: "/v1/responses".to_string(),
                status_code: 200,
                duration_ms: 20_000,
                ended_at_ms: now - 10_000,
            },
        ];
        let best =
            pick_best_recent_request("th1", Some("/p"), now, &policy, &recent).expect("best");
        assert_eq!(best.path, "/v1/responses");
    }
}
