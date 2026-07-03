use std::collections::HashSet;
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, Response, StatusCode, header};

use crate::lb::{CooldownBackoff, LoadBalancer};
use crate::logging::{
    CodexBridgeLog, RouteAttemptLog, ServiceTierLog, log_retry_trace, make_body_preview,
};
use crate::state::SessionIdentitySource;
use crate::usage::{UsageMetrics, extract_usage_from_bytes};
use crate::usage_providers;

use super::ProxyService;
use super::attempt_health::{
    penalize_attempt_target, record_attempt_failure, record_attempt_success,
};
use super::attempt_target::AttemptTarget;
use super::classify::{class_is_health_neutral, classify_observed_upstream_response};
use super::concurrency_limits::ConcurrencyPermit;
use super::http_debug::HttpDebugBase;
use super::models_compat::maybe_decode_models_response_body;
use super::passive_health::{record_passive_upstream_failure, record_passive_upstream_success};
use super::provider_evidence::{ResponseEvidenceParams, response_evidence_from_classification};
use super::reasoning_guard::{
    REASONING_GUARD_BLOCKED_CLASS, evaluate_reasoning_guard, reasoning_guard_error_body,
    reasoning_guard_retry_count,
};
use super::request_body::extract_service_tier_from_response_body;
use super::response_finalization::{
    FinalizeForwardResponseParams, finish_and_build_forward_response,
};
use super::response_fixer::{
    CodexCompactSseRepair, maybe_repair_codex_compact_sse_response,
    maybe_repair_codex_response_body,
};
use super::response_semantics::{
    IMAGE_GENERATION_MISSING_RESULT_CLASS, ResponseSemanticContract,
    validate_success_response_semantics,
};
use super::retry::{
    RetryLayerOptions, RetryPlan, response_penalty_cooldown_secs, retry_info_for_observed_attempts,
    retry_sleep, should_never_retry, should_retry_class, should_retry_status,
};
use super::route_affinity::record_session_route_affinity_success;
use super::route_attempts::{StatusRouteAttemptParams, record_status_route_attempt};
use super::stream::{SseSuccessMeta, build_sse_success_response};

pub(super) enum AttemptResponseOutcome {
    RetrySameUpstream,
    TryNextUpstream,
    Return(Response<Body>),
}

pub(super) struct AttemptResponseParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) legacy_lb: Option<&'a LoadBalancer>,
    pub(super) target: &'a AttemptTarget,
    pub(super) method: &'a Method,
    pub(super) path: &'a str,
    pub(super) status: StatusCode,
    pub(super) response_headers: HeaderMap,
    pub(super) response_headers_filtered: HeaderMap,
    pub(super) response_body: Bytes,
    pub(super) request_id: u64,
    pub(super) duration_ms: u64,
    pub(super) started_at_ms: u64,
    pub(super) upstream_headers_ms: u64,
    pub(super) provider_id: Option<&'a str>,
    pub(super) session_id: Option<&'a str>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) cwd: Option<&'a str>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) base_service_tier: &'a ServiceTierLog,
    pub(super) codex_bridge: Option<CodexBridgeLog>,
    pub(super) response_semantic_contract: Option<ResponseSemanticContract>,
    pub(super) route_graph_key: Option<&'a str>,
    pub(super) upstream_chain: &'a mut Vec<String>,
    pub(super) route_attempts: &'a mut Vec<RouteAttemptLog>,
    pub(super) route_attempt_index: usize,
    pub(super) model_note: &'a str,
    pub(super) plan: &'a RetryPlan,
    pub(super) upstream_opt: &'a RetryLayerOptions,
    pub(super) provider_opt: &'a RetryLayerOptions,
    pub(super) upstream_attempt: u32,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
    pub(super) cooldown_backoff: CooldownBackoff,
    pub(super) is_user_turn: bool,
    pub(super) allow_provider_failover: bool,
    pub(super) is_codex_service: bool,
}

