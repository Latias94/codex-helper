use std::collections::HashSet;
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode};

use crate::lb::{CooldownBackoff, LoadBalancer, SelectedUpstream};
use crate::logging::ServiceTierLog;
use crate::usage::{UsageMetrics, extract_usage_from_bytes};

use super::ProxyService;
use super::classify::{class_is_health_neutral, classify_upstream_response};
use super::http_debug::HttpDebugBase;
use super::passive_health::{record_passive_upstream_failure, record_passive_upstream_success};
use super::request_body::extract_service_tier_from_response_body;
use super::response_finalization::{
    FinalizeForwardResponseParams, finish_and_build_forward_response,
};
use super::retry::{
    RetryLayerOptions, RetryPlan, retry_info_for_chain, retry_sleep, should_never_retry,
    should_retry_class, should_retry_status,
};
use super::stream::{SseSuccessMeta, build_sse_success_response};

pub(super) enum AttemptResponseOutcome {
    RetrySameUpstream,
    TryNextUpstream,
    Return(Response<Body>),
}

pub(super) struct AttemptResponseParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) lb: &'a LoadBalancer,
    pub(super) selected: &'a SelectedUpstream,
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
    pub(super) cwd: Option<&'a str>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) base_service_tier: &'a ServiceTierLog,
    pub(super) upstream_chain: &'a mut Vec<String>,
    pub(super) model_note: &'a str,
    pub(super) plan: &'a RetryPlan,
    pub(super) upstream_opt: &'a RetryLayerOptions,
    pub(super) provider_opt: &'a RetryLayerOptions,
    pub(super) upstream_attempt: u32,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
    pub(super) cooldown_backoff: CooldownBackoff,
}

pub(super) struct StreamingAttemptResponseParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) lb: &'a LoadBalancer,
    pub(super) selected: &'a SelectedUpstream,
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
    pub(super) upstream_chain: &'a [String],
    pub(super) session_id: Option<&'a str>,
    pub(super) cwd: Option<&'a str>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) base_service_tier: &'a ServiceTierLog,
    pub(super) request_id: u64,
    pub(super) is_user_turn: bool,
    pub(super) is_codex_service: bool,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cooldown_backoff: CooldownBackoff,
    pub(super) method: &'a Method,
    pub(super) path: &'a str,
}

pub(super) async fn handle_streaming_attempt_success(
    params: StreamingAttemptResponseParams<'_>,
) -> Response<Body> {
    let StreamingAttemptResponseParams {
        proxy,
        lb,
        selected,
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
        session_id,
        cwd,
        effective_effort,
        base_service_tier,
        request_id,
        is_user_turn,
        is_codex_service,
        transport_cooldown_secs,
        cooldown_backoff,
        method,
        path,
    } = params;

    lb.record_result_with_backoff(
        selected.index,
        true,
        crate::lb::COOLDOWN_SECS,
        cooldown_backoff,
    );
    let retry = retry_info_for_chain(upstream_chain);
    build_sse_success_response(
        proxy,
        lb.clone(),
        selected.clone(),
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
            cwd: cwd.map(ToOwned::to_owned),
            effective_effort: effective_effort.map(ToOwned::to_owned),
            service_tier: base_service_tier.clone(),
            request_id,
            is_user_turn,
            is_codex_service,
            transport_cooldown_secs,
            cooldown_backoff,
            method: method.clone(),
            path: path.to_string(),
        },
    )
    .await
}

