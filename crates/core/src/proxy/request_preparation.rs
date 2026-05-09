use axum::body::Bytes;
use axum::http::{HeaderMap, Method};

use crate::logging::{BodyPreview, ServiceTierLog, make_body_preview};

use super::request_body::{
    apply_model_override_value, apply_reasoning_effort_override_value,
    apply_service_tier_override_value, extract_model_from_value,
    extract_reasoning_effort_from_value, extract_service_tier_from_value,
};

#[derive(Debug, Clone)]
pub(super) struct RequestFlavor {
    pub client_content_type: Option<String>,
    pub is_stream: bool,
    pub is_user_turn: bool,
    pub is_codex_service: bool,
}

#[derive(Debug, Clone)]
pub(super) struct PreparedRequestBody {
    pub body_for_upstream: Bytes,
    pub request_model: Option<String>,
    pub effective_effort: Option<String>,
    pub base_service_tier: ServiceTierLog,
    pub request_body_len: usize,
}

#[derive(Debug, Clone, Default)]
pub(super) struct BodyPreviewSet {
    pub debug: Option<BodyPreview>,
    pub warn: Option<BodyPreview>,
}

pub(super) fn detect_request_flavor(
    service_name: &str,
    method: &Method,
    headers: &HeaderMap,
    path: &str,
) -> RequestFlavor {
    let client_content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    let is_stream = headers
        .get("accept")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("text/event-stream"))
        .unwrap_or(false);

    let is_responses_path = path.ends_with("/responses");
    let is_user_turn = *method == Method::POST && is_responses_path;

    RequestFlavor {
        client_content_type,
        is_stream,
        is_user_turn,
        is_codex_service: service_name == "codex",
    }
}

pub(super) fn prepare_request_body(
    raw_body: &Bytes,
    override_effort: Option<&str>,
    binding_effort: Option<&str>,
    override_model: Option<&str>,
    binding_model: Option<&str>,
    override_service_tier: Option<&str>,
    binding_service_tier: Option<&str>,
) -> PreparedRequestBody {
    let mut request_json = serde_json::from_slice::<serde_json::Value>(raw_body).ok();
    let original_effort = request_json
        .as_ref()
        .and_then(extract_reasoning_effort_from_value);
    let original_service_tier = request_json
        .as_ref()
        .and_then(extract_service_tier_from_value);

    let is_object_root = request_json
        .as_ref()
        .is_some_and(serde_json::Value::is_object);
    if is_object_root && let Some(value) = request_json.as_mut() {
        if let Some(effort) = override_effort.or(binding_effort) {
            apply_reasoning_effort_override_value(value, effort);
        }
        if let Some(model) = override_model.or(binding_model) {
            apply_model_override_value(value, model);
        }
        if let Some(service_tier) = override_service_tier.or(binding_service_tier) {
            apply_service_tier_override_value(value, service_tier);
        }
    }

    let body_for_upstream = if is_object_root {
        request_json
            .as_ref()
            .and_then(|value| serde_json::to_vec(value).ok())
            .map(Bytes::from)
            .unwrap_or_else(|| raw_body.clone())
    } else {
        raw_body.clone()
    };

    let request_model = request_json.as_ref().and_then(extract_model_from_value);
    let effective_effort = request_json
        .as_ref()
        .and_then(extract_reasoning_effort_from_value)
        .or(original_effort);
    let effective_service_tier = request_json
        .as_ref()
        .and_then(extract_service_tier_from_value)
        .or(original_service_tier.clone());

    PreparedRequestBody {
        request_body_len: raw_body.len(),
        request_model,
        effective_effort,
        base_service_tier: ServiceTierLog {
            requested: original_service_tier,
            effective: effective_service_tier,
            actual: None,
        },
        body_for_upstream,
    }
}

pub(super) fn build_body_previews(
    body: &[u8],
    content_type: Option<&str>,
    previews_enabled: bool,
    debug_max: usize,
    warn_max: usize,
) -> BodyPreviewSet {
    if !previews_enabled {
        return BodyPreviewSet::default();
    }

    BodyPreviewSet {
        debug: (debug_max > 0).then(|| make_body_preview(body, content_type, debug_max)),
        warn: (warn_max > 0).then(|| make_body_preview(body, content_type, warn_max)),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::*;

    #[test]
    fn detect_request_flavor_reads_stream_and_turn_shape() {
        let mut headers = HeaderMap::new();
        headers.insert("accept", HeaderValue::from_static("text/event-stream"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let flavor = detect_request_flavor("codex", &Method::POST, &headers, "/v1/responses");

        assert_eq!(
            flavor.client_content_type.as_deref(),
            Some("application/json")
        );
        assert!(flavor.is_stream);
        assert!(flavor.is_user_turn);
        assert!(flavor.is_codex_service);
    }

    #[test]
    fn prepare_request_body_prefers_manual_overrides_over_binding_defaults() {
        let raw_body = Bytes::from_static(
            br#"{"model":"gpt-5","service_tier":"priority","reasoning":{"effort":"low"}}"#,
        );

        let prepared = prepare_request_body(
            &raw_body,
            Some("high"),
            Some("medium"),
            Some("gpt-5.4"),
            Some("gpt-5-mini"),
            Some("flex"),
            Some("default"),
        );

        assert_eq!(prepared.request_model.as_deref(), Some("gpt-5.4"));
        assert_eq!(prepared.effective_effort.as_deref(), Some("high"));
        assert_eq!(
            prepared.base_service_tier.requested.as_deref(),
            Some("priority")
        );
        assert_eq!(
            prepared.base_service_tier.effective.as_deref(),
            Some("flex")
        );
        assert_eq!(prepared.request_body_len, raw_body.len());
    }

    #[test]
    fn build_body_previews_respects_enable_flag_and_limits() {
        let previews = build_body_previews(
            br#"{"input":"hello"}"#,
            Some("application/json"),
            true,
            32,
            16,
        );
        assert!(previews.debug.is_some());
        assert!(previews.warn.is_some());

        let disabled = build_body_previews(
            br#"{"input":"hello"}"#,
            Some("application/json"),
            false,
            32,
            16,
        );
        assert!(disabled.debug.is_none());
        assert!(disabled.warn.is_none());
    }
}
