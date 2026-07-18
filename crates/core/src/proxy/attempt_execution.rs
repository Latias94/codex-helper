use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode, Uri};

use crate::endpoint_health::CooldownBackoff;
use crate::logging::{
    BodyPreview, HeaderEntry, RouteAttemptLog, ServiceTierLog, log_control_trace_event,
    upstream_origin,
};
use crate::runtime_store::{AttemptOutcome, AttemptRouteEvidence, EconomicsState};
use crate::state::{AttemptProviderScopeCapture, SessionBinding, SessionIdentitySource};

use super::ProxyService;
use super::attempt_request::{AttemptRequestIdentityParams, prepare_attempt_request_identity};
use super::attempt_response::{
    AttemptResponseOutcome, AttemptResponseParams, StreamingAttemptResponseParams,
    handle_attempt_response, handle_streaming_attempt_success,
};
use super::attempt_transport::{
    AttemptReadBodyOutcome, AttemptReadBodyParams, AttemptTargetBuildFailureParams,
    AttemptTransportOutcome, AttemptTransportParams, handle_attempt_target_build_failure,
    handle_attempt_transport, read_attempt_response_body,
};
use super::concurrency_limits::ConcurrencyPermit;
use super::headers::filter_response_headers;
use super::reasoning_guard::should_strict_buffer_reasoning_guard;
use super::request_body::{
    ReasoningOrchestrationIntent, RequestDialect, is_stale_previous_response_error,
    remove_previous_response_id_from_body,
};
use super::request_preparation::RequestFlavor;
use super::response_entity::UpstreamResponseEntity;
use super::response_semantics::ResponseSemanticContract;
use super::retry::{RetryLayerOptions, RetryPlan};
use super::route_attempts::{
    StartRouteAttemptParams, StatusRouteAttemptParams, record_status_route_attempt,
    start_selected_route_attempt,
};
use super::selected_upstream_request::{
    SelectedUpstreamRequestSetupParams, apply_selected_model_mapping,
    prepare_selected_upstream_request,
};
use crate::routing_ir::CapturedRouteCandidate;

const UPSTREAM_AUTH_UNAVAILABLE_REASON: &str = "configured upstream credentials are unavailable";

pub(super) enum SelectedUpstreamExecutionOutcome {
    ContinueProviderChain,
    StopProviderChain,
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
    target: &'a CapturedRouteCandidate,
    provider_id: Option<&'a str>,
    avoid_set: &'a HashSet<usize>,
    avoided_total: usize,
    total_upstreams: usize,
    model_note: &'a str,
}

pub(super) struct ExecuteSelectedUpstreamParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) target: &'a CapturedRouteCandidate,
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
    pub(super) request_dialect: RequestDialect,
    pub(super) request_model: Option<&'a str>,
    pub(super) session_binding: Option<&'a SessionBinding>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) deferred_reasoning_intent: Option<ReasoningOrchestrationIntent>,
    pub(super) effective_service_tier: Option<&'a str>,
    pub(super) base_service_tier: &'a ServiceTierLog,
    pub(super) session_id: Option<&'a str>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) cwd: Option<&'a str>,
    pub(super) request_flavor: &'a RequestFlavor,
    pub(super) request_body_previews: bool,
    pub(super) response_semantic_contract: Option<ResponseSemanticContract>,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
    pub(super) client_body_debug: Option<&'a BodyPreview>,
    pub(super) client_body_warn: Option<&'a BodyPreview>,
    pub(super) plan: &'a RetryPlan,
    pub(super) route_graph_key: Option<&'a str>,
    pub(super) upstream_opt: &'a RetryLayerOptions,
    pub(super) provider_opt: &'a RetryLayerOptions,
    pub(super) allow_provider_failover: bool,
    pub(super) provider_attempt: u32,
    pub(super) total_upstreams: usize,
    pub(super) cooldown_backoff: CooldownBackoff,
    pub(super) global_attempt: &'a mut u32,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
    pub(super) route_attempts: &'a mut Vec<RouteAttemptLog>,
    pub(super) concurrency_permit: Option<ConcurrencyPermit>,
}

