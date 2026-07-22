use std::collections::HashSet;
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, Response, StatusCode, header};

use crate::endpoint_health::{CooldownBackoff, RouteCapability, RuntimeHealthDomain};
use crate::logging::{
    CodexBridgeLog, HttpDebugLog, RouteAttemptLog, ServiceTierLog, log_control_trace_event,
    make_body_preview, should_include_http_debug, should_include_http_warn, upstream_origin,
};
use crate::runtime_store::{AttemptHandle, AttemptOutcome, EconomicsState, RequestAccountingScope};
use crate::state::{
    DispatchedRuntimeHealthHalfOpenProbe, SessionIdentitySource, SessionRouteAffinitySuccess,
};
use crate::usage::{UsageMetrics, extract_usage_from_bytes};

use super::ProxyService;
use super::attempt_health::{
    settle_half_open_probe_neutral, settle_or_penalize_attempt_target,
    settle_or_record_attempt_failure, settle_or_record_attempt_success,
};
use super::classify::{
    UPSTREAM_OVERLOADED_CLASS, class_is_health_neutral, classify_observed_upstream_response,
    is_buffered_http_credential_auth_failure,
};
use super::concurrency_limits::ConcurrencyPermit;
use super::http_debug::{HttpDebugBase, HttpDebugResponseParams, warn_http_debug};
use super::models_compat::{ModelsTranslationScope, maybe_decode_models_response_body};
use super::provider_evidence::{ResponseEvidenceParams, response_evidence_from_classification};
use super::reasoning_guard::{
    REASONING_GUARD_BLOCKED_CLASS, evaluate_reasoning_guard, reasoning_guard_error_body,
    reasoning_guard_retry_count,
};
use super::request_body::{
    extract_model_from_response_body, extract_service_tier_from_response_body,
};
use super::request_preparation::{
    RequestReplayPolicy, SharedRouteStateImpact, StreamTerminalPolicy,
};
use super::response_entity::UpstreamResponseEntity;
use super::response_finalization::{
    FinalizeForwardResponseParams, FinalizedForwardResponse, finish_and_build_forward_response,
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
use super::route_affinity::prepare_session_route_affinity_success;
use super::route_attempts::{
    StatusRouteAttemptParams, record_http_debug_route_attempt, record_status_route_attempt,
};
use super::stream::{SseSuccessMeta, build_sse_success_response};
use crate::routing_ir::CapturedRouteCandidate;

pub(super) enum AttemptResponseOutcome {
    RetrySameUpstream,
    TryNextUpstream,
    StopProviderChain,
    Return(Response<Body>),
}

pub(super) struct AttemptResponseParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) method: &'a Method,
    pub(super) path: &'a str,
    pub(super) status: StatusCode,
    pub(super) upstream_entity: UpstreamResponseEntity,
    pub(super) response_headers: HeaderMap,
    pub(super) response_headers_filtered: HeaderMap,
    pub(super) response_body: Bytes,
    pub(super) models_translation: ModelsTranslationScope<'a>,
    pub(super) attempt_handle: AttemptHandle,
    pub(super) request_id: u64,
    pub(super) duration_ms: u64,
    pub(super) started_at_ms: u64,
    pub(super) upstream_headers_ms: u64,
    pub(super) upstream_body_read_ms: u64,
    pub(super) debug_base: Option<HttpDebugBase>,
    pub(super) provider_id: Option<&'a str>,
    pub(super) session_id: Option<&'a str>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) cwd: Option<&'a str>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) base_service_tier: &'a ServiceTierLog,
    pub(super) codex_bridge: Option<CodexBridgeLog>,
    pub(super) response_semantic_contract: Option<ResponseSemanticContract>,
    pub(super) route_graph_key: Option<&'a str>,
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
    pub(super) last_http_debug: &'a mut Option<HttpDebugLog>,
    pub(super) cooldown_backoff: CooldownBackoff,
    pub(super) is_user_turn: bool,
    pub(super) allow_provider_failover: bool,
    pub(super) is_codex_service: bool,
    pub(super) shared_route_state_impact: SharedRouteStateImpact,
    pub(super) terminal_accounting: RequestAccountingScope,
    pub(super) route_capability: RouteCapability,
    pub(super) replay_policy: RequestReplayPolicy,
    pub(super) half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
}

