use axum::body::Body;
use axum::http::{Response, StatusCode};
use std::collections::BTreeMap;

use crate::logging::{RouteAttemptLog, log_control_trace_event};
use crate::routing_ir::{
    RouteCandidate, RoutePlanAttemptState, RoutePlanExecutor, RoutePlanRuntimeState,
    RoutePlanSkipReason,
};
use crate::runtime_identity::ProviderEndpointKey;

use super::codex_failure::{CodexFailureKind, CodexFailureSse};
use super::request_preparation::RequestFlavor;

const DEFAULT_ROUTE_UNAVAILABLE_RETRY_SECS: u64 = 8;
const MAX_ROUTE_UNAVAILABLE_RETRY_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub(super) struct RouteUnavailableReport {
    pub(super) route_attempts: Vec<RouteAttemptLog>,
    pub(super) provider_endpoints_to_probe: Vec<ProviderEndpointKey>,
    pub(super) message: String,
    status: StatusCode,
    short_cooldown_remaining_secs: Option<u64>,
}

impl RouteUnavailableReport {
    pub(super) fn failure_status_message(&self) -> (StatusCode, String) {
        (self.status, self.message.clone())
    }

    pub(super) fn short_cooldown_wait_secs(&self, max_secs: u64) -> Option<u64> {
        let wait_secs = self.short_cooldown_remaining_secs?;
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
    let mut cooldown_remaining_secs = None::<u64>;
    let mut has_runtime_unavailable_reason = false;
    let mut has_missing_auth_reason = false;
    let mut has_noncredential_route_block = false;
    let mut reason_counts = BTreeMap::<String, usize>::new();

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
        let model_unsupported = reasons
            .iter()
            .any(|reason| matches!(reason, RoutePlanSkipReason::UnsupportedModel { .. }));
        if !model_unsupported {
            for reason in &reasons {
                match reason {
                    RoutePlanSkipReason::UnsupportedModel { .. } => {}
                    RoutePlanSkipReason::MissingAuth => {
                        has_runtime_unavailable_reason = true;
                        has_missing_auth_reason = true;
                    }
                    _ => {
                        has_runtime_unavailable_reason = true;
                        has_noncredential_route_block = true;
                    }
                }
            }
            has_noncredential_route_block |= avoided;
        }
        if !model_unsupported
            && reasons.iter().any(|reason| {
                matches!(
                    reason,
                    RoutePlanSkipReason::Cooldown | RoutePlanSkipReason::UsageExhausted
                )
            })
        {
            provider_endpoints_to_probe.push(provider_endpoint.clone());
        }
        let has_cooldown_reason = !model_unsupported
            && reasons
                .iter()
                .any(|reason| matches!(reason, RoutePlanSkipReason::Cooldown));

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
        for code in reason_codes {
            *reason_counts.entry(code.to_string()).or_default() += 1;
        }
        let candidate_cooldown_secs = has_cooldown_reason
            .then_some(runtime_state.cooldown_remaining_secs)
            .flatten();
        if let Some(secs) = candidate_cooldown_secs {
            cooldown_remaining_secs =
                Some(cooldown_remaining_secs.map_or(secs, |current| current.min(secs)));
        }

        let mut attempt = route_unavailable_attempt(
            template,
            candidate,
            &provider_endpoint,
            route_state,
            request_model,
        );
        if has_cooldown_reason {
            attempt.cooldown_secs = candidate_cooldown_secs;
            attempt.cooldown_reason = Some("runtime_cooldown".to_string());
        }
        route_attempts.push(attempt);
    }

    if route_attempts.is_empty() || !has_runtime_unavailable_reason {
        return None;
    }

    provider_endpoints_to_probe.sort();
    provider_endpoints_to_probe.dedup();

    let codex_sse_retry_hint_secs = bounded_retry_hint(cooldown_remaining_secs);
    let credential_only = has_missing_auth_reason && !has_noncredential_route_block;
    let message = if credential_only {
        "No upstreams are currently routable because configured upstream credentials are unavailable"
            .to_string()
    } else if let Some(cooldown_remaining_secs) = cooldown_remaining_secs {
        format!(
            "No upstreams are currently routable; try again in {cooldown_remaining_secs} seconds"
        )
    } else {
        "No upstreams are currently routable".to_string()
    };

