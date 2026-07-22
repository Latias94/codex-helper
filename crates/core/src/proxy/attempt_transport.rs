use std::collections::HashSet;
use std::convert::TryFrom;
use std::sync::OnceLock;
use std::time::Instant;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode};
use futures_util::StreamExt;

use crate::endpoint_health::RuntimeHealthDomain;
use crate::logging::{
    BodyPreview, HeaderEntry, HttpDebugLog, RouteAttemptLog, should_include_http_debug,
    should_include_http_warn,
};
use crate::runtime_store::{AttemptHandle, AttemptOutcome, EconomicsState};
use crate::state::{
    CapturedUpstreamAttemptContext, DispatchedRuntimeHealthHalfOpenProbe, RouteDecisionProvenance,
    RuntimeHealthHalfOpenProbeLease,
};

use super::ProxyService;
use super::attempt_failures::{TerminalUpstreamFailureParams, apply_terminal_upstream_failure};
use super::attempt_request::{
    AttemptRequestIdentity, FrozenAttemptRequestSetupParams, prepare_attempt_request_with_identity,
};
use super::http_debug::{
    HttpDebugBase, HttpDebugTransportErrorParams, format_reqwest_error_for_retry_chain,
    warn_http_debug,
};
use super::request_preparation::{RequestReplayPolicy, SharedRouteStateImpact};
use super::retry::{RetryLayerOptions, backoff_sleep, should_retry_class};
use super::route_attempts::{
    ErrorRouteAttemptParams, RouteAttemptErrorKind, record_error_route_attempt,
    record_http_debug_route_attempt,
};
use crate::routing_ir::CapturedRouteCandidate;

pub(super) struct AttemptTransportSuccess {
    pub(super) response: reqwest::Response,
    pub(super) upstream_start: Instant,
    pub(super) upstream_headers_ms: u64,
    pub(super) debug_base: Option<HttpDebugBase>,
    pub(super) attempt_handle: AttemptHandle,
    pub(super) half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
}

pub(super) enum AttemptTransportOutcome {
    RetrySameUpstream,
    TryNextUpstream,
    StopProviderChain,
    Continue(Box<AttemptTransportSuccess>),
}

pub(super) enum AttemptReadBodyOutcome {
    RetrySameUpstream,
    TryNextUpstream,
    StopProviderChain,
    Continue {
        bytes: Bytes,
        half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
    },
}

enum ResponseBodyReadError {
    Read(reqwest::Error),
    TooLarge {
        limit: usize,
        observed: usize,
        content_length: Option<u64>,
    },
}

fn upstream_response_body_max_bytes() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("CODEX_HELPER_UPSTREAM_RESPONSE_BODY_MAX_BYTES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(32 * 1024 * 1024)
            .clamp(1024 * 1024, 128 * 1024 * 1024)
    })
}

impl ResponseBodyReadError {
    fn message(&self) -> String {
        match self {
            Self::Read(error) => format_reqwest_error_for_retry_chain(error),
            Self::TooLarge {
                limit,
                observed,
                content_length,
            } => match content_length {
                Some(len) => format!(
                    "upstream response body too large: content_length={} limit={} observed={}",
                    len, limit, observed
                ),
                None => format!(
                    "upstream response body too large: observed={} limit={}",
                    observed, limit
                ),
            },
        }
    }

    fn runtime_health_domain(
        &self,
        shared_route_state_impact: SharedRouteStateImpact,
    ) -> Option<RuntimeHealthDomain> {
        match self {
            Self::Read(_) if shared_route_state_impact.allows_shared_updates() => {
                Some(RuntimeHealthDomain::EndpointTransport)
            }
            Self::Read(_) | Self::TooLarge { .. } => None,
        }
    }
}

async fn read_response_body_with_limit(
    response: reqwest::Response,
) -> Result<Bytes, ResponseBodyReadError> {
    let max = upstream_response_body_max_bytes();
    let content_length = response.content_length();
    if content_length.is_some_and(|len| len > max as u64) {
        return Err(ResponseBodyReadError::TooLarge {
            limit: max,
            observed: max.saturating_add(1),
            content_length,
        });
    }

    let mut body = Vec::with_capacity(
        content_length
            .and_then(|len| usize::try_from(len).ok())
            .unwrap_or(0)
            .min(max),
    );
    let stream = response.bytes_stream();
    futures_util::pin_mut!(stream);
    while let Some(item) = stream.next().await {
        let chunk = item.map_err(ResponseBodyReadError::Read)?;
        let next_len = body.len().saturating_add(chunk.len());
        if next_len > max {
            return Err(ResponseBodyReadError::TooLarge {
                limit: max,
                observed: next_len,
                content_length,
            });
        }
        body.extend_from_slice(&chunk);
    }

    Ok(Bytes::from(body))
}