pub(super) struct StreamingAttemptResponseParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) response: reqwest::Response,
    pub(super) status: StatusCode,
    pub(super) response_headers: HeaderMap,
    pub(super) response_headers_filtered: HeaderMap,
    pub(super) start: Instant,
    pub(super) started_at_ms: u64,
    pub(super) upstream_start: Instant,
    pub(super) upstream_headers_ms: u64,
    pub(super) debug_base: Option<HttpDebugBase>,
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
    pub(super) attempt_handle: AttemptHandle,
    pub(super) route_capability: RouteCapability,
    pub(super) shared_route_state_impact: SharedRouteStateImpact,
    pub(super) terminal_accounting: RequestAccountingScope,
    pub(super) stream_terminal_policy: StreamTerminalPolicy,
    pub(super) half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
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
    replay_policy: RequestReplayPolicy,
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
        replay_policy,
    } = params;
    let status_code = status.as_u16();
    let auth_failure = is_buffered_http_credential_auth_failure(status, class);
    let reasoning_guard_blocked = matches!(class, Some(REASONING_GUARD_BLOCKED_CLASS));
    let never_retry = !replay_policy.allows_after_dispatch()
        || (auth_failure && !replay_policy.allows_credential_failover())
        || should_never_retry(plan, status_code, class)
        || compact_protocol_failure
        || reasoning_guard_blocked;
    let semantic_failure_requires_provider_failover =
        matches!(class, Some(IMAGE_GENERATION_MISSING_RESULT_CLASS));
    let same_upstream_retryable_class =
        !auth_failure && !matches!(class, Some(UPSTREAM_OVERLOADED_CLASS));
    let upstream_retryable = !never_retry
        && !semantic_failure_requires_provider_failover
        && same_upstream_retryable_class
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
        target,
        response,
        status,
        response_headers,
        response_headers_filtered,
        start,
        started_at_ms,
        upstream_start,
        upstream_headers_ms,
        debug_base,
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
        attempt_handle,
        route_capability,
        shared_route_state_impact,
        terminal_accounting,
        stream_terminal_policy,
        half_open_probe,
    } = params;

    let duration_ms = start.elapsed().as_millis() as u64;
    record_status_route_attempt(
        route_attempts,
        StatusRouteAttemptParams {
            target,
            route_attempt_index,
            status_code: status.as_u16(),
            error_class: None,
            model_note,
            upstream_headers_ms,
            duration_ms,
            cooldown_secs: None,
            cooldown_reason: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
        },
    );
    let route_affinity_success = if shared_route_state_impact.allows_shared_updates() {
        prepare_session_route_affinity_success(
            request_id,
            session_id,
            session_identity_source,
            route_graph_key,
            target,
            route_attempts,
            route_attempt_index,
        )
    } else {
        None
    };
    let retry = retry_info_for_observed_attempts(route_attempts);
    build_sse_success_response(
        proxy,
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
            attempt_handle,
            route_affinity_success,
            route_capability,
            shared_route_state_impact,
            terminal_accounting,
            stream_terminal_policy,
            half_open_probe,
        },
    )
    .await
}