pub(super) async fn execute_selected_upstream(
    params: ExecuteSelectedUpstreamParams<'_>,
) -> SelectedUpstreamExecutionOutcome {
    let ExecuteSelectedUpstreamParams {
        proxy,
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
        request_dialect,
        request_model,
        session_binding,
        effective_effort,
        deferred_reasoning_intent,
        effective_service_tier,
        base_service_tier,
        session_id,
        session_identity_source,
        cwd,
        request_flavor,
        request_body_previews,
        response_semantic_contract,
        debug_max,
        warn_max,
        client_body_debug,
        client_body_warn,
        plan,
        route_graph_key,
        upstream_opt,
        provider_opt,
        allow_provider_failover,
        provider_attempt,
        total_upstreams,
        cooldown_backoff,
        global_attempt,
        avoid_set,
        avoided_total,
        last_err,
        route_attempts,
        mut concurrency_permit,
    } = params;

    let model_mapping = apply_selected_model_mapping(target, body_for_upstream, request_model);
    let target_url = match proxy.build_target(target, uri) {
        Ok(url) => url,
        Err(error) => {
            *global_attempt = global_attempt.saturating_add(1);
            log_attempt_select(AttemptSelectLogParams {
                service_name: proxy.service_name,
                request_id,
                global_attempt: *global_attempt,
                provider_attempt,
                upstream_attempt: 0,
                provider_opt,
                upstream_opt,
                target,
                provider_id: Some(target.provider_id()),
                avoid_set,
                avoided_total: *avoided_total,
                total_upstreams,
                model_note: model_mapping.model_note.as_str(),
            });
            let route_attempt_index = start_selected_route_attempt(
                route_attempts,
                StartRouteAttemptParams {
                    target,
                    provider_id: Some(target.provider_id()),
                    provider_attempt,
                    upstream_attempt: 0,
                    provider_max_attempts: provider_opt.max_attempts,
                    upstream_max_attempts: upstream_opt.max_attempts,
                    model_note: model_mapping.model_note.as_str(),
                    avoid_set,
                    avoided_total: *avoided_total,
                    total_upstreams,
                },
            );
            return match handle_attempt_target_build_failure(AttemptTargetBuildFailureParams {
                proxy,
                target,
                error_message: error.to_string(),
                transport_cooldown_secs: plan.transport_cooldown_secs,
                cooldown_backoff,
                avoid_set,
                avoided_total,
                last_err,
                route_attempts,
                route_attempt_index,
                model_note: model_mapping.model_note.as_str(),
                allow_provider_failover,
            })
            .await
            {
                AttemptTransportOutcome::TryNextUpstream => {
                    SelectedUpstreamExecutionOutcome::ContinueProviderChain
                }
                AttemptTransportOutcome::StopProviderChain => {
                    SelectedUpstreamExecutionOutcome::StopProviderChain
                }
                AttemptTransportOutcome::RetrySameUpstream
                | AttemptTransportOutcome::Continue(_) => {
                    unreachable!("target build failure cannot continue transport")
                }
            };
        }
    };
    let request_identity = match prepare_attempt_request_identity(AttemptRequestIdentityParams {
        service_name: proxy.service_name,
        credential: target.credential(),
        credential_scope: target.runtime_identity().credential_scope.as_deref(),
        state: proxy.state.as_ref(),
        client_headers,
        client_uri,
        target_url: target_url.as_str(),
    }) {
        Ok(identity) => identity,
        Err(error) => {
            *global_attempt = global_attempt.saturating_add(1);
            log_attempt_select(AttemptSelectLogParams {
                service_name: proxy.service_name,
                request_id,
                global_attempt: *global_attempt,
                provider_attempt,
                upstream_attempt: 0,
                provider_opt,
                upstream_opt,
                target,
                provider_id: Some(target.provider_id()),
                avoid_set,
                avoided_total: *avoided_total,
                total_upstreams,
                model_note: model_mapping.model_note.as_str(),
            });
            let route_attempt_index = start_selected_route_attempt(
                route_attempts,
                StartRouteAttemptParams {
                    target,
                    provider_id: Some(target.provider_id()),
                    provider_attempt,
                    upstream_attempt: 0,
                    provider_max_attempts: provider_opt.max_attempts,
                    upstream_max_attempts: upstream_opt.max_attempts,
                    model_note: model_mapping.model_note.as_str(),
                    avoid_set,
                    avoided_total: *avoided_total,
                    total_upstreams,
                },
            );
            tracing::warn!(
                request_id,
                provider_id = target.provider_id(),
                auth_error_code = error.code(),
                error = %error,
                "selected provider authentication could not be resolved"
            );
            let outcome = handle_attempt_target_build_failure(AttemptTargetBuildFailureParams {
                proxy,
                target,
                error_message: UPSTREAM_AUTH_UNAVAILABLE_REASON.to_string(),
                transport_cooldown_secs: 0,
                cooldown_backoff,
                avoid_set,
                avoided_total,
                last_err,
                route_attempts,
                route_attempt_index,
                model_note: model_mapping.model_note.as_str(),
                allow_provider_failover,
            })
            .await;
            if let Some((status, _)) = last_err.as_mut() {
                *status = StatusCode::SERVICE_UNAVAILABLE;
            }
            return match outcome {
                AttemptTransportOutcome::TryNextUpstream => {
                    SelectedUpstreamExecutionOutcome::ContinueProviderChain
                }
                AttemptTransportOutcome::StopProviderChain => {
                    SelectedUpstreamExecutionOutcome::StopProviderChain
                }
                AttemptTransportOutcome::RetrySameUpstream
                | AttemptTransportOutcome::Continue(_) => {
                    unreachable!("authentication resolution failure cannot continue transport")
                }
            };
        }
    };
    let attempt_context = match proxy
        .state
        .capture_upstream_attempt_context(
            request_id,
            AttemptRouteEvidence {
                provider_endpoint_key: Some(target.provider_endpoint_key()),
                provider_id: Some(target.provider_id().to_owned()),
                endpoint_id: Some(target.endpoint_id().to_owned()),
                route_path: target.route_path().to_vec(),
                upstream_base_url: Some(target.base_url().to_owned()),
                mapped_model: model_mapping.effective_model.clone(),
            },
            AttemptProviderScopeCapture {
                endpoint: target_url.clone(),
                route_scope: target.provider_endpoint_key(),
                account_fingerprint: request_identity.account_fingerprint,
            },
        )
        .await
    {
        Ok(context) => context,
        Err(error) => {
            tracing::error!(
                request_id,
                provider_id = target.provider_id(),
                error = %error,
                "failed to capture durable upstream attempt context"
            );
            *last_err = Some((StatusCode::INTERNAL_SERVER_ERROR, error.to_string()));
            return SelectedUpstreamExecutionOutcome::StopProviderChain;
        }
    };

    let selected_setup =
        match prepare_selected_upstream_request(SelectedUpstreamRequestSetupParams {
            proxy,
            target,
            model_mapping,
            request_contract: attempt_context.request_contract(),
            request_dialect,
            request_model,
            session_binding,
            effective_effort,
            deferred_reasoning_intent,
            effective_service_tier,
            client_content_type: request_flavor.client_content_type.as_deref(),
            request_body_previews,
            debug_max,
            warn_max,
        }) {
            Ok(setup) => setup,
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(
                    request_id,
                    provider_id = target.provider_id(),
                    error = %error,
                    "selected provider cannot resolve deferred reasoning intent"
                );
                *last_err = Some((StatusCode::BAD_REQUEST, message));
                return SelectedUpstreamExecutionOutcome::StopProviderChain;
            }
        };
    let model_note = selected_setup.model_note;
    let provider_id = selected_setup.provider_id;
    let route_decision = selected_setup.route_decision;
    let selected_filtered_body = selected_setup.filtered_body;
    let selected_effective_effort = selected_setup.effective_effort;
    let effective_effort = selected_effective_effort.as_deref();
    let selected_upstream_request_body_len = selected_setup.upstream_request_body_len;
    let selected_upstream_request_body_debug = selected_setup.upstream_request_body_debug;
    let selected_upstream_request_body_warn = selected_setup.upstream_request_body_warn;
    let rectified_previous_response_body =
        remove_previous_response_id_from_body(&selected_filtered_body);

    for upstream_attempt in 0..upstream_opt.max_attempts {
        let mut current_filtered_body = selected_filtered_body.clone();
        let mut current_upstream_request_body_len = selected_upstream_request_body_len;
        let mut current_upstream_request_body_debug = selected_upstream_request_body_debug.clone();
        let mut current_upstream_request_body_warn = selected_upstream_request_body_warn.clone();
        let mut previous_response_rectified = false;

        *global_attempt = global_attempt.saturating_add(1);
        loop {
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
                target,
                method,
                target_url: &target_url,
                request_identity: &request_identity,
                attempt_context: &attempt_context,
                client_headers,
                client_headers_entries_cache,
                request_body_len,
                upstream_request_body_len: current_upstream_request_body_len,
                debug_max,
                warn_max,
                client_uri,
                client_body_debug,
                upstream_request_body_debug: current_upstream_request_body_debug.as_ref(),
                client_body_warn,
                upstream_request_body_warn: current_upstream_request_body_warn.as_ref(),
                request_id,
                route_decision: &route_decision,
                filtered_body: &current_filtered_body,
                upstream_opt,
                upstream_attempt,
                transport_cooldown_secs: plan.transport_cooldown_secs,
                cooldown_backoff,
                avoid_set,
                avoided_total,
                last_err,
                route_attempts,
                route_attempt_index,
                model_note: model_note.as_str(),
                allow_provider_failover,
            })
            .await;
            let (resp, upstream_start, upstream_headers_ms, debug_base, attempt_handle) =
                match transport {
                    AttemptTransportOutcome::RetrySameUpstream => break,
                    AttemptTransportOutcome::TryNextUpstream => {
                        return SelectedUpstreamExecutionOutcome::ContinueProviderChain;
                    }
                    AttemptTransportOutcome::StopProviderChain => {
                        return SelectedUpstreamExecutionOutcome::StopProviderChain;
                    }
                    AttemptTransportOutcome::Continue(success) => (
                        success.response,
                        success.upstream_start,
                        success.upstream_headers_ms,
                        success.debug_base,
                        success.attempt_handle,
                    ),
                };
            let status = resp.status();
            let success = status.is_success();
            let resp_headers = resp.headers().clone();
            let resp_headers_filtered = filter_response_headers(&resp_headers);
            let strict_buffer_reasoning_guard = request_flavor.is_stream
                && success
                && should_strict_buffer_reasoning_guard(
                    &plan.reasoning_guard,
                    proxy.service_name,
                    uri.path(),
                );

            if request_flavor.is_stream && success && !strict_buffer_reasoning_guard {
                return SelectedUpstreamExecutionOutcome::Return(
                    handle_streaming_attempt_success(StreamingAttemptResponseParams {
                        proxy,
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
                        upstream_request_body_len: current_upstream_request_body_len,
                        debug_base,
                        route_attempts,
                        route_attempt_index,
                        model_note: model_note.as_str(),
                        route_graph_key,
                        session_id,
                        session_identity_source,
                        cwd,
                        effective_effort,
                        base_service_tier,
                        codex_bridge: request_flavor.codex_bridge_log.clone(),
                        request_id,
                        is_user_turn: request_flavor.is_user_turn,
                        is_codex_service: request_flavor.is_codex_service,
                        transport_cooldown_secs: plan.transport_cooldown_secs,
                        cloudflare_challenge_cooldown_secs: plan.cloudflare_challenge_cooldown_secs,
                        cloudflare_timeout_cooldown_secs: plan.cloudflare_timeout_cooldown_secs,
                        cooldown_backoff,
                        method,
                        path: uri.path(),
                        concurrency_permit: concurrency_permit.take(),
                        attempt_handle,
                    })
                    .await,
                );
            }

            let bytes = match read_attempt_response_body(AttemptReadBodyParams {
                proxy,
                target,
                response: resp,
                upstream_opt,
                upstream_attempt,
                transport_cooldown_secs: plan.transport_cooldown_secs,
                cooldown_backoff,
                avoid_set,
                avoided_total,
                last_err,
                route_attempts,
                route_attempt_index,
                model_note: model_note.as_str(),
                allow_provider_failover,
                attempt_handle,
            })
            .await
            {
                AttemptReadBodyOutcome::RetrySameUpstream => break,
                AttemptReadBodyOutcome::TryNextUpstream => {
                    return SelectedUpstreamExecutionOutcome::ContinueProviderChain;
                }
                AttemptReadBodyOutcome::StopProviderChain => {
                    return SelectedUpstreamExecutionOutcome::StopProviderChain;
                }
                AttemptReadBodyOutcome::Continue(bytes) => bytes,
            };
            if request_flavor.is_codex_service
                && !previous_response_rectified
                && !success
                && is_stale_previous_response_error(status, bytes.as_ref())
                && let Some(rectified_body) = rectified_previous_response_body.as_ref()
            {
                if let Err(error) = proxy.state.finish_upstream_attempt(
                    attempt_handle,
                    AttemptOutcome::Failed,
                    crate::logging::now_ms(),
                    EconomicsState::Unknown,
                ) {
                    *last_err = Some((StatusCode::INTERNAL_SERVER_ERROR, error.to_string()));
                    return SelectedUpstreamExecutionOutcome::StopProviderChain;
                }
                record_status_route_attempt(
                    route_attempts,
                    StatusRouteAttemptParams {
                        target,
                        route_attempt_index,
                        status_code: status.as_u16(),
                        error_class: Some("codex_stale_previous_response_id"),
                        model_note: model_note.as_str(),
                        upstream_headers_ms,
                        duration_ms: start.elapsed().as_millis() as u64,
                        cooldown_secs: None,
                        cooldown_reason: None,
                        provider_signals: Vec::new(),
                        policy_actions: Vec::new(),
                    },
                );
                tracing::info!(
                    request_id,
                    status = status.as_u16(),
                    "retrying Codex request once without stale previous_response_id"
                );
                current_filtered_body = rectified_body.clone();
                current_upstream_request_body_len = current_filtered_body.len();
                let previews = super::request_preparation::build_body_previews(
                    current_filtered_body.as_ref(),
                    request_flavor.client_content_type.as_deref(),
                    request_body_previews,
                    debug_max,
                    warn_max,
                );
                current_upstream_request_body_debug = previews.debug;
                current_upstream_request_body_warn = previews.warn;
                previous_response_rectified = true;
                *global_attempt = global_attempt.saturating_add(1);
                continue;
            }

            let dur = start.elapsed().as_millis() as u64;
            match handle_attempt_response(AttemptResponseParams {
                proxy,
                target,
                method,
                path: uri.path(),
                status,
                upstream_entity: UpstreamResponseEntity::capture(status, &bytes),
                response_headers: resp_headers,
                response_headers_filtered: resp_headers_filtered,
                response_body: bytes,
                attempt_handle,
                request_id,
                duration_ms: dur,
                started_at_ms,
                upstream_headers_ms,
                provider_id: provider_id.as_deref(),
                session_id,
                session_identity_source,
                cwd,
                effective_effort,
                base_service_tier,
                codex_bridge: request_flavor.codex_bridge_log.clone(),
                response_semantic_contract,
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
                is_user_turn: request_flavor.is_user_turn,
                allow_provider_failover,
                is_codex_service: request_flavor.is_codex_service,
            })
            .await
            {
                AttemptResponseOutcome::RetrySameUpstream => break,
                AttemptResponseOutcome::TryNextUpstream => {
                    return SelectedUpstreamExecutionOutcome::ContinueProviderChain;
                }
                AttemptResponseOutcome::StopProviderChain => {
                    return SelectedUpstreamExecutionOutcome::StopProviderChain;
                }
                AttemptResponseOutcome::Return(response) => {
                    return SelectedUpstreamExecutionOutcome::Return(response);
                }
            }
        }
    }

    SelectedUpstreamExecutionOutcome::ContinueProviderChain
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
    let endpoint_id = target.endpoint_id();
    let provider_endpoint_key = target.provider_endpoint_key();
    let preference_group = target.preference_group();

    log_control_trace_event(serde_json::json!({
        "event": "attempt_select",
        "service": service_name,
        "request_id": request_id,
        "attempt": global_attempt,
        "provider_attempt": provider_attempt + 1,
        "upstream_attempt": upstream_attempt + 1,
        "provider_max_attempts": provider_opt.max_attempts,
        "upstream_max_attempts": upstream_opt.max_attempts,
        "upstream_origin": upstream_origin(target.base_url()),
        "provider_id": provider_id.unwrap_or_else(|| target.provider_id()),
        "endpoint_id": endpoint_id,
        "provider_endpoint_key": provider_endpoint_key,
        "preference_group": preference_group,
        "avoided_candidate_indices": avoided_indices,
        "avoided_total": avoided_total,
        "total_upstreams": total_upstreams,
        "model": model_note,
    }));
}