pub(super) struct StreamingAttemptResponseParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) legacy_lb: Option<&'a LoadBalancer>,
    pub(super) target: &'a AttemptTarget,
    pub(super) response: reqwest::Response,
    pub(super) status: StatusCode,
    pub(super) response_headers: HeaderMap,
    pub(super) response_headers_filtered: HeaderMap,
    pub(super) start: Instant,
    pub(super) started_at_ms: u64,
    pub(super) upstream_start: Instant,
    pub(super) upstream_headers_ms: u64,
    pub(super) request_body_len: usize,
    pub(super) upstream_request_body_len: usize,
    pub(super) debug_base: Option<HttpDebugBase>,
    pub(super) upstream_chain: &'a mut Vec<String>,
    pub(super) route_attempts: &'a mut Vec<RouteAttemptLog>,
    pub(super) route_attempt_index: usize,
    pub(super) model_note: &'a str,
    pub(super) session_id: Option<&'a str>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) cwd: Option<&'a str>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) base_service_tier: &'a ServiceTierLog,
    pub(super) codex_bridge: Option<CodexBridgeLog>,
    pub(super) route_graph_key: Option<&'a str>,
    pub(super) request_id: u64,
    pub(super) is_user_turn: bool,
    pub(super) is_codex_service: bool,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cloudflare_challenge_cooldown_secs: u64,
    pub(super) cloudflare_timeout_cooldown_secs: u64,
    pub(super) cooldown_backoff: CooldownBackoff,
    pub(super) method: &'a Method,
    pub(super) path: &'a str,
    pub(super) concurrency_permit: Option<ConcurrencyPermit>,
}

fn summarize_upstream_error_body(response_body: &Bytes, response_headers: &HeaderMap) -> String {
    let content_type = response_headers
        .get("content-type")
        .and_then(|value| value.to_str().ok());
    let preview = make_body_preview(response_body.as_ref(), content_type, 2048);
    if preview.encoding == "utf8" {
        if preview.truncated {
            format!("{}…", preview.data)
        } else {
            preview.data
        }
    } else {
        format!("binary response body ({} bytes)", preview.original_len)
    }
}

struct AttemptResponseDecision {
    never_retry: bool,
    retry_same_upstream: bool,
    provider_penalty: bool,
    provider_failover: bool,
    penalty_cooldown_secs: u64,
    should_probe_codex_usage: bool,
}

struct AttemptResponseDecisionParams<'a> {
    plan: &'a RetryPlan,
    upstream_opt: &'a RetryLayerOptions,
    provider_opt: &'a RetryLayerOptions,
    status: StatusCode,
    class: Option<&'a str>,
    retry_after_secs: Option<u64>,
    upstream_attempt: u32,
    allow_provider_failover: bool,
    compact_protocol_failure: bool,
    is_user_turn: bool,
    is_codex_service: bool,
}

