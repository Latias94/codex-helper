use std::collections::HashSet;
use std::convert::TryFrom;
use std::sync::OnceLock;
use std::time::Instant;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use futures_util::StreamExt;

use crate::lb::LoadBalancer;
use crate::logging::{BodyPreview, HeaderEntry, RouteAttemptLog};
use crate::state::RouteDecisionProvenance;

use super::ProxyService;
use super::attempt_failures::{TerminalUpstreamFailureParams, apply_terminal_upstream_failure};
use super::attempt_request::{AttemptRequestSetupParams, prepare_attempt_request};
use super::attempt_target::AttemptTarget;
use super::http_debug::{HttpDebugBase, format_reqwest_error_for_retry_chain};
use super::retry::{RetryLayerOptions, backoff_sleep, should_retry_class};
use super::route_attempts::{
    ErrorRouteAttemptParams, RouteAttemptErrorKind, record_error_route_attempt,
};

pub(super) struct AttemptTransportSuccess {
    pub(super) response: reqwest::Response,
    pub(super) upstream_start: Instant,
    pub(super) upstream_headers_ms: u64,
    pub(super) debug_base: Option<HttpDebugBase>,
}

pub(super) enum AttemptTransportOutcome {
    RetrySameUpstream,
    TryNextUpstream,
    Continue(Box<AttemptTransportSuccess>),
}

pub(super) enum AttemptReadBodyOutcome {
    RetrySameUpstream,
    TryNextUpstream,
    Continue(Bytes),
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
    pub(super) legacy_lb: Option<&'a LoadBalancer>,
    pub(super) target: &'a AttemptTarget,
    pub(super) method: &'a Method,
    pub(super) uri: &'a Uri,
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
    pub(super) provider_id: Option<&'a str>,
    pub(super) route_decision: &'a RouteDecisionProvenance,
    pub(super) filtered_body: &'a Bytes,
    pub(super) upstream_opt: &'a RetryLayerOptions,
    pub(super) upstream_attempt: u32,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cooldown_backoff: crate::lb::CooldownBackoff,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
    pub(super) upstream_chain: &'a mut Vec<String>,
    pub(super) route_attempts: &'a mut Vec<RouteAttemptLog>,
    pub(super) route_attempt_index: usize,
    pub(super) model_note: &'a str,
}

pub(super) struct AttemptReadBodyParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) legacy_lb: Option<&'a LoadBalancer>,
    pub(super) target: &'a AttemptTarget,
    pub(super) response: reqwest::Response,
    pub(super) upstream_opt: &'a RetryLayerOptions,
    pub(super) upstream_attempt: u32,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cooldown_backoff: crate::lb::CooldownBackoff,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
    pub(super) upstream_chain: &'a mut Vec<String>,
    pub(super) route_attempts: &'a mut Vec<RouteAttemptLog>,
    pub(super) route_attempt_index: usize,
    pub(super) model_note: &'a str,
}

