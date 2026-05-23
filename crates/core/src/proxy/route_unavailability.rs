use axum::body::Body;
use axum::http::{Response, StatusCode, header};

use crate::logging::{RouteAttemptLog, log_retry_trace};
use crate::routing_ir::{
    RouteCandidate, RoutePlanAttemptState, RoutePlanExecutor, RoutePlanRuntimeState,
    RoutePlanSkipReason,
};
use crate::runtime_identity::ProviderEndpointKey;

use super::request_preparation::RequestFlavor;

const DEFAULT_ROUTE_UNAVAILABLE_RETRY_SECS: u64 = 8;
const MAX_ROUTE_UNAVAILABLE_RETRY_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub(super) struct RouteUnavailableReport {
    pub(super) route_attempts: Vec<RouteAttemptLog>,
    pub(super) provider_endpoints_to_probe: Vec<ProviderEndpointKey>,
    pub(super) message: String,
}

impl RouteUnavailableReport {
    pub(super) fn failure_status_message(&self) -> (StatusCode, String) {
        (StatusCode::BAD_GATEWAY, self.message.clone())
    }

    pub(super) fn short_cooldown_wait_secs(&self, max_secs: u64) -> Option<u64> {
        let wait_secs = self
            .route_attempts
            .iter()
            .filter(|attempt| attempt.decision == "route_unavailable")
            .filter(|attempt| {
                attempt
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.split(',').any(|part| part == "cooldown"))
            })
            .filter_map(|attempt| attempt.cooldown_secs)
            .min()?;
        (wait_secs <= max_secs).then_some(wait_secs.saturating_add(1).max(1))
    }
}

pub(super) fn route_unavailable_report(
    service_name: &str,
    request_id: u64,
    executor: &RoutePlanExecutor<'_>,
    runtime: &RoutePlanRuntimeState,
    route_state: &RoutePlanAttemptState,
    request_model: Option<&str>,
) -> Option<RouteUnavailableReport> {
    let template = executor.template();
    if template.candidates.is_empty() {
        return None;
    }

    let explanations =
        executor.explain_candidate_skip_reasons_with_runtime_state(runtime, request_model);
    let mut route_attempts = Vec::new();
    let mut provider_endpoints_to_probe = Vec::new();
    let mut retry_after_secs = None::<u64>;
    let mut has_runtime_unavailable_reason = false;

    for candidate in &template.candidates {
        let provider_endpoint = template.candidate_provider_endpoint_key(candidate);
        let reasons = explanations
            .iter()
            .find(|explanation| explanation.provider_endpoint == provider_endpoint)
            .map(|explanation| explanation.reasons.clone())
            .unwrap_or_default();
        let avoided = route_state.avoids_candidate(template, candidate);
        if reasons.is_empty() && !avoided {
            continue;
        }

        let runtime_state = runtime.provider_endpoint(&provider_endpoint);
        if reasons
            .iter()
            .any(|reason| !matches!(reason, RoutePlanSkipReason::UnsupportedModel { .. }))
        {
            has_runtime_unavailable_reason = true;
        }
        if reasons.iter().any(|reason| {
            matches!(
                reason,
                RoutePlanSkipReason::Cooldown | RoutePlanSkipReason::UsageExhausted
            )
        }) {
            provider_endpoints_to_probe.push(provider_endpoint.clone());
        }
        if let Some(secs) = runtime_state.cooldown_remaining_secs {
            retry_after_secs = Some(retry_after_secs.map_or(secs, |current| current.min(secs)));
        }

        route_attempts.push(route_unavailable_attempt(
            template,
            candidate,
            &provider_endpoint,
            reasons,
            avoided,
            route_state,
            request_model,
        ));
    }

    if route_attempts.is_empty() || !has_runtime_unavailable_reason {
        return None;
    }

    provider_endpoints_to_probe.sort();
    provider_endpoints_to_probe.dedup();

    let retry_after_secs = retry_after_secs
        .unwrap_or(DEFAULT_ROUTE_UNAVAILABLE_RETRY_SECS)
        .clamp(1, MAX_ROUTE_UNAVAILABLE_RETRY_SECS);
    for attempt in &mut route_attempts {
        if attempt.decision == "route_unavailable" {
            attempt.cooldown_secs = Some(retry_after_secs);
            attempt.cooldown_reason = Some("route_unavailable".to_string());
        }
    }
    let reason_counts = reason_counts_for_log(&route_attempts);
    let message =
        format!("No upstreams are currently routable; try again in {retry_after_secs} seconds");

    log_retry_trace(serde_json::json!({
        "event": "route_graph_unavailable",
        "service": service_name,
        "request_id": request_id,
        "request_model": request_model,
        "retry_after_secs": retry_after_secs,
        "reason_counts": reason_counts,
        "provider_endpoints_to_probe": provider_endpoints_to_probe
            .iter()
            .map(ProviderEndpointKey::stable_key)
            .collect::<Vec<_>>(),
    }));

    Some(RouteUnavailableReport {
        route_attempts,
        provider_endpoints_to_probe,
        message,
    })
}