pub(super) async fn handle_attempt_response(
    params: AttemptResponseParams<'_>,
) -> AttemptResponseOutcome {
    let AttemptResponseParams {
        proxy,
        target,
        method,
        path,
        status,
        upstream_entity,
        response_headers,
        response_headers_filtered,
        response_body,
        models_translation,
        attempt_handle,
        request_id,
        duration_ms,
        started_at_ms,
        upstream_headers_ms,
        upstream_body_read_ms,
        debug_base,
        provider_id,
        session_id,
        session_identity_source,
        cwd,
        effective_effort,
        base_service_tier,
        codex_bridge,
        response_semantic_contract,
        route_graph_key,
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
        last_http_debug,
        cooldown_backoff,
        is_user_turn,
        allow_provider_failover,
        is_codex_service,
        shared_route_state_impact,
        terminal_accounting,
        route_capability,
        replay_policy,
        mut half_open_probe,
    } = params;

    let upstream_status = status;
    let upstream_response_body = response_body.clone();
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
            models_translation,
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
            log_control_trace_event(serde_json::json!({
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
            tracing::warn!(
                request_id,
                error_class = class,
                reason = matched.rule.as_str(),
                action = guard_decision.action_label(),
                "upstream response failed reasoning guard"
            );
        }
    }

    upstream_entity.reconcile_headers(
        response_status,
        &response_body,
        &mut response_headers_filtered,
    );

    let status_code = response_status.as_u16();
    let classified_response =
        classify_observed_upstream_response(status_code, &response_headers, response_body.as_ref());
    let retry_after_secs = classified_response.retry_after_secs();
    let cls = semantic_error_class
        .map(ToOwned::to_owned)
        .or_else(|| classified_response.class.clone());
    let auth_failure = is_buffered_http_credential_auth_failure(response_status, cls.as_deref());
    if auth_failure && shared_route_state_impact.allows_shared_updates() {
        proxy
            .config
            .schedule_credential_refresh(target.credential());
    }
    let observed_service_tier = extract_service_tier_from_response_body(response_body.as_ref());
    let reported_model = extract_model_from_response_body(response_body.as_ref());
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
        replay_policy,
    });
    let penalty_cooldown_secs = decision.penalty_cooldown_secs;
    let cooldown_reason = cls
        .as_deref()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("status_{status_code}"));
    let shared_route_updates_allowed = shared_route_state_impact.allows_shared_updates();
    let response_health_domain = if !shared_route_updates_allowed {
        None
    } else if auth_failure {
        Some(RuntimeHealthDomain::Credential)
    } else if class_is_health_neutral(cls.as_deref()) {
        None
    } else if matches!(cls.as_deref(), Some(UPSTREAM_OVERLOADED_CLASS)) {
        Some(RuntimeHealthDomain::Capacity(route_capability))
    } else {
        Some(RuntimeHealthDomain::Capability(route_capability))
    };
    let applies_immediate_cooldown = response_health_domain
        .is_some_and(|domain| !matches!(domain, RuntimeHealthDomain::Capacity(_)));
    let credential_penalty = response_health_domain == Some(RuntimeHealthDomain::Credential);
    let applies_health_cooldown =
        credential_penalty || (applies_immediate_cooldown && decision.provider_penalty);
    let route_facing_evidence =
        response_health_domain.is_some() && (decision.provider_penalty || credential_penalty);
    let provider_evidence = response_evidence_from_classification(ResponseEvidenceParams {
        target,
        classified_response: &classified_response,
        status_code,
        error_class: cls.as_deref(),
        route_facing: route_facing_evidence,
    });
    let http_debug_warn = should_include_http_warn(response_status.as_u16())
        .then(|| {
            debug_base.as_ref()?.response_log(HttpDebugResponseParams {
                status_code: upstream_status.as_u16(),
                response_headers: &response_headers,
                response_body: upstream_response_body.as_ref(),
                response_preview_body: None,
                upstream_headers_ms,
                upstream_first_chunk_ms: None,
                upstream_body_read_ms: Some(upstream_body_read_ms),
                for_warn: true,
            })
        })
        .flatten();
    if let Some(http_debug_warn) = http_debug_warn.as_ref() {
        warn_http_debug(response_status.as_u16(), http_debug_warn);
    }
    let http_debug = should_include_http_debug(response_status.as_u16())
        .then(|| {
            debug_base.as_ref()?.response_log(HttpDebugResponseParams {
                status_code: upstream_status.as_u16(),
                response_headers: &response_headers,
                response_body: upstream_response_body.as_ref(),
                response_preview_body: None,
                upstream_headers_ms,
                upstream_first_chunk_ms: None,
                upstream_body_read_ms: Some(upstream_body_read_ms),
                for_warn: false,
            })
        })
        .flatten();
    record_status_route_attempt(
        route_attempts,
        StatusRouteAttemptParams {
            target,
            route_attempt_index,
            status_code,
            error_class: cls.as_deref(),
            model_note,
            upstream_headers_ms,
            duration_ms,
            cooldown_secs: applies_health_cooldown.then_some(penalty_cooldown_secs),
            cooldown_reason: applies_health_cooldown.then_some(cooldown_reason.as_str()),
            provider_signals: provider_evidence.signals.clone(),
            policy_actions: Vec::new(),
        },
    );
    record_http_debug_route_attempt(route_attempts, route_attempt_index, http_debug.as_ref());

    if let Err(error) = proxy.state.finish_upstream_attempt(
        attempt_handle,
        if response_status.is_success() {
            AttemptOutcome::Succeeded
        } else {
            AttemptOutcome::Failed
        },
        crate::logging::now_ms(),
        EconomicsState::Unknown,
    ) {
        *last_err = Some((StatusCode::INTERNAL_SERVER_ERROR, error.to_string()));
        return AttemptResponseOutcome::StopProviderChain;
    }

    if response_status.is_success() {
        let usage = success_usage;
        let retry = retry_info_for_observed_attempts(route_attempts);
        let route_affinity_success = prepare_session_route_affinity_success(
            request_id,
            session_id,
            session_identity_source,
            route_graph_key,
            target,
            route_attempts,
            route_attempt_index,
        );
        let finalized = finish_attempt_forward_response(
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
            reported_model,
            attempt_handle,
            codex_bridge.clone(),
            usage,
            Some(route_decision_from_model_note(
                route_attempts,
                route_attempt_index,
            )),
            retry,
            route_affinity_success,
            terminal_accounting,
            http_debug,
            response_headers_filtered,
            response_body,
        )
        .await;
        if finalized.terminal_published && shared_route_updates_allowed {
            settle_or_record_attempt_success(
                proxy.state.as_ref(),
                proxy.service_name,
                target,
                route_capability,
                half_open_probe.take(),
            )
            .await;
        } else {
            settle_half_open_probe_neutral(proxy.state.as_ref(), half_open_probe.take()).await;
        }
        return AttemptResponseOutcome::Return(finalized.response);
    }

    let response_text = summarize_upstream_error_body(&response_body, &response_headers);
    if decision.should_probe_codex_usage && shared_route_updates_allowed {
        enqueue_usage_probe_for_target(proxy, target).await;
    }
    if decision.never_retry {
        if let Some(health_domain) = response_health_domain {
            if health_domain == RuntimeHealthDomain::Credential {
                settle_or_penalize_attempt_target(
                    proxy.state.as_ref(),
                    proxy.service_name,
                    target,
                    health_domain,
                    penalty_cooldown_secs,
                    cooldown_backoff,
                    half_open_probe.take(),
                )
                .await;
            } else {
                settle_or_record_attempt_failure(
                    proxy.state.as_ref(),
                    proxy.service_name,
                    target,
                    health_domain,
                    crate::endpoint_health::COOLDOWN_SECS,
                    cooldown_backoff,
                    half_open_probe.take(),
                )
                .await;
            }
        }
        settle_half_open_probe_neutral(proxy.state.as_ref(), half_open_probe.take()).await;
        let retry = retry_info_for_observed_attempts(route_attempts);
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
                reported_model,
                attempt_handle,
                codex_bridge.clone(),
                None,
                Some(route_decision_from_model_note(
                    route_attempts,
                    route_attempt_index,
                )),
                retry,
                None,
                terminal_accounting,
                http_debug,
                response_headers_filtered,
                response_body,
            )
            .await
            .response,
        );
    }

    if decision.retry_same_upstream {
        settle_half_open_probe_neutral(proxy.state.as_ref(), half_open_probe.take()).await;
        *last_http_debug = http_debug;
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
        if let Some(health_domain) = response_health_domain {
            if matches!(health_domain, RuntimeHealthDomain::Capacity(_)) {
                settle_or_record_attempt_failure(
                    proxy.state.as_ref(),
                    proxy.service_name,
                    target,
                    health_domain,
                    penalty_cooldown_secs,
                    cooldown_backoff,
                    half_open_probe.take(),
                )
                .await;
            } else {
                settle_or_penalize_attempt_target(
                    proxy.state.as_ref(),
                    proxy.service_name,
                    target,
                    health_domain,
                    penalty_cooldown_secs,
                    cooldown_backoff,
                    half_open_probe.take(),
                )
                .await;
            }
        }
        settle_half_open_probe_neutral(proxy.state.as_ref(), half_open_probe.take()).await;
        *last_err = Some((response_status, response_text));

        if decision.provider_failover {
            *last_http_debug = http_debug;
            if avoid_set.insert(target.attempt_avoid_index()) {
                *avoided_total = avoided_total.saturating_add(1);
            }
            return AttemptResponseOutcome::TryNextUpstream;
        }

        let retry = retry_info_for_observed_attempts(route_attempts);
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
                reported_model,
                attempt_handle,
                codex_bridge.clone(),
                None,
                Some(route_decision_from_model_note(
                    route_attempts,
                    route_attempt_index,
                )),
                retry,
                None,
                terminal_accounting,
                http_debug,
                response_headers_filtered,
                response_body,
            )
            .await
            .response,
        );
    }

    settle_half_open_probe_neutral(proxy.state.as_ref(), half_open_probe.take()).await;
    let retry = retry_info_for_observed_attempts(route_attempts);
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
            reported_model,
            attempt_handle,
            codex_bridge,
            None,
            Some(route_decision_from_model_note(
                route_attempts,
                route_attempt_index,
            )),
            retry,
            None,
            terminal_accounting,
            http_debug,
            response_headers_filtered,
            response_body,
        )
        .await
        .response,
    )
}