pub(super) async fn handle_attempt_transport(
    params: AttemptTransportParams<'_>,
) -> AttemptTransportOutcome {
    let AttemptTransportParams {
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
        upstream_request_body_debug,
        client_body_warn,
        upstream_request_body_warn,
        request_id,
        provider_id,
        route_decision,
        filtered_body,
        upstream_opt,
        upstream_attempt,
        transport_cooldown_secs,
        cooldown_backoff,
        avoid_set,
        avoided_total,
        last_err,
        upstream_chain,
        route_attempts,
        route_attempt_index,
        model_note,
    } = params;

    let target_url = match proxy.build_target(target, uri) {
        Ok((url, _headers)) => url,
        Err(error) => {
            let err_str = error.to_string();
            apply_terminal_upstream_failure(TerminalUpstreamFailureParams {
                proxy,
                lb: None,
                target,
                error_class: "target_build_error",
                penalize_reason: None,
                cooldown_secs: transport_cooldown_secs,
                cooldown_backoff,
                error_message: err_str.clone(),
                avoid_set,
                avoided_total,
                last_err,
            })
            .await;
            record_error_route_attempt(
                upstream_chain,
                route_attempts,
                ErrorRouteAttemptParams {
                    target,
                    route_attempt_index,
                    kind: RouteAttemptErrorKind::TargetBuild,
                    reason: err_str.as_str(),
                    model_note,
                    duration_ms: None,
                    cooldown_secs: Some(transport_cooldown_secs),
                    cooldown_reason: Some("target_build_error"),
                },
            );
            return AttemptTransportOutcome::TryNextUpstream;
        }
    };

    let attempt_request = prepare_attempt_request(AttemptRequestSetupParams {
        proxy,
        auth: &target.upstream().auth,
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
        .update_request_route(
            request_id,
            target.compatibility_station_name().map(ToOwned::to_owned),
            provider_id
                .map(ToOwned::to_owned)
                .or_else(|| target.provider_id().map(ToOwned::to_owned)),
            target.upstream().base_url.clone(),
            Some(route_decision.clone()),
        )
        .await;
    let debug_base = attempt_request.debug_base;

    let builder = proxy
        .client
        .request(method.clone(), target_url.clone())
        .headers(headers)
        .body(filtered_body.clone());

    let upstream_start = Instant::now();
    let response = match builder.send().await {
        Ok(response) => response,
        Err(error) => {
            let err_str = format_reqwest_error_for_retry_chain(&error);
            let can_retry_upstream = upstream_attempt + 1 < upstream_opt.max_attempts
                && should_retry_class(upstream_opt, Some("upstream_transport_error"));
            record_error_route_attempt(
                upstream_chain,
                route_attempts,
                ErrorRouteAttemptParams {
                    target,
                    route_attempt_index,
                    kind: RouteAttemptErrorKind::Transport,
                    reason: err_str.as_str(),
                    model_note,
                    duration_ms: Some(upstream_start.elapsed().as_millis() as u64),
                    cooldown_secs: (!can_retry_upstream).then_some(transport_cooldown_secs),
                    cooldown_reason: (!can_retry_upstream).then_some("upstream_transport_error"),
                },
            );
            if can_retry_upstream {
                backoff_sleep(upstream_opt, upstream_attempt).await;
                return AttemptTransportOutcome::RetrySameUpstream;
            }

            apply_terminal_upstream_failure(TerminalUpstreamFailureParams {
                proxy,
                lb: legacy_lb,
                target,
                error_class: "upstream_transport_error",
                penalize_reason: Some("upstream_transport_error"),
                cooldown_secs: transport_cooldown_secs,
                cooldown_backoff,
                error_message: err_str,
                avoid_set,
                avoided_total,
                last_err,
            })
            .await;
            return AttemptTransportOutcome::TryNextUpstream;
        }
    };

    AttemptTransportOutcome::Continue(Box::new(AttemptTransportSuccess {
        response,
        upstream_start,
        upstream_headers_ms: upstream_start.elapsed().as_millis() as u64,
        debug_base,
    }))
}

pub(super) async fn read_attempt_response_body(
    params: AttemptReadBodyParams<'_>,
) -> AttemptReadBodyOutcome {
    let AttemptReadBodyParams {
        proxy,
        legacy_lb,
        target,
        response,
        upstream_opt,
        upstream_attempt,
        transport_cooldown_secs,
        cooldown_backoff,
        avoid_set,
        avoided_total,
        last_err,
        upstream_chain,
        route_attempts,
        route_attempt_index,
        model_note,
    } = params;

    match read_response_body_with_limit(response).await {
        Ok(bytes) => AttemptReadBodyOutcome::Continue(bytes),
        Err(error) => {
            let err_str = error.message();
            let (route_kind, can_retry_upstream, error_class, cooldown_reason) = match &error {
                ResponseBodyReadError::Read(_) => {
                    let can_retry_upstream = upstream_attempt + 1 < upstream_opt.max_attempts
                        && should_retry_class(upstream_opt, Some("upstream_transport_error"));
                    (
                        RouteAttemptErrorKind::BodyRead,
                        can_retry_upstream,
                        "upstream_body_read_error",
                        "upstream_body_read_error",
                    )
                }
                ResponseBodyReadError::TooLarge { .. } => (
                    RouteAttemptErrorKind::BodyTooLarge,
                    false,
                    "upstream_response_body_too_large",
                    "upstream_response_body_too_large",
                ),
            };
            record_error_route_attempt(
                upstream_chain,
                route_attempts,
                ErrorRouteAttemptParams {
                    target,
                    route_attempt_index,
                    kind: route_kind,
                    reason: err_str.as_str(),
                    model_note,
                    duration_ms: None,
                    cooldown_secs: (!can_retry_upstream).then_some(transport_cooldown_secs),
                    cooldown_reason: (!can_retry_upstream).then_some(cooldown_reason),
                },
            );
            if matches!(error, ResponseBodyReadError::Read(_)) && can_retry_upstream {
                backoff_sleep(upstream_opt, upstream_attempt).await;
                return AttemptReadBodyOutcome::RetrySameUpstream;
            }

            apply_terminal_upstream_failure(TerminalUpstreamFailureParams {
                proxy,
                lb: legacy_lb,
                target,
                error_class,
                penalize_reason: Some(error_class),
                cooldown_secs: transport_cooldown_secs,
                cooldown_backoff,
                error_message: err_str,
                avoid_set,
                avoided_total,
                last_err,
            })
            .await;
            AttemptReadBodyOutcome::TryNextUpstream
        }
    }
}
