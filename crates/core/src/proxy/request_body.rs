use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue};

use crate::provider_catalog::ProviderModelRequestContract;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestDialect {
    ResponsesHttp,
    ResponsesCompact,
    ChatCompletions,
    ResponsesWebSocket,
    Passthrough,
}

impl RequestDialect {
    pub(super) fn from_http_path(path: &str) -> Self {
        let path = path.trim_end_matches('/');
        if path.ends_with("/responses/compact") {
            Self::ResponsesCompact
        } else if path.ends_with("/chat/completions") {
            Self::ChatCompletions
        } else if path.ends_with("/responses") {
            Self::ResponsesHttp
        } else {
            Self::Passthrough
        }
    }

    fn uses_responses_reasoning(self) -> bool {
        matches!(
            self,
            Self::ResponsesHttp | Self::ResponsesCompact | Self::ResponsesWebSocket
        )
    }

    pub(super) fn supports_reasoning_effort(self) -> bool {
        self.uses_responses_reasoning() || self == Self::ChatCompletions
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReasoningOrchestrationIntent {
    Ultra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum DeferredReasoningIntentError {
    #[error("reasoning intent requires a captured provider request contract")]
    MissingCapturedContract,
    #[error("selected provider request contract does not support the ultra intent")]
    UnsupportedUltra,
    #[error("selected request dialect cannot carry a reasoning effort")]
    UnsupportedDialect,
    #[error("request body must be a JSON object to resolve a reasoning intent")]
    InvalidRequestBody,
}

pub(super) fn apply_deferred_reasoning_intent(
    body: &Bytes,
    dialect: RequestDialect,
    intent: ReasoningOrchestrationIntent,
    contract: Option<&ProviderModelRequestContract>,
) -> Result<Bytes, DeferredReasoningIntentError> {
    let contract = contract.ok_or(DeferredReasoningIntentError::MissingCapturedContract)?;
    match intent {
        ReasoningOrchestrationIntent::Ultra if !contract.ultra_maps_to_max() => {
            return Err(DeferredReasoningIntentError::UnsupportedUltra);
        }
        ReasoningOrchestrationIntent::Ultra => {}
    }
    if !dialect.supports_reasoning_effort() {
        return Err(DeferredReasoningIntentError::UnsupportedDialect);
    }

    let mut value = serde_json::from_slice::<serde_json::Value>(body.as_ref())
        .map_err(|_| DeferredReasoningIntentError::InvalidRequestBody)?;
    if !apply_reasoning_effort_override_value(&mut value, dialect, "max") {
        return Err(DeferredReasoningIntentError::InvalidRequestBody);
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|_| DeferredReasoningIntentError::InvalidRequestBody)
}

pub(super) fn extract_reasoning_effort_from_value(
    value: &serde_json::Value,
    dialect: RequestDialect,
) -> Option<String> {
    let effort = if dialect.uses_responses_reasoning() {
        value
            .get("reasoning")
            .and_then(|reasoning| reasoning.get("effort"))
    } else if dialect == RequestDialect::ChatCompletions {
        value.get("reasoning_effort")
    } else {
        None
    };
    effort
        .and_then(serde_json::Value::as_str)
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

pub(super) fn codex_responses_body_requests_stream(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("stream").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

pub(super) fn codex_responses_body_has_compaction_trigger(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    value
        .get("input")
        .is_some_and(value_mentions_compaction_trigger)
}

fn value_mentions_compaction_trigger(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => {
            let is_compaction_trigger = object
                .get("type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value == "compaction_trigger");
            is_compaction_trigger || object.values().any(value_mentions_compaction_trigger)
        }
        serde_json::Value::Array(items) => items.iter().any(value_mentions_compaction_trigger),
        _ => false,
    }
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
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("service_tier"))
                .and_then(|service_tier| service_tier.as_str())
        })
        .map(ToOwned::to_owned)
}

fn extract_model_from_response_value(value: &serde_json::Value) -> Option<String> {
    value
        .get("model")
        .and_then(|model| model.as_str())
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("model"))
                .and_then(|model| model.as_str())
        })
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("model"))
                .and_then(|model| model.as_str())
        })
        .map(ToOwned::to_owned)
}

