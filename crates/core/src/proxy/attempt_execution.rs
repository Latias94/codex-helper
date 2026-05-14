use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode, Uri};

use crate::lb::{CooldownBackoff, LoadBalancer};
use crate::logging::{BodyPreview, HeaderEntry, RouteAttemptLog, ServiceTierLog, log_retry_trace};
use crate::state::SessionBinding;

use super::ProxyService;
use super::attempt_response::{
    AttemptResponseOutcome, AttemptResponseParams, StreamingAttemptResponseParams,
    handle_attempt_response, handle_streaming_attempt_success,
};
use super::attempt_target::AttemptTarget;
use super::attempt_transport::{
    AttemptReadBodyOutcome, AttemptReadBodyParams, AttemptTransportOutcome, AttemptTransportParams,
    handle_attempt_transport, read_attempt_response_body,
};
use super::headers::filter_response_headers;
use super::request_preparation::RequestFlavor;
use super::retry::{RetryLayerOptions, RetryPlan};
use super::route_attempts::{StartRouteAttemptParams, start_selected_route_attempt};
use super::selected_upstream_request::{
    SelectedUpstreamRequestSetupParams, prepare_selected_upstream_request,
};

pub(super) enum SelectedUpstreamExecutionOutcome {
    ContinueStation,
    Return(Response<Body>),
}

struct AttemptSelectLogParams<'a> {
    service_name: &'a str,
    request_id: u64,
    global_attempt: u32,
    provider_attempt: u32,
    upstream_attempt: u32,
    provider_opt: &'a RetryLayerOptions,
    upstream_opt: &'a RetryLayerOptions,
    target: &'a AttemptTarget,
    provider_id: Option<&'a str>,
    avoid_set: &'a HashSet<usize>,
    avoided_total: usize,
    total_upstreams: usize,
    model_note: &'a str,
}

pub(super) struct ExecuteSelectedUpstreamParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) legacy_lb: Option<&'a LoadBalancer>,
    pub(super) target: &'a AttemptTarget,
    pub(super) method: &'a Method,
    pub(super) uri: &'a Uri,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) client_headers_entries_cache: &'a OnceLock<Vec<HeaderEntry>>,
    pub(super) client_uri: &'a str,
    pub(super) start: &'a Instant,
    pub(super) started_at_ms: u64,
    pub(super) request_id: u64,
    pub(super) request_body_len: usize,
    pub(super) body_for_upstream: &'a Bytes,
    pub(super) request_model: Option<&'a str>,
    pub(super) session_binding: Option<&'a SessionBinding>,
    pub(super) session_override_config: Option<&'a str>,
    pub(super) global_station_override: Option<&'a str>,
    pub(super) override_model: Option<&'a str>,
    pub(super) override_effort: Option<&'a str>,
    pub(super) override_service_tier: Option<&'a str>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) effective_service_tier: Option<&'a str>,
    pub(super) base_service_tier: &'a ServiceTierLog,
    pub(super) session_id: Option<&'a str>,
    pub(super) cwd: Option<&'a str>,
    pub(super) request_flavor: &'a RequestFlavor,
    pub(super) request_body_previews: bool,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
    pub(super) client_body_debug: Option<&'a BodyPreview>,
    pub(super) client_body_warn: Option<&'a BodyPreview>,
    pub(super) plan: &'a RetryPlan,
    pub(super) route_graph_key: Option<&'a str>,
    pub(super) upstream_opt: &'a RetryLayerOptions,
    pub(super) provider_opt: &'a RetryLayerOptions,
    pub(super) provider_attempt: u32,
    pub(super) total_upstreams: usize,
    pub(super) cooldown_backoff: CooldownBackoff,
    pub(super) global_attempt: &'a mut u32,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
    pub(super) upstream_chain: &'a mut Vec<String>,
    pub(super) route_attempts: &'a mut Vec<RouteAttemptLog>,
}

