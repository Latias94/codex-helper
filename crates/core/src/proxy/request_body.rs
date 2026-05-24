use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue};

pub(super) fn extract_reasoning_effort_from_value(value: &serde_json::Value) -> Option<String> {
    value
        .get("reasoning")
        .and_then(|reasoning| reasoning.get("effort"))
        .and_then(|effort| effort.as_str())
        .map(ToOwned::to_owned)
}

pub(super) fn extract_model_from_value(value: &serde_json::Value) -> Option<String> {
    value
        .get("model")
        .and_then(|model| model.as_str())
        .map(ToOwned::to_owned)
}

pub(super) fn extract_service_tier_from_value(value: &serde_json::Value) -> Option<String> {
    value
        .get("service_tier")
        .and_then(|service_tier| service_tier.as_str())
        .map(ToOwned::to_owned)
}

fn extract_service_tier_from_response_value(value: &serde_json::Value) -> Option<String> {
    value
        .get("service_tier")
        .and_then(|service_tier| service_tier.as_str())
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("service_tier"))
                .and_then(|service_tier| service_tier.as_str())
        })
        .map(ToOwned::to_owned)
}

pub(super) fn extract_service_tier_from_response_body(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    extract_service_tier_from_response_value(&value)
}

pub(super) fn apply_reasoning_effort_override_value(value: &mut serde_json::Value, effort: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let reasoning = object
        .entry("reasoning")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(reasoning_object) = reasoning.as_object_mut() {
        reasoning_object.insert(
            "effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
    } else {
        let mut new_reasoning = serde_json::Map::new();
        new_reasoning.insert(
            "effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
        *reasoning = serde_json::Value::Object(new_reasoning);
    }
}

pub(super) fn apply_model_override_value(value: &mut serde_json::Value, model: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object.insert(
        "model".to_string(),
        serde_json::Value::String(model.to_string()),
    );
}

pub(super) fn apply_service_tier_override_value(value: &mut serde_json::Value, service_tier: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    object.insert(
        "service_tier".to_string(),
        serde_json::Value::String(service_tier.to_string()),
    );
}

pub(super) fn normalize_codex_compact_request_value(value: &mut serde_json::Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };

    let mut normalized = serde_json::Map::new();
    for field in [
        "model",
        "input",
        "instructions",
        "tools",
        "parallel_tool_calls",
        "reasoning",
        "service_tier",
        "prompt_cache_key",
        "text",
    ] {
        if let Some(value) = object.get(field) {
            normalized.insert(field.to_string(), value.clone());
        }
    }

    *object = normalized;
}

pub(super) fn codex_compact_request_requires_affinity(body: &[u8]) -> bool {
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    normalize_codex_compact_request_value(&mut value);
    value_mentions_state_bound_compact_field(&value)
}

fn value_mentions_state_bound_compact_field(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => object.iter().any(|(key, value)| {
            let state_bound_field = matches!(
                key.as_str(),
                "encrypted_content" | "previous_response_id" | "compaction_summary"
            ) && !value.is_null();
            state_bound_field || value_mentions_state_bound_compact_field(value)
        }),
        serde_json::Value::Array(items) => {
            items.iter().any(value_mentions_state_bound_compact_field)
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CodexSessionCompletion {
    pub completed: bool,
    pub session_id: Option<String>,
}

pub(super) fn complete_codex_session_fields(
    headers: &mut HeaderMap,
    raw_body: &Bytes,
) -> (Bytes, CodexSessionCompletion) {
    let Some(session_id) = session_completion_candidate(headers, raw_body.as_ref()) else {
        return (raw_body.clone(), CodexSessionCompletion::default());
    };
    let thread_id = body_string_field(raw_body.as_ref(), &["prompt_cache_key"])
        .and_then(normalize_session_completion_value)
        .unwrap_or_else(|| session_id.clone());

    let mut completed = false;
    completed |= insert_missing_session_header(headers, "session_id", session_id.as_str());
    completed |= insert_missing_session_header(headers, "x-session-id", session_id.as_str());
    completed |= insert_missing_session_header(headers, "session-id", session_id.as_str());
    completed |= insert_missing_session_header(headers, "thread-id", thread_id.as_str());

    let body = serde_json::from_slice::<serde_json::Value>(raw_body.as_ref())
        .ok()
        .and_then(|mut value| {
            let object = value.as_object_mut()?;
            let missing_prompt_cache_key = object
                .get("prompt_cache_key")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .is_none_or(str::is_empty);
            if missing_prompt_cache_key {
                object.insert(
                    "prompt_cache_key".to_string(),
                    serde_json::Value::String(session_id.clone()),
                );
                completed = true;
            }
            serde_json::to_vec(&value).ok().map(Bytes::from)
        })
        .unwrap_or_else(|| raw_body.clone());

    (
        body,
        CodexSessionCompletion {
            completed,
            session_id: Some(session_id),
        },
    )
}

pub(super) fn codex_session_identity_and_completed_body(
    headers: &mut HeaderMap,
    raw_body: &Bytes,
) -> (
    Option<crate::proxy::client_identity::ClientSessionIdentity>,
    Bytes,
) {
    let session_identity_hint =
        crate::proxy::client_identity::extract_session_identity_with_body_fallback(
            headers,
            raw_body.as_ref(),
        );
    let completed_body = complete_codex_session_fields(headers, raw_body).0;
    (session_identity_hint, completed_body)
}

fn insert_missing_session_header(headers: &mut HeaderMap, name: &'static str, value: &str) -> bool {
    if has_header_value(headers, name) {
        return false;
    }
    let Ok(value) = HeaderValue::from_str(value) else {
        return false;
    };
    headers.insert(name, value);
    true
}

pub(super) fn remove_previous_response_id_from_body(body: &Bytes) -> Option<Bytes> {
    let mut value = serde_json::from_slice::<serde_json::Value>(body.as_ref()).ok()?;
    let object = value.as_object_mut()?;
    object.remove("previous_response_id")?;
    serde_json::to_vec(&value).ok().map(Bytes::from)
}

pub(super) fn is_stale_previous_response_error(
    status: axum::http::StatusCode,
    body: &[u8],
) -> bool {
    if status != axum::http::StatusCode::BAD_REQUEST && status != axum::http::StatusCode::NOT_FOUND
    {
        return false;
    }
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    let mentions_previous = text.contains("previous_response_id")
        || text.contains("previous response")
        || text.contains("previous_response");
    let missing = text.contains("not found")
        || text.contains("does not exist")
        || text.contains("doesn't exist")
        || text.contains("no response")
        || text.contains("not exist")
        || text.contains("missing");
    mentions_previous && missing
}

// Keep Codex session completion aligned with stable session anchors.
// `previous_response_id` is a turn-local continuation token, not a stable
// session key, so it must not be promoted to `prompt_cache_key`.
fn session_completion_candidate(headers: &HeaderMap, body: &[u8]) -> Option<String> {
    header_string(headers, "session_id")
        .or_else(|| header_string(headers, "x-session-id"))
        .or_else(|| header_string(headers, "session-id"))
        .or_else(|| header_string(headers, "conversation_id"))
        .or_else(|| header_string(headers, "thread-id"))
        .or_else(|| body_string_field(body, &["session_id"]))
        .or_else(|| body_string_field(body, &["x-session-id"]))
        .or_else(|| body_string_field(body, &["prompt_cache_key"]))
        .or_else(|| body_string_field(body, &["metadata", "session_id"]))
        .and_then(normalize_session_completion_value)
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn has_header_value(headers: &HeaderMap, name: &str) -> bool {
    header_string(headers, name).is_some()
}

fn body_string_field(body: &[u8], path: &[&str]) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    let mut current = &value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn normalize_session_completion_value(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 256 {
        return None;
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
        .then(|| value.to_string())
}

pub(super) fn scan_service_tier_from_sse_bytes_incremental(
    data: &[u8],
    scan_pos: &mut usize,
    last: &mut Option<String>,
) {
    let mut i = (*scan_pos).min(data.len());

    while i < data.len() {
        let Some(rel_end) = data[i..].iter().position(|b| *b == b'\n') else {
            break;
        };
        let end = i + rel_end;
        let mut line = &data[i..end];
        i = end.saturating_add(1);

        if line.ends_with(b"\r") {
            line = &line[..line.len().saturating_sub(1)];
        }
        if line.is_empty() {
            continue;
        }

        const DATA_PREFIX: &[u8] = b"data:";
        if !line.starts_with(DATA_PREFIX) {
            continue;
        }
        let mut payload = &line[DATA_PREFIX.len()..];
        while !payload.is_empty() && payload[0].is_ascii_whitespace() {
            payload = &payload[1..];
        }
        if payload.is_empty() || payload == b"[DONE]" {
            continue;
        }

        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload)
            && let Some(service_tier) = extract_service_tier_from_response_value(&value)
        {
            *last = Some(service_tier);
        }
    }

    *scan_pos = i;
}

#[cfg(test)]
mod tests {
    use super::{
        apply_model_override_value, apply_reasoning_effort_override_value,
        apply_service_tier_override_value, codex_compact_request_requires_affinity,
        complete_codex_session_fields, extract_model_from_value,
        extract_reasoning_effort_from_value, extract_service_tier_from_response_body,
        extract_service_tier_from_value, is_stale_previous_response_error,
        normalize_codex_compact_request_value, remove_previous_response_id_from_body,
        scan_service_tier_from_sse_bytes_incremental,
    };
    use axum::body::Bytes;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};

    #[test]
    fn extracts_request_fields() {
        let body = br#"{
            "model":"gpt-5",
            "service_tier":"priority",
            "reasoning":{"effort":"high"}
        }"#;
        let value: serde_json::Value = serde_json::from_slice(body).expect("request json");

        assert_eq!(
            extract_reasoning_effort_from_value(&value).as_deref(),
            Some("high")
        );
        assert_eq!(extract_model_from_value(&value).as_deref(), Some("gpt-5"));
        assert_eq!(
            extract_service_tier_from_value(&value).as_deref(),
            Some("priority")
        );
    }

    #[test]
    fn applies_request_overrides() {
        let body = br#"{"input":"hello"}"#;
        let mut value: serde_json::Value = serde_json::from_slice(body).expect("request json");
        apply_reasoning_effort_override_value(&mut value, "medium");
        apply_model_override_value(&mut value, "gpt-5.4");
        apply_service_tier_override_value(&mut value, "flex");

        assert_eq!(
            extract_reasoning_effort_from_value(&value).as_deref(),
            Some("medium")
        );
        assert_eq!(extract_model_from_value(&value).as_deref(), Some("gpt-5.4"));
        assert_eq!(
            extract_service_tier_from_value(&value).as_deref(),
            Some("flex")
        );
    }

    #[test]
    fn completes_codex_session_fields_without_overwriting_existing_values() {
        let mut headers = HeaderMap::new();
        let raw = Bytes::from_static(
            br#"{"model":"gpt-5","metadata":{"session_id":"meta-1"},"prompt_cache_key":"pcache-1"}"#,
        );

        let (body, completion) = complete_codex_session_fields(&mut headers, &raw);

        assert!(completion.completed);
        assert_eq!(completion.session_id.as_deref(), Some("pcache-1"));
        assert_eq!(
            headers.get("session_id"),
            Some(&HeaderValue::from_static("pcache-1"))
        );
        assert_eq!(
            headers.get("x-session-id"),
            Some(&HeaderValue::from_static("pcache-1"))
        );
        assert_eq!(
            headers.get("session-id"),
            Some(&HeaderValue::from_static("pcache-1"))
        );
        assert_eq!(
            headers.get("thread-id"),
            Some(&HeaderValue::from_static("pcache-1"))
        );
        let value: serde_json::Value = serde_json::from_slice(body.as_ref()).expect("json body");
        assert_eq!(value["prompt_cache_key"].as_str(), Some("pcache-1"));
    }

    #[test]
    fn completes_prompt_cache_key_from_metadata_session_id() {
        let mut headers = HeaderMap::new();
        let raw = Bytes::from_static(br#"{"model":"gpt-5","metadata":{"session_id":"meta-1"}}"#);

        let (body, completion) = complete_codex_session_fields(&mut headers, &raw);

        assert!(completion.completed);
        assert_eq!(completion.session_id.as_deref(), Some("meta-1"));
        assert_eq!(
            headers.get("session-id"),
            Some(&HeaderValue::from_static("meta-1"))
        );
        assert_eq!(
            headers.get("thread-id"),
            Some(&HeaderValue::from_static("meta-1"))
        );
        let value: serde_json::Value = serde_json::from_slice(body.as_ref()).expect("json body");
        assert_eq!(value["prompt_cache_key"].as_str(), Some("meta-1"));
    }

    #[test]
    fn does_not_complete_codex_session_fields_from_previous_response_id() {
        let mut headers = HeaderMap::new();
        let raw = Bytes::from_static(
            br#"{"model":"gpt-5","previous_response_id":"resp-1","input":"hi"}"#,
        );

        let (body, completion) = complete_codex_session_fields(&mut headers, &raw);

        assert!(!completion.completed);
        assert!(completion.session_id.is_none());
        assert!(headers.get("session_id").is_none());
        assert!(headers.get("x-session-id").is_none());
        assert!(headers.get("session-id").is_none());
        assert!(headers.get("thread-id").is_none());
        let value: serde_json::Value = serde_json::from_slice(body.as_ref()).expect("json body");
        assert!(value.get("prompt_cache_key").is_none());
    }

    #[test]
    fn normalizes_codex_compact_body_to_supported_payload_fields() {
        let mut value: serde_json::Value = serde_json::json!({
            "model": "gpt-5.5",
            "input": [{"type": "message", "role": "user", "content": "compact me"}],
            "instructions": "compact-test",
            "tools": [{"type": "function", "name": "shell"}],
            "parallel_tool_calls": true,
            "reasoning": {"effort": "high"},
            "text": {"verbosity": "low"},
            "previous_response_id": "resp_123",
            "store": true,
            "stream": true,
            "prompt_cache_key": "cache_123",
            "service_tier": "flex",
            "include": ["reasoning.encrypted_content"]
        });

        normalize_codex_compact_request_value(&mut value);

        assert_eq!(value["model"].as_str(), Some("gpt-5.5"));
        assert!(value.get("tools").is_some());
        assert_eq!(value["parallel_tool_calls"].as_bool(), Some(true));
        assert_eq!(value["reasoning"]["effort"].as_str(), Some("high"));
        assert_eq!(value["text"]["verbosity"].as_str(), Some("low"));
        assert!(value.get("previous_response_id").is_none());
        assert!(value.get("store").is_none());
        assert!(value.get("stream").is_none());
        assert_eq!(value["prompt_cache_key"].as_str(), Some("cache_123"));
        assert_eq!(value["service_tier"].as_str(), Some("flex"));
        assert!(value.get("previous_response_id").is_none());
        assert!(value.get("include").is_none());
    }

    #[test]
    fn detects_state_bound_codex_compact_body() {
        assert!(codex_compact_request_requires_affinity(
            br#"{"model":"gpt-5","input":[{"type":"reasoning","encrypted_content":"state"}]}"#
        ));
        assert!(!codex_compact_request_requires_affinity(
            br#"{"model":"gpt-5","previous_response_id":"resp_123","input":"hi"}"#
        ));
        assert!(codex_compact_request_requires_affinity(
            br#"{"model":"gpt-5","input":[{"type":"reasoning","previous_response_id":"resp_123"}]}"#
        ));
        assert!(!codex_compact_request_requires_affinity(
            br#"{"model":"gpt-5","prompt_cache_key":"cache","input":"hi"}"#
        ));
    }

    #[test]
    fn removes_previous_response_id_from_json_body() {
        let body = Bytes::from_static(
            br#"{"model":"gpt-5","previous_response_id":"resp-1","input":"hi"}"#,
        );

        let repaired = remove_previous_response_id_from_body(&body).expect("repaired body");

        let value: serde_json::Value =
            serde_json::from_slice(repaired.as_ref()).expect("json body");
        assert!(value.get("previous_response_id").is_none());
        assert_eq!(value["model"].as_str(), Some("gpt-5"));
    }

    #[test]
    fn stale_previous_response_error_detection_is_conservative() {
        assert!(is_stale_previous_response_error(
            StatusCode::BAD_REQUEST,
            br#"{"error":{"message":"No response found for previous_response_id resp-1"}}"#
        ));
        assert!(is_stale_previous_response_error(
            StatusCode::NOT_FOUND,
            br#"previous response does not exist"#
        ));
        assert!(!is_stale_previous_response_error(
            StatusCode::BAD_REQUEST,
            br#"{"error":"invalid model"}"#
        ));
        assert!(!is_stale_previous_response_error(
            StatusCode::TOO_MANY_REQUESTS,
            br#"No response found for previous_response_id resp-1"#
        ));
    }

    #[test]
    fn extracts_service_tier_from_response_shapes() {
        assert_eq!(
            extract_service_tier_from_response_body(br#"{"service_tier":"priority"}"#).as_deref(),
            Some("priority")
        );
        assert_eq!(
            extract_service_tier_from_response_body(br#"{"response":{"service_tier":"flex"}}"#)
                .as_deref(),
            Some("flex")
        );
    }

    #[test]
    fn scans_service_tier_from_sse_incrementally() {
        let chunk1 = b"data: {\"response\":{\"service_tier\":\"priority\"}}\n";
        let chunk2 = b"data: {\"service_tier\":\"flex\"}\n\ndata: [DONE]\n";
        let mut scan_pos = 0;
        let mut last = None;
        let mut data = Vec::new();

        data.extend_from_slice(chunk1);
        scan_service_tier_from_sse_bytes_incremental(&data, &mut scan_pos, &mut last);
        assert_eq!(last.as_deref(), Some("priority"));

        data.extend_from_slice(chunk2);
        scan_service_tier_from_sse_bytes_incremental(&data, &mut scan_pos, &mut last);
        assert_eq!(last.as_deref(), Some("flex"));
    }
}