async fn enqueue_usage_probe_for_target(proxy: &ProxyService, target: &CapturedRouteCandidate) {
    let provider_catalog = proxy.config.capture().await.usage_provider_catalog();
    super::providers_api::enqueue_provider_balance_probe(
        proxy.client.clone(),
        proxy.state.clone(),
        target.clone(),
        provider_catalog,
    );
}

#[allow(clippy::too_many_arguments)]
async fn finish_attempt_forward_response(
    proxy: &ProxyService,
    method: &Method,
    path: &str,
    target: &CapturedRouteCandidate,
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
    reported_model: Option<String>,
    attempt_handle: AttemptHandle,
    codex_bridge: Option<CodexBridgeLog>,
    usage: Option<UsageMetrics>,
    route_decision: Option<crate::state::RouteDecisionProvenance>,
    retry: Option<crate::logging::RetryInfo>,
    route_affinity_success: Option<SessionRouteAffinitySuccess>,
    terminal_accounting: RequestAccountingScope,
    http_debug: Option<HttpDebugLog>,
    response_headers: HeaderMap,
    response_body: Bytes,
) -> FinalizedForwardResponse {
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
            winning_attempt: status.is_success().then_some(attempt_handle),
            status,
            duration_ms,
            started_at_ms,
            upstream_headers_ms,
            provider_id: provider_id
                .map(ToOwned::to_owned)
                .or_else(|| Some(target.provider_id().to_owned())),
            endpoint_id: Some(target.endpoint_id().to_owned()),
            provider_endpoint_key: Some(target.provider_endpoint_key()),
            upstream_origin: upstream_origin(target.base_url()),
            session_id: session_id.map(ToOwned::to_owned),
            session_identity_source,
            cwd: cwd.map(ToOwned::to_owned),
            effective_effort: effective_effort.map(ToOwned::to_owned),
            service_tier: service_tier_for_log,
            reported_model,
            codex_bridge,
            usage,
            route_decision,
            retry,
            route_affinity_success,
            terminal_accounting,
            http_debug,
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