pub(super) async fn execute_selected_upstream(
    params: ExecuteSelectedUpstreamParams<'_>,
) -> SelectedUpstreamExecutionOutcome {
    let ExecuteSelectedUpstreamParams {
        proxy,
        legacy_lb,
        target,
        method,
        uri,
        client_headers,
        client_headers_entries_cache,
        client_uri,
        start,
        started_at_ms,
        request_id,
        request_body_len,
        body_for_upstream,
        request_model,
        session_binding,
        session_override_config,
        global_station_override,
        override_model,
        override_effort,
        override_service_tier,
        effective_effort,
        effective_service_tier,
        base_service_tier,
        session_id,
        cwd,
        request_flavor,
        request_body_previews,
        debug_max,
        warn_max,
        client_body_debug,
        client_body_warn,
        plan,
        route_graph_key,
        upstream_opt,
        provider_opt,
        provider_attempt,
        total_upstreams,
        cooldown_backoff,
        global_attempt,
        avoid_set,
        avoided_total,
        last_err,
        upstream_chain,
        route_attempts,
    } = params;

    let selected_setup = prepare_selected_upstream_request(SelectedUpstreamRequestSetupParams {
        proxy,
        target,
        body_for_upstream,
        request_model,
        session_binding,
        session_override_config,
        global_station_override,
        override_model,
        override_effort,
        override_service_tier,
        effective_effort,
        effective_service_tier,
        client_content_type: request_flavor.client_content_type.as_deref(),
        request_body_previews,
        debug_max,
        warn_max,
    });
    let model_note = selected_setup.model_note;
    let provider_id = selected_setup.provider_id;
    let route_decision = selected_setup.route_decision;
    let filtered_body = selected_setup.filtered_body;
    let upstream_request_body_len = selected_setup.upstream_request_body_len;
    let upstream_request_body_debug = selected_setup.upstream_request_body_debug;
    let upstream_request_body_warn = selected_setup.upstream_request_body_warn;

    for upstream_attempt in 0..upstream_opt.max_attempts {
        *global_attempt = global_attempt.saturating_add(1);
        log_attempt_select(AttemptSelectLogParams {
            service_name: proxy.service_name,
            request_id,
            global_attempt: *global_attempt,
            provider_attempt,
            upstream_attempt,
            provider_opt,
            upstream_opt,
            target,
            provider_id: provider_id.as_deref(),
            avoid_set,
            avoided_total: *avoided_total,
            total_upstreams,
            model_note: model_note.as_str(),
        });
        let route_attempt_index = start_selected_route_attempt(
            route_attempts,
            StartRouteAttemptParams {
                target,
                provider_id: provider_id.as_deref(),
                provider_attempt,
                upstream_attempt,
                provider_max_attempts: provider_opt.max_attempts,
                upstream_max_attempts: upstream_opt.max_attempts,
                model_note: model_note.as_str(),
                avoid_set,
                avoided_total: *avoided_total,
                total_upstreams,
            },
        );

        let transport = handle_attempt_transport(AttemptTransportParams {
            proxy,
            legacy_lb,
            target,
            method,
            uri,
            client_headers,
            client_headers_entries_cache,
            request_body_len,
            upstream_request_body_len,
            debug_max,
            warn_max,
            client_uri,
            client_body_debug,
            upstream_request_body_debug: upstream_request_body_debug.as_ref(),
            client_body_warn,
            upstream_request_body_warn: upstream_request_body_warn.as_ref(),
            request_id,
            provider_id: provider_id.as_deref(),
            route_decision: &route_decision,
            filtered_body: &filtered_body,
            upstream_opt,
            upstream_attempt,
            transport_cooldown_secs: plan.transport_cooldown_secs,
            cooldown_backoff,
            avoid_set,
            avoided_total,
            last_err,
            upstream_chain,
            route_attempts,
            route_attempt_index,
            model_note: model_note.as_str(),
        })
        .await;
        let (resp, upstream_start, upstream_headers_ms, debug_base) = match transport {
            AttemptTransportOutcome::RetrySameUpstream => continue,
            AttemptTransportOutcome::TryNextUpstream => break,
            AttemptTransportOutcome::Continue(success) => (
                success.response,
                success.upstream_start,
                success.upstream_headers_ms,
                success.debug_base,
            ),
        };
        let status = resp.status();
        let success = status.is_success();
        let resp_headers = resp.headers().clone();
        let resp_headers_filtered = filter_response_headers(&resp_headers);

        if request_flavor.is_stream && success {
            return SelectedUpstreamExecutionOutcome::Return(
                handle_streaming_attempt_success(StreamingAttemptResponseParams {
                    proxy,
                    legacy_lb,
                    target,
                    response: resp,
                    status,
                    response_headers: resp_headers,
                    response_headers_filtered: resp_headers_filtered,
                    start: *start,
                    started_at_ms,
                    upstream_start,
                    upstream_headers_ms,
                    request_body_len,
                    upstream_request_body_len,
                    debug_base,
                    upstream_chain,
                    route_attempts,
                    route_attempt_index,
                    model_note: model_note.as_str(),
                    route_graph_key,
                    session_id,
                    cwd,
                    effective_effort,
                    base_service_tier,
                    request_id,
                    is_user_turn: request_flavor.is_user_turn,
                    is_codex_service: request_flavor.is_codex_service,
                    transport_cooldown_secs: plan.transport_cooldown_secs,
                    cooldown_backoff,
                    method,
                    path: uri.path(),
                })
                .await,
            );
        }

        let bytes = match read_attempt_response_body(AttemptReadBodyParams {
            proxy,
            legacy_lb,
            target,
            response: resp,
            upstream_opt,
            upstream_attempt,
            transport_cooldown_secs: plan.transport_cooldown_secs,
            cooldown_backoff,
            avoid_set,
            avoided_total,
            last_err,
            upstream_chain,
            route_attempts,
            route_attempt_index,
            model_note: model_note.as_str(),
        })
        .await
        {
            AttemptReadBodyOutcome::RetrySameUpstream => continue,
            AttemptReadBodyOutcome::TryNextUpstream => break,
            AttemptReadBodyOutcome::Continue(bytes) => bytes,
        };

        let dur = start.elapsed().as_millis() as u64;
        match handle_attempt_response(AttemptResponseParams {
            proxy,
            legacy_lb,
            target,
            method,
            path: uri.path(),
            status,
            response_headers: resp_headers,
            response_headers_filtered: resp_headers_filtered,
            response_body: bytes,
            request_id,
            duration_ms: dur,
            started_at_ms,
            upstream_headers_ms,
            provider_id: provider_id.as_deref(),
            session_id,
            cwd,
            effective_effort,
            base_service_tier,
            upstream_chain,
            route_attempts,
            route_attempt_index,
            model_note: model_note.as_str(),
            route_graph_key,
            plan,
            upstream_opt,
            provider_opt,
            upstream_attempt,
            avoid_set,
            avoided_total,
            last_err,
            cooldown_backoff,
        })
        .await
        {
            AttemptResponseOutcome::RetrySameUpstream => continue,
            AttemptResponseOutcome::TryNextUpstream => break,
            AttemptResponseOutcome::Return(response) => {
                return SelectedUpstreamExecutionOutcome::Return(response);
            }
        }
    }

    SelectedUpstreamExecutionOutcome::ContinueStation
}