pub(super) async fn handle_attempt_response(
    params: AttemptResponseParams<'_>,
) -> AttemptResponseOutcome {
    let AttemptResponseParams {
        proxy,
        lb,
        selected,
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
        cwd,
        effective_effort,
        base_service_tier,
        upstream_chain,
        model_note,
        plan,
        upstream_opt,
        provider_opt,
        upstream_attempt,
        avoid_set,
        avoided_total,
        last_err,
        cooldown_backoff,
    } = params;

    let status_code = status.as_u16();
    let (cls, _hint, _cf_ray) =
        classify_upstream_response(status_code, &response_headers, response_body.as_ref());
    let never_retry = should_never_retry(plan, status_code, cls.as_deref());
    let observed_service_tier = extract_service_tier_from_response_body(response_body.as_ref());

    upstream_chain.push(format!(
        "{} (idx={}) status={} class={} model={}",
        selected.upstream.base_url,
        selected.index,
        status_code,
        cls.as_deref().unwrap_or("-"),
        model_note
    ));

    if status.is_success() {
        lb.record_result_with_backoff(
            selected.index,
            true,
            crate::lb::COOLDOWN_SECS,
            cooldown_backoff,
        );
        record_passive_upstream_success(
            proxy.state.as_ref(),
            proxy.service_name,
            &selected.station_name,
            &selected.upstream.base_url,
            status_code,
        )
        .await;

        let usage = extract_usage_from_bytes(&response_body);
        let retry = retry_info_for_chain(upstream_chain);
        return AttemptResponseOutcome::Return(
            finish_attempt_forward_response(
                proxy,
                method,
                path,
                selected,
                request_id,
                status,
                duration_ms,
                started_at_ms,
                upstream_headers_ms,
                provider_id,
                session_id,
                cwd,
                effective_effort,
                base_service_tier,
                observed_service_tier,
                usage,
                retry,
                response_headers_filtered,
                response_body,
            )
            .await,
        );
    }

    let response_text = String::from_utf8_lossy(response_body.as_ref()).to_string();
    if never_retry {
        if !class_is_health_neutral(cls.as_deref()) {
            lb.record_result_with_backoff(
                selected.index,
                false,
                crate::lb::COOLDOWN_SECS,
                cooldown_backoff,
            );
        }
        record_passive_upstream_failure(
            proxy.state.as_ref(),
            proxy.service_name,
            &selected.station_name,
            &selected.upstream.base_url,
            Some(status_code),
            cls.as_deref(),
            Some(response_text),
        )
        .await;

        let retry = retry_info_for_chain(upstream_chain);
        return AttemptResponseOutcome::Return(
            finish_attempt_forward_response(
                proxy,
                method,
                path,
                selected,
                request_id,
                status,
                duration_ms,
                started_at_ms,
                upstream_headers_ms,
                provider_id,
                session_id,
                cwd,
                effective_effort,
                base_service_tier,
                observed_service_tier,
                None,
                retry,
                response_headers_filtered,
                response_body,
            )
            .await,
        );
    }

    let upstream_retryable = should_retry_status(upstream_opt, status_code)
        || should_retry_class(upstream_opt, cls.as_deref());
    if upstream_retryable && upstream_attempt + 1 < upstream_opt.max_attempts {
        retry_sleep(upstream_opt, upstream_attempt, &response_headers).await;
        return AttemptResponseOutcome::RetrySameUpstream;
    }

    let provider_retryable = should_retry_status(provider_opt, status_code)
        || should_retry_class(provider_opt, cls.as_deref());
    if provider_retryable {
        if !class_is_health_neutral(cls.as_deref()) {
            let penalty_reason = format!("status_{status_code}");
            lb.penalize_with_backoff(
                selected.index,
                plan.transport_cooldown_secs,
                penalty_reason.as_str(),
                cooldown_backoff,
            );
        }
        record_passive_upstream_failure(
            proxy.state.as_ref(),
            proxy.service_name,
            &selected.station_name,
            &selected.upstream.base_url,
            Some(status_code),
            cls.as_deref(),
            Some(response_text.clone()),
        )
        .await;
        *last_err = Some((status, response_text));

        if avoid_set.insert(selected.index) {
            *avoided_total = avoided_total.saturating_add(1);
        }
        return AttemptResponseOutcome::TryNextUpstream;
    }

    let retry = retry_info_for_chain(upstream_chain);
    record_passive_upstream_failure(
        proxy.state.as_ref(),
        proxy.service_name,
        &selected.station_name,
        &selected.upstream.base_url,
        Some(status_code),
        cls.as_deref(),
        Some(response_text),
    )
    .await;

    AttemptResponseOutcome::Return(
        finish_attempt_forward_response(
            proxy,
            method,
            path,
            selected,
            request_id,
            status,
            duration_ms,
            started_at_ms,
            upstream_headers_ms,
            provider_id,
            session_id,
            cwd,
            effective_effort,
            base_service_tier,
            observed_service_tier,
            None,
            retry,
            response_headers_filtered,
            response_body,
        )
        .await,
    )
}

#[allow(clippy::too_many_arguments)]
async fn finish_attempt_forward_response(
    proxy: &ProxyService,
    method: &Method,
    path: &str,
    selected: &SelectedUpstream,
    request_id: u64,
    status: StatusCode,
    duration_ms: u64,
    started_at_ms: u64,
    upstream_headers_ms: u64,
    provider_id: Option<&str>,
    session_id: Option<&str>,
    cwd: Option<&str>,
    effective_effort: Option<&str>,
    base_service_tier: &ServiceTierLog,
    observed_service_tier: Option<String>,
    usage: Option<UsageMetrics>,
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
            station_name: selected.station_name.clone(),
            provider_id: provider_id.map(ToOwned::to_owned),
            upstream_base_url: selected.upstream.base_url.clone(),
            session_id: session_id.map(ToOwned::to_owned),
            cwd: cwd.map(ToOwned::to_owned),
            effective_effort: effective_effort.map(ToOwned::to_owned),
            service_tier: service_tier_for_log,
            usage,
            retry,
            response_headers,
            response_body,
        },
    )
    .await
}