pub(super) struct AttemptTransportParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) method: &'a Method,
    pub(super) target_url: &'a reqwest::Url,
    pub(super) request_identity: &'a AttemptRequestIdentity,
    pub(super) attempt_context: &'a CapturedUpstreamAttemptContext,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) client_headers_entries_cache: &'a OnceLock<Vec<HeaderEntry>>,
    pub(super) request_body_len: usize,
    pub(super) upstream_request_body_len: usize,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
    pub(super) client_uri: &'a str,
    pub(super) client_body_debug: Option<&'a BodyPreview>,
    pub(super) upstream_request_body_debug: Option<&'a BodyPreview>,
    pub(super) client_body_warn: Option<&'a BodyPreview>,
    pub(super) upstream_request_body_warn: Option<&'a BodyPreview>,
    pub(super) request_id: u64,
    pub(super) route_decision: &'a RouteDecisionProvenance,
    pub(super) filtered_body: &'a Bytes,
    pub(super) last_http_debug: &'a mut Option<HttpDebugLog>,
    pub(super) upstream_opt: &'a RetryLayerOptions,
    pub(super) upstream_attempt: u32,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cooldown_backoff: crate::endpoint_health::CooldownBackoff,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
    pub(super) route_attempts: &'a mut Vec<RouteAttemptLog>,
    pub(super) route_attempt_index: usize,
    pub(super) model_note: &'a str,
    pub(super) allow_provider_failover: bool,
    pub(super) shared_route_state_impact: SharedRouteStateImpact,
    pub(super) replay_policy: RequestReplayPolicy,
    pub(super) half_open_probe: Option<RuntimeHealthHalfOpenProbeLease>,
}

pub(super) struct AttemptTargetBuildFailureParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) error_message: String,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cooldown_backoff: crate::endpoint_health::CooldownBackoff,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
    pub(super) route_attempts: &'a mut Vec<RouteAttemptLog>,
    pub(super) route_attempt_index: usize,
    pub(super) model_note: &'a str,
    pub(super) allow_provider_failover: bool,
}

pub(super) struct AttemptReadBodyParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) response: reqwest::Response,
    pub(super) response_headers: &'a HeaderMap,
    pub(super) upstream_headers_ms: u64,
    pub(super) debug_base: Option<&'a HttpDebugBase>,
    pub(super) last_http_debug: &'a mut Option<HttpDebugLog>,
    pub(super) upstream_opt: &'a RetryLayerOptions,
    pub(super) upstream_attempt: u32,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cooldown_backoff: crate::endpoint_health::CooldownBackoff,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
    pub(super) route_attempts: &'a mut Vec<RouteAttemptLog>,
    pub(super) route_attempt_index: usize,
    pub(super) model_note: &'a str,
    pub(super) allow_provider_failover: bool,
    pub(super) attempt_handle: AttemptHandle,
    pub(super) shared_route_state_impact: SharedRouteStateImpact,
    pub(super) replay_policy: RequestReplayPolicy,
    pub(super) half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
}

pub(super) async fn handle_attempt_target_build_failure(
    params: AttemptTargetBuildFailureParams<'_>,
) -> AttemptTransportOutcome {
    let AttemptTargetBuildFailureParams {
        proxy,
        target,
        error_message,
        transport_cooldown_secs,
        cooldown_backoff,
        avoid_set,
        avoided_total,
        last_err,
        route_attempts,
        route_attempt_index,
        model_note,
        allow_provider_failover,
    } = params;
    apply_terminal_upstream_failure(TerminalUpstreamFailureParams {
        proxy,
        target,
        health_domain: None,
        cooldown_secs: transport_cooldown_secs,
        cooldown_backoff,
        error_message,
        half_open_probe: None,
        avoid_set,
        avoided_total,
        last_err,
    })
    .await;
    record_error_route_attempt(
        route_attempts,
        ErrorRouteAttemptParams {
            target,
            route_attempt_index,
            kind: RouteAttemptErrorKind::TargetBuild,
            model_note,
            duration_ms: None,
            cooldown_secs: None,
            cooldown_reason: None,
        },
    );
    if allow_provider_failover {
        AttemptTransportOutcome::TryNextUpstream
    } else {
        AttemptTransportOutcome::StopProviderChain
    }
}