fn log_attempt_select(params: AttemptSelectLogParams<'_>) {
    let AttemptSelectLogParams {
        service_name,
        request_id,
        global_attempt,
        provider_attempt,
        upstream_attempt,
        provider_opt,
        upstream_opt,
        target,
        provider_id,
        avoid_set,
        avoided_total,
        total_upstreams,
        model_note,
    } = params;

    let mut avoided_indices = avoid_set.iter().copied().collect::<Vec<_>>();
    avoided_indices.sort_unstable();
    let avoid_for_station = if target.uses_provider_endpoint_attempt_index() {
        Vec::new()
    } else {
        avoided_indices.clone()
    };
    let avoided_candidate_indices = if target.uses_provider_endpoint_attempt_index() {
        avoided_indices
    } else {
        Vec::new()
    };
    let endpoint_id = target.endpoint_id();
    let provider_endpoint_key = target.provider_endpoint_key();
    let preference_group = target.preference_group();

    log_retry_trace(serde_json::json!({
        "event": "attempt_select",
        "service": service_name,
        "request_id": request_id,
        "attempt": global_attempt,
        "provider_attempt": provider_attempt + 1,
        "upstream_attempt": upstream_attempt + 1,
        "provider_max_attempts": provider_opt.max_attempts,
        "upstream_max_attempts": upstream_opt.max_attempts,
        "upstream_base_url": target.upstream().base_url.as_str(),
        "provider_id": provider_id.or_else(|| target.provider_id()),
        "endpoint_id": endpoint_id,
        "provider_endpoint_key": provider_endpoint_key,
        "preference_group": preference_group,
        "compatibility": {
            "station_name": target.compatibility_station_name(),
            "upstream_index": target.compatibility_upstream_index(),
        },
        "avoid_for_station": avoid_for_station,
        "avoided_candidate_indices": avoided_candidate_indices,
        "avoided_total": avoided_total,
        "total_upstreams": total_upstreams,
        "model": model_note,
    }));
}
