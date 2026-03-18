use std::collections::VecDeque;

use super::*;

/// Read a best-effort transcript from a Codex session JSONL file.
///
/// If `tail` is Some(N), only the last N extracted messages are returned.
pub async fn read_codex_session_transcript(
    path: &Path,
    tail: Option<usize>,
) -> Result<Vec<SessionTranscriptMessage>> {
    match tail {
        Some(0) => Ok(Vec::new()),
        Some(n) => read_codex_session_transcript_tail(path, n).await,
        None => read_codex_session_transcript_full(path).await,
    }
}

/// Best-effort, case-insensitive substring search within the last `tail` transcript messages.
///
/// This is intended for interactive UIs (history/session manager). It trades completeness for speed:
/// - Only scans the last N extracted messages (not the full file).
/// - Returns `false` for empty queries or `tail == 0`.
pub async fn codex_session_transcript_tail_contains_query(
    path: &Path,
    query: &str,
    tail: usize,
) -> Result<bool> {
    let needle = query.trim();
    if needle.is_empty() || tail == 0 {
        return Ok(false);
    }

    let needle = needle.to_lowercase();
    let msgs = read_codex_session_transcript(path, Some(tail)).await?;
    Ok(msgs
        .iter()
        .any(|m| m.text.to_lowercase().contains(needle.as_str())))
}

async fn read_codex_session_transcript_full(path: &Path) -> Result<Vec<SessionTranscriptMessage>> {
    let file = fs::File::open(path)
        .await
        .with_context(|| format!("failed to open session file {:?}", path))?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    let mut out: Vec<SessionTranscriptMessage> = Vec::new();
    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let Some(msg) = extract_transcript_message(&value) else {
            continue;
        };
        if msg.text.trim().is_empty() {
            continue;
        }
        out.push(msg);
    }
    Ok(out)
}

async fn read_codex_session_transcript_tail(
    path: &Path,
    n: usize,
) -> Result<Vec<SessionTranscriptMessage>> {
    // Best-effort optimization: read a bounded window from the file tail instead of scanning the
    // whole JSONL. If we don't collect enough messages, expand the window a few times.
    let mut max_bytes = TAIL_SCAN_MAX_BYTES;
    let mut last: Vec<SessionTranscriptMessage> = Vec::new();
    for _ in 0..5 {
        let (bytes, started_mid) = read_file_tail_bytes(path, max_bytes).await?;
        last = extract_transcript_messages_from_jsonl_bytes(&bytes, started_mid, n);
        if last.len() >= n {
            break;
        }
        max_bytes = max_bytes.saturating_mul(2).min(16 * 1024 * 1024);
    }
    Ok(last)
}

async fn read_file_tail_bytes(path: &Path, max_bytes: usize) -> Result<(Vec<u8>, bool)> {
    let meta = fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat session file {:?}", path))?;
    let len = meta.len();
    let start = len.saturating_sub(max_bytes as u64);
    let started_mid = start > 0;

    let mut file = fs::File::open(path)
        .await
        .with_context(|| format!("failed to open session file {:?}", path))?;
    file.seek(std::io::SeekFrom::Start(start)).await?;

    let mut buf = Vec::new();
    file.read_to_end(&mut buf).await?;
    Ok((buf, started_mid))
}

fn extract_transcript_messages_from_jsonl_bytes(
    bytes: &[u8],
    started_mid: bool,
    tail_n: usize,
) -> Vec<SessionTranscriptMessage> {
    if tail_n == 0 {
        return Vec::new();
    }

    let mut slice = bytes;
    if started_mid {
        // If we started mid-file, the first line might be partial; drop it.
        if let Some(pos) = slice.iter().position(|&b| b == b'\n') {
            slice = &slice[pos + 1..];
        }
    }

    let mut ring: VecDeque<SessionTranscriptMessage> = VecDeque::with_capacity(tail_n.max(1));

    for raw in slice.split(|&b| b == b'\n') {
        if raw.is_empty() {
            continue;
        }
        let line = match std::str::from_utf8(raw) {
            Ok(s) => s.trim().trim_end_matches('\r'),
            Err(_) => continue,
        };
        if line.is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(msg) = extract_transcript_message(&value) else {
            continue;
        };
        if msg.text.trim().is_empty() {
            continue;
        }

        ring.push_back(msg);
        if ring.len() > tail_n {
            ring.pop_front();
        }
    }

    ring.into_iter().collect()
}

fn normalize_role(role: &str) -> String {
    match role {
        "user" => "User".to_string(),
        "assistant" => "Assistant".to_string(),
        "system" => "System".to_string(),
        other => other.to_string(),
    }
}

fn assistant_or_user_message_from_response_item(value: &Value) -> Option<(String, String)> {
    let obj = value.as_object()?;
    let type_str = obj.get("type")?.as_str()?;
    if type_str != "response_item" {
        return None;
    }
    let payload = obj.get("payload")?.as_object()?;
    let payload_type = payload.get("type")?.as_str()?;
    if payload_type != "message" {
        return None;
    }

    let role = payload.get("role")?.as_str()?;
    let text = payload
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|items| extract_text_from_content_items(items))?;

    Some((normalize_role(role), text))
}

fn extract_text_from_content_items(items: &[Value]) -> Option<String> {
    let mut out = String::new();
    for item in items {
        let obj = match item.as_object() {
            Some(o) => o,
            None => continue,
        };
        let t = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if !t.ends_with("_text") && t != "text" {
            continue;
        }
        let Some(text) = obj.get("text").and_then(|v| v.as_str()) else {
            continue;
        };
        out.push_str(text);
    }
    if out.is_empty() { None } else { Some(out) }
}

fn extract_transcript_message(value: &Value) -> Option<SessionTranscriptMessage> {
    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if let Some(msg) = user_message_text(value) {
        return Some(SessionTranscriptMessage {
            timestamp,
            role: "User".to_string(),
            text: msg.to_string(),
        });
    }

    if let Some((role, text)) = assistant_or_user_message_from_response_item(value) {
        return Some(SessionTranscriptMessage {
            timestamp,
            role,
            text,
        });
    }

    None
}