fn decide_attempt_response(params: AttemptResponseDecisionParams<'_>) -> AttemptResponseDecision {
    let AttemptResponseDecisionParams {
        plan,
        upstream_opt,
        provider_opt,
        status,
        class,
        retry_after_secs,
        upstream_attempt,
        allow_provider_failover,
        compact_protocol_failure,
        is_user_turn,
        is_codex_service,
    } = params;
    let status_code = status.as_u16();
    let reasoning_guard_blocked = matches!(class, Some(REASONING_GUARD_BLOCKED_CLASS));
    let never_retry = should_never_retry(plan, status_code, class)
        || compact_protocol_failure
        || reasoning_guard_blocked;
    let semantic_failure_requires_provider_failover =
        matches!(class, Some(IMAGE_GENERATION_MISSING_RESULT_CLASS));
    let upstream_retryable = !never_retry
        && !semantic_failure_requires_provider_failover
        && (should_retry_status(upstream_opt, status_code)
            || should_retry_class(upstream_opt, class));
    let retry_same_upstream =
        upstream_retryable && upstream_attempt + 1 < upstream_opt.max_attempts;
    let provider_retryable = !never_retry
        && (should_retry_status(provider_opt, status_code)
            || should_retry_class(provider_opt, class));
    let provider_penalty =
        !status.is_success() && !never_retry && !retry_same_upstream && provider_retryable;
    let provider_failover = provider_penalty && allow_provider_failover;
    let penalty_cooldown_secs = response_penalty_cooldown_secs(
        plan.cloudflare_challenge_cooldown_secs,
        plan.cloudflare_timeout_cooldown_secs,
        plan.transport_cooldown_secs,
        class,
        retry_after_secs,
    );
    let should_probe_codex_usage =
        status_code == StatusCode::TOO_MANY_REQUESTS.as_u16() && is_user_turn && is_codex_service;

    AttemptResponseDecision {
        never_retry,
        retry_same_upstream,
        provider_penalty,
        provider_failover,
        penalty_cooldown_secs,
        should_probe_codex_usage,
    }
}

pub(super) async fn handle_streaming_attempt_success(
    params: StreamingAttemptResponseParams<'_>,
) -> Response<Body> {
    let StreamingAttemptResponseParams {
        proxy,
        legacy_lb,
        target,
        response,
        status,
        response_headers,
        response_headers_filtered,
        start,
        started_at_ms,
        upstream_start,
        upstream_headers_ms,
        request_body_len,
        upstream_request_body_len,
        debug_base,
        upstream_chain,
        route_attempts,
        route_attempt_index,
        model_note,
        session_id,
        session_identity_source,
        cwd,
        effective_effort,
        base_service_tier,
        codex_bridge,
        route_graph_key,
        request_id,
        is_user_turn,
        is_codex_service,
        transport_cooldown_secs,
        cloudflare_challenge_cooldown_secs,
        cloudflare_timeout_cooldown_secs,
        cooldown_backoff,
        method,
        path,
        concurrency_permit,
    } = params;

    record_attempt_success(
        proxy.state.as_ref(),
        proxy.service_name,
        legacy_lb,
        target,
        crate::lb::COOLDOWN_SECS,
        cooldown_backoff,
    )
    .await;
    let duration_ms = start.elapsed().as_millis() as u64;
    record_status_route_attempt(
        upstream_chain,
        route_attempts,
        StatusRouteAttemptParams {
            target,
            route_attempt_index,
            status_code: status.as_u16(),
            error_class: None,
            reason: None,
            model_note,
            upstream_headers_ms,
            duration_ms,
            cooldown_secs: None,
            cooldown_reason: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
        },
    );
    record_session_route_affinity_success(
        proxy,
        session_id,
        session_identity_source,
        route_graph_key,
        target,
        route_attempts,
        route_attempt_index,
    )
    .await;
    let retry = retry_info_for_observed_attempts(upstream_chain, route_attempts);
    build_sse_success_response(
        proxy,
        legacy_lb.cloned(),
        target.clone(),
        response,
        SseSuccessMeta {
            status,
            resp_headers: response_headers,
            resp_headers_filtered: response_headers_filtered,
            start,
            started_at_ms,
            upstream_start,
            upstream_headers_ms,
            request_body_len,
            upstream_request_body_len,
            debug_base,
            retry,
            session_id: session_id.map(ToOwned::to_owned),
            session_identity_source,
            cwd: cwd.map(ToOwned::to_owned),
            effective_effort: effective_effort.map(ToOwned::to_owned),
            service_tier: base_service_tier.clone(),
            codex_bridge,
            route_decision: Some(route_decision_from_model_note(
                route_attempts,
                route_attempt_index,
            )),
            request_id,
            is_user_turn,
            is_codex_service,
            transport_cooldown_secs,
            cloudflare_challenge_cooldown_secs,
            cloudflare_timeout_cooldown_secs,
            cooldown_backoff,
            method: method.clone(),
            path: path.to_string(),
            concurrency_permit,
        },
    )
    .await
}