    log_control_trace_event(serde_json::json!({
        "event": "route_graph_unavailable",
        "service": service_name,
        "request_id": request_id,
        "request_model": request_model,
        "codex_sse_retry_hint_secs": codex_sse_retry_hint_secs,
        "cooldown_remaining_secs": cooldown_remaining_secs,
        "credential_only": credential_only,
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
        status: if credential_only {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::BAD_GATEWAY
        },
        short_cooldown_remaining_secs: cooldown_remaining_secs,
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
    let failure_kind = codex_stream_failure_kind(route_attempts)?;
    Some(codex_stream_response(message, route_attempts, failure_kind))
}

fn codex_stream_failure_kind(route_attempts: &[RouteAttemptLog]) -> Option<CodexFailureKind> {
    let has_route_unavailable = route_attempts
        .iter()
        .any(|attempt| attempt.decision == "route_unavailable");
    if has_route_unavailable {
        return Some(CodexFailureKind::RouteUnavailable);
    }

    route_attempts
        .iter()
        .any(|attempt| {
            matches!(
                attempt.decision.as_str(),
                "failed_status" | "failed_transport" | "failed_body_read" | "failed_body_too_large"
            )
        })
        .then_some(CodexFailureKind::UpstreamFailure)
}

fn route_unavailable_attempt(
    template: &crate::routing_ir::RoutePlanTemplate,
    candidate: &RouteCandidate,
    provider_endpoint: &ProviderEndpointKey,
    route_state: &RoutePlanAttemptState,
    request_model: Option<&str>,
) -> RouteAttemptLog {
    let mut attempt = RouteAttemptLog {
        attempt_index: 0,
        provider_id: Some(candidate.provider_id.clone()),
        endpoint_id: Some(candidate.endpoint_id.clone()),
        provider_endpoint_key: Some(provider_endpoint.stable_key()),
        preference_group: Some(candidate.preference_group),
        route_path: candidate.route_path.clone(),
        avoided_candidate_indices: route_state.route_avoid_candidate_indices(template),
        avoided_total: Some(route_state.avoided_total()),
        total_upstreams: Some(template.candidates.len()),
        decision: "route_unavailable".to_string(),
        error_class: Some("route_unavailable".to_string()),
        model: request_model.map(ToOwned::to_owned),
        skipped: true,
        ..Default::default()
    };
    attempt.refresh_code();
    attempt
}

fn codex_stream_response(
    message: &str,
    route_attempts: &[RouteAttemptLog],
    failure_kind: CodexFailureKind,
) -> Response<Body> {
    let retry_after_secs = bounded_retry_hint(route_unavailable_retry_after_secs(route_attempts));
    CodexFailureSse::route_failure(message, retry_after_secs, failure_kind).into_response()
}

fn bounded_retry_hint(retry_after_secs: Option<u64>) -> u64 {
    retry_after_secs
        .unwrap_or(DEFAULT_ROUTE_UNAVAILABLE_RETRY_SECS)
        .clamp(1, MAX_ROUTE_UNAVAILABLE_RETRY_SECS)
}

fn route_unavailable_retry_after_secs(route_attempts: &[RouteAttemptLog]) -> Option<u64> {
    route_attempts
        .iter()
        .filter(|attempt| attempt.decision == "route_unavailable")
        .filter_map(|attempt| attempt.cooldown_secs)
        .min()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::config::{RouteAffinityPolicy, SchedulingPreset, UpstreamAuth};
    use crate::credentials::{CredentialGeneration, CredentialReadinessCode};
    use crate::routing_ir::{
        RouteCandidateConcurrency, RoutePlanTemplate, RoutePlanUpstreamRuntimeState,
    };

    use super::*;

    fn test_template(provider_ids: &[&str]) -> RoutePlanTemplate {
        RoutePlanTemplate {
            service_name: "codex".to_string(),
            entry: "default".to_string(),
            affinity_policy: RouteAffinityPolicy::Off,
            scheduling_preset: SchedulingPreset::Balanced,
            fallback_ttl_ms: None,
            reprobe_preferred_after_ms: None,
            nodes: BTreeMap::new(),
            expanded_provider_order: provider_ids
                .iter()
                .map(|provider_id| (*provider_id).to_string())
                .collect(),
            candidates: provider_ids
                .iter()
                .enumerate()
                .map(|(index, provider_id)| RouteCandidate {
                    provider_id: (*provider_id).to_string(),
                    provider_alias: None,
                    endpoint_id: "default".to_string(),
                    base_url: format!("https://{provider_id}.example/v1"),
                    continuity_domain: None,
                    auth: UpstreamAuth::default(),
                    tags: BTreeMap::new(),
                    supported_models: BTreeMap::new(),
                    model_mapping: BTreeMap::new(),
                    model_rules: Arc::default(),
                    route_path: vec!["default".to_string(), (*provider_id).to_string()],
                    preference_group: index as u32,
                    stable_index: index,
                    concurrency: RouteCandidateConcurrency::default(),
                })
                .collect(),
            credential_generation: CredentialGeneration::empty(),
        }
    }

    fn provider_endpoint(template: &RoutePlanTemplate, index: usize) -> ProviderEndpointKey {
        template.candidate_provider_endpoint_key(&template.candidates[index])
    }

    #[tokio::test]
    async fn credential_only_unavailability_has_no_synthetic_cooldown() {
        let template = test_template(&["primary", "backup"]);
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        for index in 0..template.candidates.len() {
            runtime.set_provider_endpoint(
                provider_endpoint(&template, index),
                RoutePlanUpstreamRuntimeState {
                    credential_readiness: CredentialReadinessCode::Missing,
                    ..Default::default()
                },
            );
        }

        let report = route_unavailable_report(
            "codex",
            1,
            &executor,
            &runtime,
            &RoutePlanAttemptState::default(),
            Some("gpt-5"),
        )
        .expect("credential-only report");

        assert_eq!(report.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(report.message.contains("credentials are unavailable"));
        assert!(report.provider_endpoints_to_probe.is_empty());
        assert_eq!(report.short_cooldown_wait_secs(60), None);
        assert!(report.route_attempts.iter().all(|attempt| {
            attempt.cooldown_secs.is_none() && attempt.cooldown_reason.is_none()
        }));
        assert_eq!(
            route_unavailable_retry_after_secs(&report.route_attempts),
            None
        );

        let response = codex_stream_response(
            report.message.as_str(),
            &report.route_attempts,
            CodexFailureKind::RouteUnavailable,
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read synthetic SSE body");
        let body = String::from_utf8(body.to_vec()).expect("SSE body is UTF-8");
        assert!(body.contains(r#""retry_after_secs":8"#), "{body}");
        assert!(body.contains("credentials are unavailable"), "{body}");
    }

    #[test]
    fn mixed_credential_and_cooldown_reasons_keep_candidate_local_metadata() {
        let template = test_template(&["credential-blocked", "cooldown-blocked"]);
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            provider_endpoint(&template, 0),
            RoutePlanUpstreamRuntimeState {
                credential_readiness: CredentialReadinessCode::Missing,
                ..Default::default()
            },
        );
        runtime.set_provider_endpoint(
            provider_endpoint(&template, 1),
            RoutePlanUpstreamRuntimeState {
                cooldown_active: true,
                cooldown_remaining_secs: Some(30),
                ..Default::default()
            },
        );

        let report = route_unavailable_report(
            "codex",
            2,
            &executor,
            &runtime,
            &RoutePlanAttemptState::default(),
            Some("gpt-5"),
        )
        .expect("mixed report");

        assert_eq!(report.status, StatusCode::BAD_GATEWAY);
        assert!(!report.message.contains("credentials are unavailable"));
        assert!(report.message.contains("30 seconds"));
        assert_eq!(report.short_cooldown_wait_secs(60), Some(31));
        assert_eq!(
            route_unavailable_retry_after_secs(&report.route_attempts),
            Some(30)
        );
        assert_eq!(
            report.provider_endpoints_to_probe,
            vec![provider_endpoint(&template, 1)]
        );

        let credential_attempt = report
            .route_attempts
            .iter()
            .find(|attempt| attempt.provider_id.as_deref() == Some("credential-blocked"))
            .expect("credential attempt");
        assert_eq!(credential_attempt.cooldown_secs, None);
        assert_eq!(credential_attempt.cooldown_reason, None);

        let cooldown_attempt = report
            .route_attempts
            .iter()
            .find(|attempt| attempt.provider_id.as_deref() == Some("cooldown-blocked"))
            .expect("cooldown attempt");
        assert_eq!(cooldown_attempt.cooldown_secs, Some(30));
        assert_eq!(
            cooldown_attempt.cooldown_reason.as_deref(),
            Some("runtime_cooldown")
        );
    }

    #[test]
    fn attempted_failure_plus_missing_credential_is_not_reported_as_credential_only() {
        let template = test_template(&["attempted", "credential-blocked"]);
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            provider_endpoint(&template, 1),
            RoutePlanUpstreamRuntimeState {
                credential_readiness: CredentialReadinessCode::Missing,
                ..Default::default()
            },
        );
        let mut route_state = RoutePlanAttemptState::default();
        assert!(route_state.avoid_candidate(&template, &template.candidates[0]));

        let report =
            route_unavailable_report("codex", 4, &executor, &runtime, &route_state, Some("gpt-5"))
                .expect("mixed attempted and credential-blocked report");

        assert_eq!(report.status, StatusCode::BAD_GATEWAY);
        assert!(!report.message.contains("credentials are unavailable"));
        assert!(report.route_attempts.iter().all(|attempt| {
            attempt.cooldown_secs.is_none() && attempt.cooldown_reason.is_none()
        }));
    }

    #[test]
    fn unsupported_model_with_missing_credential_does_not_create_runtime_report() {
        let mut template = test_template(&["unsupported"]);
        template.candidates[0].supported_models = BTreeMap::from([("gpt-4".to_string(), true)]);
        template.candidates[0].model_rules = Arc::new(
            crate::model_routing::CompiledModelRules::compile(
                [("gpt-4".to_string(), true)],
                std::iter::empty(),
            )
            .expect("compile model rules"),
        );
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            provider_endpoint(&template, 0),
            RoutePlanUpstreamRuntimeState {
                credential_readiness: CredentialReadinessCode::Missing,
                ..Default::default()
            },
        );

        let report = route_unavailable_report(
            "codex",
            5,
            &executor,
            &runtime,
            &RoutePlanAttemptState::default(),
            Some("gpt-5"),
        );

        assert!(report.is_none());
    }

    #[test]
    fn unsupported_candidate_does_not_distort_compatible_credential_block() {
        let mut template = test_template(&["unsupported", "compatible"]);
        template.candidates[0].supported_models = BTreeMap::from([("gpt-4".to_string(), true)]);
        template.candidates[0].model_rules = Arc::new(
            crate::model_routing::CompiledModelRules::compile(
                [("gpt-4".to_string(), true)],
                std::iter::empty(),
            )
            .expect("compile model rules"),
        );
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            provider_endpoint(&template, 0),
            RoutePlanUpstreamRuntimeState {
                cooldown_active: true,
                cooldown_remaining_secs: Some(5),
                credential_readiness: CredentialReadinessCode::Missing,
                ..Default::default()
            },
        );
        runtime.set_provider_endpoint(
            provider_endpoint(&template, 1),
            RoutePlanUpstreamRuntimeState {
                credential_readiness: CredentialReadinessCode::Missing,
                ..Default::default()
            },
        );

        let report = route_unavailable_report(
            "codex",
            6,
            &executor,
            &runtime,
            &RoutePlanAttemptState::default(),
            Some("gpt-5"),
        )
        .expect("compatible credential block report");

        assert_eq!(report.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(report.message.contains("credentials are unavailable"));
        assert!(report.provider_endpoints_to_probe.is_empty());
        assert_eq!(report.short_cooldown_wait_secs(10), None);
        let unsupported_attempt = report
            .route_attempts
            .iter()
            .find(|attempt| attempt.provider_id.as_deref() == Some("unsupported"))
            .expect("unsupported attempt");
        assert_eq!(unsupported_attempt.cooldown_secs, None);
        assert_eq!(unsupported_attempt.cooldown_reason, None);
    }

    #[tokio::test]
    async fn long_cooldown_stays_exact_in_attempt_and_is_bounded_only_for_sse() {
        let template = test_template(&["cooldown"]);
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            provider_endpoint(&template, 0),
            RoutePlanUpstreamRuntimeState {
                cooldown_active: true,
                cooldown_remaining_secs: Some(300),
                ..Default::default()
            },
        );

        let report = route_unavailable_report(
            "codex",
            7,
            &executor,
            &runtime,
            &RoutePlanAttemptState::default(),
            Some("gpt-5"),
        )
        .expect("long cooldown report");

        assert_eq!(report.route_attempts[0].cooldown_secs, Some(300));
        assert!(report.message.contains("300 seconds"));
        assert_eq!(report.short_cooldown_wait_secs(10), None);

        let response = codex_stream_response(
            report.message.as_str(),
            &report.route_attempts,
            CodexFailureKind::RouteUnavailable,
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read synthetic SSE body");
        let body = String::from_utf8(body.to_vec()).expect("SSE body is UTF-8");
        assert!(body.contains(r#""retry_after_secs":60"#), "{body}");
    }

    #[test]
    fn usage_exhaustion_keeps_retry_hint_out_of_cooldown_metadata() {
        let template = test_template(&["limited"]);
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            provider_endpoint(&template, 0),
            RoutePlanUpstreamRuntimeState {
                usage_exhausted: true,
                ..Default::default()
            },
        );

        let report = route_unavailable_report(
            "codex",
            3,
            &executor,
            &runtime,
            &RoutePlanAttemptState::default(),
            Some("gpt-5"),
        )
        .expect("usage-exhausted report");

        assert_eq!(report.status, StatusCode::BAD_GATEWAY);
        assert_eq!(report.message, "No upstreams are currently routable");
        assert_eq!(report.short_cooldown_wait_secs(60), None);
        assert_eq!(
            report.provider_endpoints_to_probe,
            vec![provider_endpoint(&template, 0)]
        );
        assert_eq!(report.route_attempts[0].cooldown_secs, None);
        assert_eq!(report.route_attempts[0].cooldown_reason, None);
    }
}