pub(super) fn merge_response_metadata_from_value(
    value: &serde_json::Value,
    last_model: &mut Option<String>,
    last_service_tier: &mut Option<String>,
) {
    if let Some(model) = extract_model_from_response_value(value) {
        *last_model = Some(model);
    }
    if let Some(service_tier) = extract_service_tier_from_response_value(value) {
        *last_service_tier = Some(service_tier);
    }
}

pub(super) fn extract_model_from_response_body(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    extract_model_from_response_value(&value)
}

pub(super) fn extract_service_tier_from_response_body(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    extract_service_tier_from_response_value(&value)
}

pub(super) fn apply_reasoning_effort_override_value(
    value: &mut serde_json::Value,
    dialect: RequestDialect,
    effort: &str,
) -> bool {
    let Some(object) = value.as_object_mut() else {
        return false;
    };

    if dialect.uses_responses_reasoning() {
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
        true
    } else if dialect == RequestDialect::ChatCompletions {
        object.insert(
            "reasoning_effort".to_string(),
            serde_json::Value::String(effort.to_string()),
        );
        true
    } else {
        false
    }
}

pub(super) fn remove_reasoning_effort_value(
    value: &mut serde_json::Value,
    dialect: RequestDialect,
) -> bool {
    let Some(object) = value.as_object_mut() else {
        return false;
    };

    if dialect.uses_responses_reasoning() {
        object
            .get_mut("reasoning")
            .and_then(serde_json::Value::as_object_mut)
            .and_then(|reasoning| reasoning.remove("effort"))
            .is_some()
    } else if dialect == RequestDialect::ChatCompletions {
        object.remove("reasoning_effort").is_some()
    } else {
        false
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

    for field in ["previous_response_id", "store", "stream", "include"] {
        object.remove(field);
    }
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

#[cfg(test)]
pub(super) fn scan_service_tier_from_sse_bytes_incremental(
    data: &[u8],
    scan_pos: &mut usize,
    last: &mut Option<String>,
) {
    let mut ignored_model = None;
    scan_response_metadata_from_sse_bytes_incremental(data, scan_pos, &mut ignored_model, last);
}

#[cfg(test)]
pub(super) fn scan_response_metadata_from_sse_bytes_incremental(
    data: &[u8],
    scan_pos: &mut usize,
    last_model: &mut Option<String>,
    last_service_tier: &mut Option<String>,
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

        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(payload) {
            merge_response_metadata_from_value(&value, last_model, last_service_tier);
        }
    }

    *scan_pos = i;
}

#[cfg(test)]
mod tests {
    use super::{
        DeferredReasoningIntentError, ReasoningOrchestrationIntent, RequestDialect,
        apply_deferred_reasoning_intent, apply_model_override_value,
        apply_reasoning_effort_override_value, apply_service_tier_override_value,
        codex_compact_request_requires_affinity, codex_responses_body_has_compaction_trigger,
        codex_responses_body_requests_stream, complete_codex_session_fields,
        extract_model_from_response_body, extract_model_from_value,
        extract_reasoning_effort_from_value, extract_service_tier_from_response_body,
        extract_service_tier_from_value, is_stale_previous_response_error,
        normalize_codex_compact_request_value, remove_previous_response_id_from_body,
        scan_response_metadata_from_sse_bytes_incremental,
        scan_service_tier_from_sse_bytes_incremental,
    };
    use crate::provider_catalog::{
        AccountFingerprint, ProviderAdapter, ProviderCatalogEpoch, ProviderCatalogScope,
        ProviderModelRequestContract,
    };
    use axum::body::Bytes;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};

    fn request_contract(model: &str) -> ProviderModelRequestContract {
        let scope = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://api.openai.com/v1",
            "codex/provider/default",
            AccountFingerprint::from_digest([7; 32]),
            "test-runtime",
        )
        .expect("provider scope");
        ProviderCatalogEpoch::bundled_openai_codex(scope)
            .expect("provider epoch")
            .capture_model_request_contract(model)
            .expect("provider request contract")
    }

    #[test]
    fn extracts_request_fields() {
        let body = br#"{
            "model":"gpt-5",
            "service_tier":"priority",
            "reasoning":{"effort":"high"}
        }"#;
        let value: serde_json::Value = serde_json::from_slice(body).expect("request json");

        assert_eq!(
            extract_reasoning_effort_from_value(&value, RequestDialect::ResponsesHttp).as_deref(),
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
        apply_reasoning_effort_override_value(&mut value, RequestDialect::ResponsesHttp, "medium");
        apply_model_override_value(&mut value, "gpt-5.4");
        apply_service_tier_override_value(&mut value, "flex");

        assert_eq!(
            extract_reasoning_effort_from_value(&value, RequestDialect::ResponsesHttp).as_deref(),
            Some("medium")
        );
        assert_eq!(extract_model_from_value(&value).as_deref(), Some("gpt-5.4"));
        assert_eq!(
            extract_service_tier_from_value(&value).as_deref(),
            Some("flex")
        );
    }

    #[test]
    fn deferred_ultra_maps_to_max_only_with_captured_provider_support() {
        let responses = Bytes::from_static(
            br#"{"reasoning":{"mode":"pro","future_mode":"deliberate"},"future_request_field":true}"#,
        );
        let supported = request_contract("gpt-5.6-sol");

        let resolved = apply_deferred_reasoning_intent(
            &responses,
            RequestDialect::ResponsesHttp,
            ReasoningOrchestrationIntent::Ultra,
            Some(&supported),
        )
        .expect("supported ultra mapping");
        let value: serde_json::Value =
            serde_json::from_slice(resolved.as_ref()).expect("json body");
        assert_eq!(value["reasoning"]["effort"].as_str(), Some("max"));
        assert_eq!(value["reasoning"]["mode"].as_str(), Some("pro"));
        assert_eq!(
            value["reasoning"]["future_mode"].as_str(),
            Some("deliberate")
        );
        assert_eq!(value["future_request_field"].as_bool(), Some(true));

        assert_eq!(
            apply_deferred_reasoning_intent(
                &responses,
                RequestDialect::ResponsesHttp,
                ReasoningOrchestrationIntent::Ultra,
                None,
            ),
            Err(DeferredReasoningIntentError::MissingCapturedContract)
        );
        assert_eq!(
            apply_deferred_reasoning_intent(
                &responses,
                RequestDialect::ResponsesHttp,
                ReasoningOrchestrationIntent::Ultra,
                Some(&request_contract("gpt-5.6-luna")),
            ),
            Err(DeferredReasoningIntentError::UnsupportedUltra)
        );
    }

    #[test]
    fn deferred_ultra_uses_chat_reasoning_effort_without_responses_reasoning() {
        let chat = Bytes::from_static(
            br#"{"messages":[],"parallel_tool_calls":false,"future_request_field":true}"#,
        );
        let supported = request_contract("gpt-5.6-terra");

        let resolved = apply_deferred_reasoning_intent(
            &chat,
            RequestDialect::ChatCompletions,
            ReasoningOrchestrationIntent::Ultra,
            Some(&supported),
        )
        .expect("supported chat ultra mapping");
        let value: serde_json::Value =
            serde_json::from_slice(resolved.as_ref()).expect("json body");
        assert_eq!(value["reasoning_effort"].as_str(), Some("max"));
        assert!(value.get("reasoning").is_none());
        assert_eq!(value["parallel_tool_calls"].as_bool(), Some(false));
        assert_eq!(value["future_request_field"].as_bool(), Some(true));
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
    fn normalizes_codex_compact_body_by_removing_only_known_incompatible_fields() {
        let mut value: serde_json::Value = serde_json::json!({
            "model": "gpt-5.5",
            "input": [{"type": "message", "role": "user", "content": "compact me"}],
            "instructions": "compact-test",
            "tools": [{"type": "function", "name": "shell"}],
            "parallel_tool_calls": false,
            "reasoning": {"effort": "high", "future_mode": "deliberate"},
            "text": {"verbosity": "low"},
            "previous_response_id": "resp_123",
            "store": true,
            "stream": true,
            "prompt_cache_key": "cache_123",
            "service_tier": "flex",
            "include": ["reasoning.encrypted_content"],
            "future_request_field": {"enabled": true}
        });

        normalize_codex_compact_request_value(&mut value);

        assert_eq!(value["model"].as_str(), Some("gpt-5.5"));
        assert!(value.get("tools").is_some());
        assert_eq!(value["parallel_tool_calls"].as_bool(), Some(false));
        assert_eq!(value["reasoning"]["effort"].as_str(), Some("high"));
        assert_eq!(
            value["reasoning"]["future_mode"].as_str(),
            Some("deliberate")
        );
        assert_eq!(value["text"]["verbosity"].as_str(), Some("low"));
        assert!(value.get("previous_response_id").is_none());
        assert!(value.get("store").is_none());
        assert!(value.get("stream").is_none());
        assert_eq!(value["prompt_cache_key"].as_str(), Some("cache_123"));
        assert_eq!(value["service_tier"].as_str(), Some("flex"));
        assert!(value.get("include").is_none());
        assert_eq!(
            value["future_request_field"]["enabled"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn request_dialect_is_selected_from_the_concrete_http_path() {
        assert_eq!(
            RequestDialect::from_http_path("/v1/responses"),
            RequestDialect::ResponsesHttp
        );
        assert_eq!(
            RequestDialect::from_http_path("/backend-api/codex/responses/"),
            RequestDialect::ResponsesHttp
        );
        assert_eq!(
            RequestDialect::from_http_path("/v1/responses/compact/"),
            RequestDialect::ResponsesCompact
        );
        assert_eq!(
            RequestDialect::from_http_path("/v1/chat/completions"),
            RequestDialect::ChatCompletions
        );
        assert_eq!(
            RequestDialect::from_http_path("/v1/messages"),
            RequestDialect::Passthrough
        );
    }

    #[test]
    fn detects_responses_stream_flag_from_body() {
        assert!(codex_responses_body_requests_stream(
            br#"{"model":"gpt-5","input":"hi","stream":true}"#
        ));
        assert!(!codex_responses_body_requests_stream(
            br#"{"model":"gpt-5","input":"hi","stream":false}"#
        ));
        assert!(!codex_responses_body_requests_stream(
            br#"{"model":"gpt-5","input":"hi"}"#
        ));
    }

    #[test]
    fn detects_structured_compaction_trigger_from_responses_body() {
        assert!(codex_responses_body_has_compaction_trigger(
            br#"{"model":"gpt-5","input":[{"type":"message","role":"user"},{"type":"compaction_trigger"}]}"#
        ));
        assert!(codex_responses_body_has_compaction_trigger(
            br#"{"model":"gpt-5","input":[{"content":[{"type":"compaction_trigger"}]}]}"#
        ));
        assert!(!codex_responses_body_has_compaction_trigger(
            br#"{"model":"gpt-5","input":"please mention compaction_trigger"}"#
        ));
        assert!(!codex_responses_body_has_compaction_trigger(
            br#"{"model":"gpt-5","input":[{"type":"message","name":"compaction_trigger"}]}"#
        ));
        assert!(!codex_responses_body_has_compaction_trigger(
            br#"{"model":"gpt-5","metadata":{"type":"compaction_trigger"},"input":"hi"}"#
        ));
        assert!(!codex_responses_body_has_compaction_trigger(b"not json"));
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
    fn extracts_reported_model_from_response_shapes() {
        assert_eq!(
            extract_model_from_response_body(br#"{"model":"gpt-5.6-sol"}"#).as_deref(),
            Some("gpt-5.6-sol")
        );
        assert_eq!(
            extract_model_from_response_body(br#"{"response":{"model":"gpt-5.6-terra"}}"#)
                .as_deref(),
            Some("gpt-5.6-terra")
        );
    }

    #[test]
    fn scans_reported_model_and_tier_from_sse_incrementally() {
        let chunk1 =
            b"data: {\"response\":{\"model\":\"gpt-5.6-sol\",\"service_tier\":\"priority\"}}\n";
        let chunk2 =
            b"data: {\"model\":\"gpt-5.6-terra\",\"service_tier\":\"flex\"}\n\ndata: [DONE]\n";
        let mut scan_pos = 0;
        let mut model = None;
        let mut tier = None;
        let mut data = Vec::new();

        data.extend_from_slice(chunk1);
        scan_response_metadata_from_sse_bytes_incremental(
            &data,
            &mut scan_pos,
            &mut model,
            &mut tier,
        );
        assert_eq!(model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(tier.as_deref(), Some("priority"));

        data.extend_from_slice(chunk2);
        scan_response_metadata_from_sse_bytes_incremental(
            &data,
            &mut scan_pos,
            &mut model,
            &mut tier,
        );
        assert_eq!(model.as_deref(), Some("gpt-5.6-terra"));
        assert_eq!(tier.as_deref(), Some("flex"));
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