pub(super) async fn handle_attempt_response(
    params: AttemptResponseParams<'_>,
) -> AttemptResponseOutcome {
    let AttemptResponseParams {
        proxy,
        legacy_lb,
        target,
        method,
        path,
        status,
        response_headers,
        response_headers_filtered,
        response_body,
        request_id,
        duration_ms,
        started_at_ms,
        upstream_headers_ms,
        provider_id,
        session_id,
        session_identity_source,
        cwd,
        effective_effort,
        base_service_tier,
        codex_bridge,
        response_semantic_contract,
        route_graph_key,
        upstream_chain,
        route_attempts,
        route_attempt_index,
        model_note,
        plan,
        upstream_opt,
        provider_opt,
        upstream_attempt,
        avoid_set,
        avoided_total,
        last_err,
        cooldown_backoff,
        is_user_turn,
        allow_provider_failover,
        is_codex_service,
    } = params;

    let mut response_headers_filtered = response_headers_filtered;
    let mut response_status = status;
    let mut response_body = maybe_repair_codex_response_body(
        proxy.service_name,
        path,
        &response_headers,
        response_body,
    );
    let compact_sse_repair = maybe_repair_codex_compact_sse_response(
        proxy.service_name,
        path,
        &response_headers,
        &response_body,
    );
    let compact_protocol_failure = compact_sse_repair
        .as_ref()
        .is_some_and(|repair| matches!(repair, CodexCompactSseRepair::UpstreamFailureJson(_)));
    if let Some(repair) = compact_sse_repair {
        match repair {
            CodexCompactSseRepair::FinalJson(body) => {
                response_body = body;
                response_headers_filtered.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
            }
            CodexCompactSseRepair::UpstreamFailureJson(body) => {
                response_body = body;
                response_headers_filtered.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response_status = StatusCode::BAD_GATEWAY;
            }
        }
    }
    let mut response_body = if response_status.is_success() {
        maybe_decode_models_response_body(
            proxy.service_name,
            path,
            &response_headers,
            response_body,
        )
    } else {
        response_body
    };

    let semantic_failure = if response_status.is_success() {
        validate_success_response_semantics(response_semantic_contract, &response_body).err()
    } else {
        None
    };
    let mut semantic_error_class = None;
    if let Some(failure) = semantic_failure {
        let failure = *failure;
        semantic_error_class = Some(failure.error_class);
        response_status = failure.status;
        response_headers_filtered = failure.response_headers;
        response_body = failure.response_body;
        tracing::warn!(
            request_id,
            error_class = failure.error_class,
            message = failure.message.as_str(),
            "upstream response failed semantic validation"
        );
    }

    let success_usage = if response_status.is_success() {
        extract_usage_from_bytes(&response_body)
    } else {
        None
    };
    let mut route_attempt_reason = None;
    if response_status.is_success() {
        let guard_decision = evaluate_reasoning_guard(
            &plan.reasoning_guard,
            proxy.service_name,
            path,
            success_usage.as_ref(),
            reasoning_guard_retry_count(route_attempts),
        );
        if let Some(matched) = guard_decision.matched()
            && plan.reasoning_guard.log_matches
        {
            log_retry_trace(serde_json::json!({
                "event": "reasoning_guard_match",
                "service": proxy.service_name,
                "request_id": request_id,
                "path": path,
                "reasoning_tokens": matched.reasoning_tokens,
                "rule": matched.rule,
                "action": guard_decision.action_label(),
                "retryable": guard_decision.retryable(),
            }));
        }
        if let (Some(class), Some(matched)) =
            (guard_decision.failure_class(), guard_decision.matched())
        {
            semantic_error_class = Some(class);
            response_status = StatusCode::BAD_GATEWAY;
            response_headers_filtered.remove(header::CONTENT_LENGTH);
            response_headers_filtered.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response_body = reasoning_guard_error_body(matched, class, guard_decision.retryable());
            route_attempt_reason = Some(matched.rule.clone());
            tracing::warn!(
                request_id,
                error_class = class,
                reason = matched.rule.as_str(),
                action = guard_decision.action_label(),
                "upstream response failed reasoning guard"
            );
        }
    }

    let status_code = response_status.as_u16();
    let classified_response =
        classify_observed_upstream_response(status_code, &response_headers, response_body.as_ref());
    let retry_after_secs = classified_response.retry_after_secs();
    let cls = semantic_error_class
        .map(ToOwned::to_owned)
        .or_else(|| classified_response.class.clone());
    let observed_service_tier = extract_service_tier_from_response_body(response_body.as_ref());
    let decision = decide_attempt_response(AttemptResponseDecisionParams {
        plan,
        upstream_opt,
        provider_opt,
        status: response_status,
        class: cls.as_deref(),
        retry_after_secs,
        upstream_attempt,
        allow_provider_failover,
        compact_protocol_failure,
        is_user_turn,
        is_codex_service,
    });
    let penalty_cooldown_secs = decision.penalty_cooldown_secs;
    let cooldown_reason = cls
        .as_deref()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("status_{status_code}"));
    let route_facing_evidence =
        decision.provider_penalty && !class_is_health_neutral(cls.as_deref());
    let provider_evidence = response_evidence_from_classification(ResponseEvidenceParams {
        target,
        classified_response: &classified_response,
        status_code,
        error_class: cls.as_deref(),
        route_facing: route_facing_evidence,
        default_cooldown_secs: penalty_cooldown_secs,
    });
    record_status_route_attempt(
        upstream_chain,
        route_attempts,
        StatusRouteAttemptParams {
            target,
            route_attempt_index,
            status_code,
            error_class: cls.as_deref(),
            reason: route_attempt_reason.as_deref(),
            model_note,
            upstream_headers_ms,
            duration_ms,
            cooldown_secs: decision.provider_penalty.then_some(penalty_cooldown_secs),
            cooldown_reason: decision
                .provider_penalty
                .then_some(cooldown_reason.as_str()),
            provider_signals: provider_evidence.signals.clone(),
            policy_actions: provider_evidence.actions.clone(),
        },
    );

    if response_status.is_success() {
        record_attempt_success(
            proxy.state.as_ref(),
            proxy.service_name,
            legacy_lb,
            target,
            crate::lb::COOLDOWN_SECS,
            cooldown_backoff,
        )
        .await;
        if let Some(station_name) = target.compatibility_station_name() {
            record_passive_upstream_success(
                proxy.state.as_ref(),
                proxy.service_name,
                station_name,
                &target.upstream().base_url,
                status_code,
            )
            .await;
        }
        record_session_route_affinity_success(
            proxy,
            session_id,
            session_identity_source,
            route_graph_key,
            target,
            route_attempts,
            route_attempt_index,
        )
        .await;

        let usage = success_usage;
        let retry = retry_info_for_observed_attempts(upstream_chain, route_attempts);
        return AttemptResponseOutcome::Return(
            finish_attempt_forward_response(
                proxy,
                method,
                path,
                target,
                request_id,
                response_status,
                duration_ms,
                started_at_ms,
                upstream_headers_ms,
                provider_id,
                session_id,
                session_identity_source,
                cwd,
                effective_effort,
                base_service_tier,
                observed_service_tier,
                codex_bridge.clone(),
                usage,
                Some(route_decision_from_model_note(
                    route_attempts,
                    route_attempt_index,
                )),
                retry,
                response_headers_filtered,
                response_body,
            )
            .await,
        );
    }

    let response_text = summarize_upstream_error_body(&response_body, &response_headers);
    if decision.should_probe_codex_usage {
        enqueue_usage_probe_for_target(proxy, target).await;
    }
    if decision.never_retry {
        if !class_is_health_neutral(cls.as_deref()) {
            record_attempt_failure(
                proxy.state.as_ref(),
                proxy.service_name,
                legacy_lb,
                target,
                crate::lb::COOLDOWN_SECS,
                cooldown_backoff,
            )
            .await;
        }
        if let Some(station_name) = target.compatibility_station_name() {
            record_passive_upstream_failure(
                proxy.state.as_ref(),
                proxy.service_name,
                station_name,
                &target.upstream().base_url,
                Some(status_code),
                cls.as_deref(),
                Some(response_text),
            )
            .await;
        }

        let retry = retry_info_for_observed_attempts(upstream_chain, route_attempts);
        return AttemptResponseOutcome::Return(
            finish_attempt_forward_response(
                proxy,
                method,
                path,
                target,
                request_id,
                response_status,
                duration_ms,
                started_at_ms,
                upstream_headers_ms,
                provider_id,
                session_id,
                session_identity_source,
                cwd,
                effective_effort,
                base_service_tier,
                observed_service_tier,
                codex_bridge.clone(),
                None,
                Some(route_decision_from_model_note(
                    route_attempts,
                    route_attempt_index,
                )),
                retry,
                response_headers_filtered,
                response_body,
            )
            .await,
        );
    }

    if decision.retry_same_upstream {
        retry_sleep(
            upstream_opt,
            upstream_attempt,
            &response_headers,
            retry_after_secs,
        )
        .await;
        return AttemptResponseOutcome::RetrySameUpstream;
    }

    if decision.provider_penalty {
        if !class_is_health_neutral(cls.as_deref()) {
            provider_evidence
                .apply_to_state(proxy.service_name, proxy.state.as_ref())
                .await;
            penalize_attempt_target(
                proxy.state.as_ref(),
                proxy.service_name,
                legacy_lb,
                target,
                penalty_cooldown_secs,
                cooldown_reason.as_str(),
                cooldown_backoff,
            )
            .await;
        }
        if let Some(station_name) = target.compatibility_station_name() {
            record_passive_upstream_failure(
                proxy.state.as_ref(),
                proxy.service_name,
                station_name,
                &target.upstream().base_url,
                Some(status_code),
                cls.as_deref(),
                Some(response_text.clone()),
            )
            .await;
        }
        *last_err = Some((response_status, response_text));

        if decision.provider_failover {
            if avoid_set.insert(target.attempt_avoid_index()) {
                *avoided_total = avoided_total.saturating_add(1);
            }
            return AttemptResponseOutcome::TryNextUpstream;
        }

        let retry = retry_info_for_observed_attempts(upstream_chain, route_attempts);
        return AttemptResponseOutcome::Return(
            finish_attempt_forward_response(
                proxy,
                method,
                path,
                target,
                request_id,
                response_status,
                duration_ms,
                started_at_ms,
                upstream_headers_ms,
                provider_id,
                session_id,
                session_identity_source,
                cwd,
                effective_effort,
                base_service_tier,
                observed_service_tier,
                codex_bridge.clone(),
                None,
                Some(route_decision_from_model_note(
                    route_attempts,
                    route_attempt_index,
                )),
                retry,
                response_headers_filtered,
                response_body,
            )
            .await,
        );
    }

    let retry = retry_info_for_observed_attempts(upstream_chain, route_attempts);
    if let Some(station_name) = target.compatibility_station_name() {
        record_passive_upstream_failure(
            proxy.state.as_ref(),
            proxy.service_name,
            station_name,
            &target.upstream().base_url,
            Some(status_code),
            cls.as_deref(),
            Some(response_text),
        )
        .await;
    }

    AttemptResponseOutcome::Return(
        finish_attempt_forward_response(
            proxy,
            method,
            path,
            target,
            request_id,
            response_status,
            duration_ms,
            started_at_ms,
            upstream_headers_ms,
            provider_id,
            session_id,
            session_identity_source,
            cwd,
            effective_effort,
            base_service_tier,
            observed_service_tier,
            codex_bridge,
            None,
            Some(route_decision_from_model_note(
                route_attempts,
                route_attempt_index,
            )),
            retry,
            response_headers_filtered,
            response_body,
        )
        .await,
    )
}