pub(super) fn route_unavailable_response_for_request(
    request_flavor: &RequestFlavor,
    message: &str,
    route_attempts: &[RouteAttemptLog],
) -> Option<Response<Body>> {
    if !request_flavor.is_codex_service || !request_flavor.is_user_turn || !request_flavor.is_stream
    {
        return None;
    }
    if !route_attempts
        .iter()
        .any(|attempt| attempt.decision == "route_unavailable")
    {
        return None;
    }
    Some(codex_stream_response(message, route_attempts))
}

fn route_unavailable_attempt(
    template: &crate::routing_ir::RoutePlanTemplate,
    candidate: &RouteCandidate,
    provider_endpoint: &ProviderEndpointKey,
    reasons: Vec<RoutePlanSkipReason>,
    avoided: bool,
    route_state: &RoutePlanAttemptState,
    request_model: Option<&str>,
) -> RouteAttemptLog {
    let mut reason_codes = reasons
        .iter()
        .map(RoutePlanSkipReason::code)
        .collect::<Vec<_>>();
    if avoided {
        reason_codes.push("attempt_avoided");
    }
    if reason_codes.is_empty() {
        reason_codes.push("not_selected");
    }
    reason_codes.sort_unstable();
    reason_codes.dedup();

    let reason = reason_codes.join(",");
    let raw = format!(
        "endpoint={} unavailable reason={} model={}",
        provider_endpoint.stable_key(),
        reason,
        request_model.unwrap_or("-")
    );

    RouteAttemptLog {
        attempt_index: 0,
        provider_id: Some(candidate.provider_id.clone()),
        endpoint_id: Some(candidate.endpoint_id.clone()),
        provider_endpoint_key: Some(provider_endpoint.stable_key()),
        preference_group: Some(candidate.preference_group),
        route_path: candidate.route_path.clone(),
        station_name: candidate
            .compatibility_station_name
            .as_ref()
            .or(template.compatibility_station_name.as_ref())
            .cloned(),
        upstream_base_url: Some(candidate.base_url.clone()),
        upstream_index: candidate.compatibility_upstream_index,
        avoided_candidate_indices: route_state.route_avoid_candidate_indices(template),
        avoided_total: Some(route_state.avoided_total()),
        total_upstreams: Some(template.candidates.len()),
        decision: "route_unavailable".to_string(),
        reason: Some(reason),
        error_class: Some("route_unavailable".to_string()),
        model: request_model.map(ToOwned::to_owned),
        skipped: true,
        raw,
        ..Default::default()
    }
}

fn reason_counts_for_log(route_attempts: &[RouteAttemptLog]) -> serde_json::Value {
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for attempt in route_attempts {
        if let Some(reason) = attempt.reason.as_deref() {
            for code in reason.split(',').filter(|code| !code.is_empty()) {
                *counts.entry(code.to_string()).or_default() += 1;
            }
        }
    }
    serde_json::to_value(counts).unwrap_or(serde_json::Value::Null)
}

fn codex_stream_response(message: &str, route_attempts: &[RouteAttemptLog]) -> Response<Body> {
    let retry_after_secs = route_unavailable_retry_after_secs(route_attempts)
        .unwrap_or(DEFAULT_ROUTE_UNAVAILABLE_RETRY_SECS);
    let body = synthetic_codex_rate_limit_sse(message, retry_after_secs);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from(body))
        .expect("synthetic SSE response should build")
}

fn synthetic_codex_rate_limit_sse(message: &str, retry_after_secs: u64) -> String {
    let payload = serde_json::json!({
        "type": "response.failed",
        "sequence_number": 1,
        "response": {
            "id": format!("resp_codex_helper_route_unavailable_{}", crate::logging::now_ms()),
            "object": "response",
            "created_at": crate::logging::now_ms() / 1000,
            "status": "failed",
            "background": false,
            "error": {
                "code": "rate_limit_exceeded",
                "message": message,
            },
            "usage": null,
            "user": null,
            "metadata": {
                "codex_helper_error": "route_unavailable",
                "retry_after_secs": retry_after_secs,
            },
        },
    });
    format!("event: response.failed\ndata: {payload}\n\n")
}

fn route_unavailable_retry_after_secs(route_attempts: &[RouteAttemptLog]) -> Option<u64> {
    route_attempts
        .iter()
        .find(|attempt| attempt.decision == "route_unavailable")
        .and_then(|attempt| attempt.cooldown_secs)
}