pub(super) async fn handle_attempt_transport(
    params: AttemptTransportParams<'_>,
) -> AttemptTransportOutcome {
    let AttemptTransportParams {
        proxy,
        target,
        method,
        target_url,
        request_identity,
        attempt_context,
        client_headers,
        client_headers_entries_cache,
        request_body_len,
        upstream_request_body_len,
        debug_max,
        warn_max,
        client_uri,
        client_body_debug,
        upstream_request_body_debug,
        client_body_warn,
        upstream_request_body_warn,
        request_id,
        route_decision,
        filtered_body,
        last_http_debug,
        upstream_opt,
        upstream_attempt,
        transport_cooldown_secs,
        cooldown_backoff,
        avoid_set,
        avoided_total,
        last_err,
        route_attempts,
        route_attempt_index,
        model_note,
        allow_provider_failover,
        shared_route_state_impact,
        replay_policy,
        half_open_probe,
    } = params;

    *last_http_debug = None;

    let stable_route_attempt_index = route_attempts
        .get(route_attempt_index)
        .map(|attempt| attempt.attempt_index)
        .unwrap_or(route_attempt_index as u32);
    let attempt_request = prepare_attempt_request_with_identity(FrozenAttemptRequestSetupParams {
        route_attempt_index: stable_route_attempt_index,
        identity: request_identity,
        client_headers,
        client_headers_entries_cache,
        request_body_len,
        upstream_request_body_len,
        debug_max,
        warn_max,
        client_uri,
        target_url: target_url.as_str(),
        client_body_debug,
        upstream_request_body_debug,
        client_body_warn,
        upstream_request_body_warn,
    });
    let headers = attempt_request.headers;
    proxy
        .state
        .update_request_route(request_id, route_decision.clone())
        .await;
    let debug_base = attempt_request.debug_base;

    let builder = proxy
        .client
        .request(method.clone(), target_url.clone())
        .headers(headers)
        .body(filtered_body.clone());

    let attempt_handle = match proxy
        .state
        .begin_upstream_attempt(attempt_context, crate::logging::now_ms())
        .await
    {
        Ok(handle) => handle,
        Err(error) => {
            let message = error.to_string();
            record_error_route_attempt(
                route_attempts,
                ErrorRouteAttemptParams {
                    target,
                    route_attempt_index,
                    kind: RouteAttemptErrorKind::Lifecycle,
                    model_note,
                    duration_ms: None,
                    cooldown_secs: None,
                    cooldown_reason: None,
                },
            );
            *last_err = Some((StatusCode::INTERNAL_SERVER_ERROR, message));
            return AttemptTransportOutcome::StopProviderChain;
        }
    };

    let half_open_probe = match half_open_probe {
        Some(probe) => match proxy.state.dispatch_runtime_half_open_probe(probe).await {
            Ok(probe) => Some(probe),
            Err(_) => {
                let message =
                    "transient half-open probe became unavailable before dispatch".to_string();
                if let Err(error) = proxy.state.finish_upstream_attempt(
                    attempt_handle,
                    AttemptOutcome::Failed,
                    crate::logging::now_ms(),
                    EconomicsState::Unknown,
                ) {
                    *last_err = Some((StatusCode::INTERNAL_SERVER_ERROR, error.to_string()));
                    return AttemptTransportOutcome::StopProviderChain;
                }
                record_error_route_attempt(
                    route_attempts,
                    ErrorRouteAttemptParams {
                        target,
                        route_attempt_index,
                        kind: RouteAttemptErrorKind::HalfOpenInvalidated,
                        model_note,
                        duration_ms: None,
                        cooldown_secs: None,
                        cooldown_reason: None,
                    },
                );
                *last_err = Some((StatusCode::SERVICE_UNAVAILABLE, message));
                return AttemptTransportOutcome::StopProviderChain;
            }
        },
        None => None,
    };

    let upstream_start = Instant::now();
    let response = match builder.send().await {
        Ok(response) => response,
        Err(error) => {
            let err_str = format_reqwest_error_for_retry_chain(&error);
            let status_code = StatusCode::BAD_GATEWAY.as_u16();
            if should_include_http_warn(status_code)
                && let Some(http_debug) = debug_base.as_ref().and_then(|base| {
                    base.transport_error_log(HttpDebugTransportErrorParams {
                        response_headers: None,
                        upstream_headers_ms: None,
                        upstream_body_read_ms: None,
                        error_class: "upstream_transport_error",
                        error_hint:
                            "upstream connection failed before response headers were received",
                        upstream_error: err_str.clone(),
                        for_warn: true,
                    })
                })
            {
                warn_http_debug(status_code, &http_debug);
            }
            *last_http_debug = should_include_http_debug(status_code)
                .then(|| {
                    debug_base.as_ref()?.transport_error_log(HttpDebugTransportErrorParams {
                        response_headers: None,
                        upstream_headers_ms: None,
                        upstream_body_read_ms: None,
                        error_class: "upstream_transport_error",
                        error_hint:
                            "upstream connection failed before response headers were received",
                        upstream_error: err_str.clone(),
                        for_warn: false,
                    })
                })
                .flatten();
            record_http_debug_route_attempt(
                route_attempts,
                route_attempt_index,
                last_http_debug.as_ref(),
            );
            if let Err(commit_error) = proxy.state.finish_upstream_attempt(
                attempt_handle,
                AttemptOutcome::Failed,
                crate::logging::now_ms(),
                EconomicsState::Unknown,
            ) {
                let message = commit_error.to_string();
                record_error_route_attempt(
                    route_attempts,
                    ErrorRouteAttemptParams {
                        target,
                        route_attempt_index,
                        kind: RouteAttemptErrorKind::Lifecycle,
                        model_note,
                        duration_ms: Some(upstream_start.elapsed().as_millis() as u64),
                        cooldown_secs: None,
                        cooldown_reason: None,
                    },
                );
                *last_err = Some((StatusCode::INTERNAL_SERVER_ERROR, message));
                return AttemptTransportOutcome::StopProviderChain;
            }
            let can_retry_upstream = replay_policy.allows_after_dispatch()
                && upstream_attempt + 1 < upstream_opt.max_attempts
                && should_retry_class(upstream_opt, Some("upstream_transport_error"));
            record_error_route_attempt(
                route_attempts,
                ErrorRouteAttemptParams {
                    target,
                    route_attempt_index,
                    kind: RouteAttemptErrorKind::Transport,
                    model_note,
                    duration_ms: Some(upstream_start.elapsed().as_millis() as u64),
                    cooldown_secs: (!can_retry_upstream
                        && shared_route_state_impact.allows_shared_updates())
                    .then_some(transport_cooldown_secs),
                    cooldown_reason: (!can_retry_upstream
                        && shared_route_state_impact.allows_shared_updates())
                    .then_some("upstream_transport_error"),
                },
            );
            if can_retry_upstream {
                backoff_sleep(upstream_opt, upstream_attempt).await;
                return AttemptTransportOutcome::RetrySameUpstream;
            }

            apply_terminal_upstream_failure(TerminalUpstreamFailureParams {
                proxy,
                target,
                health_domain: shared_route_state_impact
                    .allows_shared_updates()
                    .then_some(RuntimeHealthDomain::EndpointTransport),
                cooldown_secs: transport_cooldown_secs,
                cooldown_backoff,
                error_message: err_str,
                half_open_probe,
                avoid_set,
                avoided_total,
                last_err,
            })
            .await;
            return if replay_policy.allows_after_dispatch() && allow_provider_failover {
                AttemptTransportOutcome::TryNextUpstream
            } else {
                AttemptTransportOutcome::StopProviderChain
            };
        }
    };

    AttemptTransportOutcome::Continue(Box::new(AttemptTransportSuccess {
        response,
        upstream_start,
        upstream_headers_ms: upstream_start.elapsed().as_millis() as u64,
        debug_base,
        attempt_handle,
        half_open_probe,
    }))
}

