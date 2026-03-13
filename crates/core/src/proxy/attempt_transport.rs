use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Instant;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};

use crate::lb::{LoadBalancer, SelectedUpstream};
use crate::logging::{BodyPreview, HeaderEntry};
use crate::state::RouteDecisionProvenance;

use super::attempt_failures::apply_terminal_upstream_failure;
use super::attempt_request::{AttemptRequestSetupParams, prepare_attempt_request};
use super::http_debug::HttpDebugBase;
use super::retry::{RetryLayerOptions, backoff_sleep, should_retry_class};
use super::{ProxyService, format_reqwest_error_for_retry_chain};

pub(super) struct AttemptTransportSuccess {
    pub(super) response: reqwest::Response,
    pub(super) upstream_start: Instant,
    pub(super) upstream_headers_ms: u64,
    pub(super) debug_base: Option<HttpDebugBase>,
}

pub(super) enum AttemptTransportOutcome {
    RetrySameUpstream,
    TryNextUpstream,
    Continue(AttemptTransportSuccess),
}

pub(super) enum AttemptReadBodyOutcome {
    RetrySameUpstream,
    TryNextUpstream,
    Continue(Bytes),
}

pub(super) struct AttemptTransportParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) lb: &'a LoadBalancer,
    pub(super) selected: &'a SelectedUpstream,
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
    pub(super) model_note: &'a str,
}

pub(super) struct AttemptReadBodyParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) lb: &'a LoadBalancer,
    pub(super) selected: &'a SelectedUpstream,
    pub(super) response: reqwest::Response,
    pub(super) upstream_opt: &'a RetryLayerOptions,
    pub(super) upstream_attempt: u32,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cooldown_backoff: crate::lb::CooldownBackoff,
    pub(super) avoid_set: &'a mut HashSet<usize>,
    pub(super) avoided_total: &'a mut usize,
    pub(super) last_err: &'a mut Option<(StatusCode, String)>,
    pub(super) upstream_chain: &'a mut Vec<String>,
    pub(super) model_note: &'a str,
}

pub(super) async fn handle_attempt_transport(
    params: AttemptTransportParams<'_>,
) -> AttemptTransportOutcome {
    let AttemptTransportParams {
        proxy,
        lb,
        selected,
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
        model_note,
    } = params;

    let target_url = match proxy.build_target(selected, uri) {
        Ok((url, _headers)) => url,
        Err(error) => {
            let err_str = error.to_string();
            upstream_chain.push(format!(
                "{}:{} (idx={}) target_build_error={} model={}",
                selected.station_name,
                selected.upstream.base_url,
                selected.index,
                err_str,
                model_note
            ));
            apply_terminal_upstream_failure(
                proxy,
                None,
                selected,
                "target_build_error",
                None,
                transport_cooldown_secs,
                cooldown_backoff,
                err_str,
                avoid_set,
                avoided_total,
                last_err,
            )
            .await;
            return AttemptTransportOutcome::TryNextUpstream;
        }
    };

    let attempt_request = prepare_attempt_request(AttemptRequestSetupParams {
        proxy,
        auth: &selected.upstream.auth,
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
            selected.station_name.clone(),
            provider_id.map(ToOwned::to_owned),
            selected.upstream.base_url.clone(),
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
            upstream_chain.push(format!(
                "{}:{} (idx={}) transport_error={} model={}",
                selected.station_name,
                selected.upstream.base_url,
                selected.index,
                err_str,
                model_note
            ));
            let can_retry_upstream = upstream_attempt + 1 < upstream_opt.max_attempts
                && should_retry_class(upstream_opt, Some("upstream_transport_error"));
            if can_retry_upstream {
                backoff_sleep(upstream_opt, upstream_attempt).await;
                return AttemptTransportOutcome::RetrySameUpstream;
            }

            apply_terminal_upstream_failure(
                proxy,
                Some(lb),
                selected,
                "upstream_transport_error",
                Some("upstream_transport_error"),
                transport_cooldown_secs,
                cooldown_backoff,
                err_str,
                avoid_set,
                avoided_total,
                last_err,
            )
            .await;
            return AttemptTransportOutcome::TryNextUpstream;
        }
    };

    AttemptTransportOutcome::Continue(AttemptTransportSuccess {
        response,
        upstream_start,
        upstream_headers_ms: upstream_start.elapsed().as_millis() as u64,
        debug_base,
    })
}

pub(super) async fn read_attempt_response_body(
    params: AttemptReadBodyParams<'_>,
) -> AttemptReadBodyOutcome {
    let AttemptReadBodyParams {
        proxy,
        lb,
        selected,
        response,
        upstream_opt,
        upstream_attempt,
        transport_cooldown_secs,
        cooldown_backoff,
        avoid_set,
        avoided_total,
        last_err,
        upstream_chain,
        model_note,
    } = params;

    match response.bytes().await {
        Ok(bytes) => AttemptReadBodyOutcome::Continue(bytes),
        Err(error) => {
            let err_str = format_reqwest_error_for_retry_chain(&error);
            upstream_chain.push(format!(
                "{}:{} (idx={}) body_read_error={} model={}",
                selected.station_name,
                selected.upstream.base_url,
                selected.index,
                err_str,
                model_note
            ));
            let can_retry_upstream = upstream_attempt + 1 < upstream_opt.max_attempts
                && should_retry_class(upstream_opt, Some("upstream_transport_error"));
            if can_retry_upstream {
                backoff_sleep(upstream_opt, upstream_attempt).await;
                return AttemptReadBodyOutcome::RetrySameUpstream;
            }

            apply_terminal_upstream_failure(
                proxy,
                Some(lb),
                selected,
                "upstream_body_read_error",
                Some("upstream_body_read_error"),
                transport_cooldown_secs,
                cooldown_backoff,
                err_str,
                avoid_set,
                avoided_total,
                last_err,
            )
            .await;
            AttemptReadBodyOutcome::TryNextUpstream
        }
    }
}