async fn enqueue_usage_probe_for_target(proxy: &ProxyService, target: &AttemptTarget) {
    let cfg_snapshot = proxy.config.snapshot().await;
    if let Some(provider_endpoint) = target.provider_endpoint_ref().cloned() {
        usage_providers::enqueue_poll_for_codex_provider_endpoint(
            proxy.client.clone(),
            cfg_snapshot,
            proxy.lb_states.clone(),
            proxy.state.clone(),
            proxy.service_name,
            provider_endpoint,
        );
    } else if let (Some(station_name), Some(upstream_index)) = (
        target.compatibility_station_name().map(ToOwned::to_owned),
        target.compatibility_upstream_index(),
    ) {
        usage_providers::enqueue_poll_for_codex_upstream(
            proxy.client.clone(),
            cfg_snapshot,
            proxy.lb_states.clone(),
            proxy.state.clone(),
            proxy.service_name,
            &station_name,
            upstream_index,
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_attempt_forward_response(
    proxy: &ProxyService,
    method: &Method,
    path: &str,
    target: &AttemptTarget,
    request_id: u64,
    status: StatusCode,
    duration_ms: u64,
    started_at_ms: u64,
    upstream_headers_ms: u64,
    provider_id: Option<&str>,
    session_id: Option<&str>,
    session_identity_source: Option<SessionIdentitySource>,
    cwd: Option<&str>,
    effective_effort: Option<&str>,
    base_service_tier: &ServiceTierLog,
    observed_service_tier: Option<String>,
    codex_bridge: Option<CodexBridgeLog>,
    usage: Option<UsageMetrics>,
    route_decision: Option<crate::state::RouteDecisionProvenance>,
    retry: Option<crate::logging::RetryInfo>,
    response_headers: HeaderMap,
    response_body: Bytes,
) -> Response<Body> {
    let service_tier_for_log = ServiceTierLog {
        actual: observed_service_tier,
        ..base_service_tier.clone()
    };

    finish_and_build_forward_response(
        proxy,
        method,
        path,
        FinalizeForwardResponseParams {
            request_id,
            status,
            duration_ms,
            started_at_ms,
            upstream_headers_ms,
            station_name: target.compatibility_station_name().map(ToOwned::to_owned),
            provider_id: provider_id
                .map(ToOwned::to_owned)
                .or_else(|| target.provider_id().map(ToOwned::to_owned)),
            endpoint_id: target.endpoint_id(),
            provider_endpoint_key: target.provider_endpoint_key(),
            upstream_base_url: target.upstream().base_url.clone(),
            session_id: session_id.map(ToOwned::to_owned),
            session_identity_source,
            cwd: cwd.map(ToOwned::to_owned),
            effective_effort: effective_effort.map(ToOwned::to_owned),
            service_tier: service_tier_for_log,
            codex_bridge,
            usage,
            route_decision,
            retry,
            response_headers,
            response_body,
        },
    )
    .await
}

fn route_decision_from_model_note(
    route_attempts: &[RouteAttemptLog],
    route_attempt_index: usize,
) -> crate::state::RouteDecisionProvenance {
    route_attempts
        .get(route_attempt_index)
        .and_then(|attempt| attempt.model.as_deref())
        .filter(|model| *model != "-")
        .map(|model| crate::state::RouteDecisionProvenance {
            effective_model: Some(crate::state::ResolvedRouteValue {
                value: model.to_string(),
                source: crate::state::RouteValueSource::RuntimeFallback,
            }),
            ..crate::state::RouteDecisionProvenance::default()
        })
        .unwrap_or_default()
}