pub(super) async fn read_attempt_response_body(
    params: AttemptReadBodyParams<'_>,
) -> AttemptReadBodyOutcome {
    let AttemptReadBodyParams {
        proxy,
        target,
        response,
        response_headers,
        upstream_headers_ms,
        debug_base,
        last_http_debug,
        upstream_opt,
        upstream_attempt,
        transport_cooldown_secs,
        cooldown_backoff,
        avoid_set,
        avoided_total,
        last_err,
        route_attempts,
        route_attempt_index,
        model_note,
        allow_provider_failover,
        attempt_handle,
        shared_route_state_impact,
        replay_policy,
        half_open_probe,
    } = params;

    let body_read_started = Instant::now();
    match read_response_body_with_limit(response).await {
        Ok(bytes) => AttemptReadBodyOutcome::Continue {
            bytes,
            half_open_probe,
        },
        Err(error) => {
            let err_str = error.message();
            let (error_class, error_hint) = match &error {
                ResponseBodyReadError::Read(_) => (
                    "upstream_body_read_error",
                    "upstream response body could not be read to completion",
                ),
                ResponseBodyReadError::TooLarge { .. } => (
                    "upstream_response_body_too_large",
                    "upstream response body exceeded the configured safety limit",
                ),
            };
            let body_read_ms = body_read_started.elapsed().as_millis() as u64;
            let status_code = StatusCode::BAD_GATEWAY.as_u16();
            if should_include_http_warn(status_code)
                && let Some(http_debug) = debug_base.and_then(|base| {
                    base.transport_error_log(HttpDebugTransportErrorParams {
                        response_headers: Some(response_headers),
                        upstream_headers_ms: Some(upstream_headers_ms),
                        upstream_body_read_ms: Some(body_read_ms),
                        error_class,
                        error_hint,
                        upstream_error: err_str.clone(),
                        for_warn: true,
                    })
                })
            {
                warn_http_debug(status_code, &http_debug);
            }
            *last_http_debug = should_include_http_debug(status_code)
                .then(|| {
                    debug_base?.transport_error_log(HttpDebugTransportErrorParams {
                        response_headers: Some(response_headers),
                        upstream_headers_ms: Some(upstream_headers_ms),
                        upstream_body_read_ms: Some(body_read_ms),
                        error_class,
                        error_hint,
                        upstream_error: err_str.clone(),
                        for_warn: false,
                    })
                })
                .flatten();
            record_http_debug_route_attempt(
                route_attempts,
                route_attempt_index,
                last_http_debug.as_ref(),
            );
            let health_domain = error.runtime_health_domain(shared_route_state_impact);
            if let Err(commit_error) = proxy.state.finish_upstream_attempt(
                attempt_handle,
                AttemptOutcome::Failed,
                crate::logging::now_ms(),
                EconomicsState::Unknown,
            ) {
                let message = commit_error.to_string();
                record_error_route_attempt(
                    route_attempts,
                    ErrorRouteAttemptParams {
                        target,
                        route_attempt_index,
                        kind: RouteAttemptErrorKind::Lifecycle,
                        model_note,
                        duration_ms: None,
                        cooldown_secs: None,
                        cooldown_reason: None,
                    },
                );
                *last_err = Some((StatusCode::INTERNAL_SERVER_ERROR, message));
                return AttemptReadBodyOutcome::StopProviderChain;
            }
            let (route_kind, can_retry_upstream, cooldown_reason) = match &error {
                ResponseBodyReadError::Read(_) => {
                    let can_retry_upstream = replay_policy.allows_after_dispatch()
                        && upstream_attempt + 1 < upstream_opt.max_attempts
                        && should_retry_class(upstream_opt, Some("upstream_transport_error"));
                    (
                        RouteAttemptErrorKind::BodyRead,
                        can_retry_upstream,
                        "upstream_body_read_error",
                    )
                }
                ResponseBodyReadError::TooLarge { .. } => (
                    RouteAttemptErrorKind::BodyTooLarge,
                    false,
                    "upstream_response_body_too_large",
                ),
            };
            record_error_route_attempt(
                route_attempts,
                ErrorRouteAttemptParams {
                    target,
                    route_attempt_index,
                    kind: route_kind,
                    model_note,
                    duration_ms: None,
                    cooldown_secs: (!can_retry_upstream && health_domain.is_some())
                        .then_some(transport_cooldown_secs),
                    cooldown_reason: (!can_retry_upstream && health_domain.is_some())
                        .then_some(cooldown_reason),
                },
            );
            if matches!(error, ResponseBodyReadError::Read(_)) && can_retry_upstream {
                backoff_sleep(upstream_opt, upstream_attempt).await;
                return AttemptReadBodyOutcome::RetrySameUpstream;
            }

            apply_terminal_upstream_failure(TerminalUpstreamFailureParams {
                proxy,
                target,
                health_domain,
                cooldown_secs: transport_cooldown_secs,
                cooldown_backoff,
                error_message: err_str,
                half_open_probe,
                avoid_set,
                avoided_total,
                last_err,
            })
            .await;
            if replay_policy.allows_after_dispatch() && allow_provider_failover {
                AttemptReadBodyOutcome::TryNextUpstream
            } else {
                AttemptReadBodyOutcome::StopProviderChain
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_response_body_is_not_upstream_health_evidence() {
        let error = ResponseBodyReadError::TooLarge {
            limit: 1024,
            observed: 1025,
            content_length: Some(1025),
        };

        assert_eq!(
            error.runtime_health_domain(SharedRouteStateImpact::RouteFacing),
            None
        );
    }
}
