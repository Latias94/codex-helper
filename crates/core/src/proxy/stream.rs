use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Response, StatusCode};
use futures_util::StreamExt;
use tracing::{info, warn};

use crate::lb::LoadBalancer;
use crate::logging::{
    HttpDebugLog, RetryInfo, ServiceTierLog, log_request_with_debug, make_body_preview,
    should_include_http_debug, should_include_http_warn,
};
use crate::state::ProxyState;
use crate::usage_providers;

use super::ProxyService;
use super::attempt_health::{
    penalize_attempt_target, record_attempt_failure, record_attempt_success,
};
use super::attempt_target::AttemptTarget;
use super::classify::classify_upstream_response;
use super::headers::header_map_to_entries;
use super::http_debug::{HttpDebugBase, warn_http_debug};
use super::passive_health::{record_passive_upstream_failure, record_passive_upstream_success};
use super::request_body::scan_service_tier_from_sse_bytes_incremental;

fn stream_buffer_max_bytes() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("CODEX_HELPER_STREAM_BUFFER_MAX_BYTES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(1024 * 1024)
            .clamp(64 * 1024, 32 * 1024 * 1024)
    })
}

#[derive(Default)]
struct StreamUsageState {
    buffer: Vec<u8>,
    logged: bool,
    finished: bool,
    stream_error: bool,
    warned_non_success: bool,
    first_chunk_ms: Option<u64>,
    usage: Option<crate::usage::UsageMetrics>,
    usage_scan_pos: usize,
    service_tier: Option<String>,
    service_tier_scan_pos: usize,
}

enum StreamHealthUpdate {
    Success,
    Failure,
    FailureAndPenalty { error_reason: &'static str },
}

fn trim_stream_buffer(state: &mut StreamUsageState, max_keep: usize) {
    if max_keep == 0 || state.buffer.len() <= max_keep {
        return;
    }

    let drop_len = state.buffer.len().saturating_sub(max_keep);
    state.buffer.drain(..drop_len);
    state.usage_scan_pos = state.usage_scan_pos.saturating_sub(drop_len);
    state.service_tier_scan_pos = state.service_tier_scan_pos.saturating_sub(drop_len);
}

struct StreamFinalize {
    service_name: String,
    method: String,
    path: String,
    status_code: u16,
    start: Instant,
    started_at_ms: u64,
    upstream_start: Instant,
    upstream_headers_ms: u64,
    request_body_len: usize,
    upstream_request_body_len: usize,
    compatibility_station_name: Option<String>,
    provider_id: Option<String>,
    endpoint_id: Option<String>,
    provider_endpoint_key: Option<String>,
    upstream_base_url: String,
    retry: Option<RetryInfo>,
    session_id: Option<String>,
    cwd: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: ServiceTierLog,
    request_id: u64,
    state: Arc<ProxyState>,
    resp_headers: HeaderMap,
    debug_base: Option<HttpDebugBase>,
    usage_state: Arc<Mutex<StreamUsageState>>,
    legacy_lb: Option<LoadBalancer>,
    target: AttemptTarget,
    transport_cooldown_secs: u64,
    cooldown_backoff: crate::lb::CooldownBackoff,
}

fn summarize_error_body(body: &[u8], content_type: Option<&str>) -> Option<String> {
    if body.is_empty() {
        return None;
    }

    let preview = make_body_preview(body, content_type, 2048);
    Some(if preview.encoding == "utf8" {
        if preview.truncated {
            format!("{}…", preview.data)
        } else {
            preview.data
        }
    } else {
        format!("binary response body ({} bytes)", preview.original_len)
    })
}

impl StreamFinalize {
    fn build_http_debug(
        &self,
        body: &[u8],
        first_chunk_ms: Option<u64>,
        for_warn: bool,
    ) -> Option<HttpDebugLog> {
        let b = self.debug_base.as_ref()?;
        let max = if for_warn {
            b.warn_max_body_bytes
        } else {
            b.debug_max_body_bytes
        };
        if max == 0 {
            return None;
        }
        let resp_ct = self
            .resp_headers
            .get("content-type")
            .and_then(|v| v.to_str().ok());
        let (client_body, upstream_request_body) = if for_warn {
            (
                b.client_body_warn.clone(),
                b.upstream_request_body_warn.clone(),
            )
        } else {
            (
                b.client_body_debug.clone(),
                b.upstream_request_body_debug.clone(),
            )
        };
        let (cls, hint, cf_ray) =
            classify_upstream_response(self.status_code, &self.resp_headers, body);
        Some(HttpDebugLog {
            request_body_len: Some(self.request_body_len),
            upstream_request_body_len: Some(self.upstream_request_body_len),
            upstream_headers_ms: Some(self.upstream_headers_ms),
            upstream_first_chunk_ms: first_chunk_ms,
            upstream_body_read_ms: None,
            upstream_error_class: cls,
            upstream_error_hint: hint,
            upstream_cf_ray: cf_ray,
            client_uri: b.client_uri.clone(),
            target_url: b.target_url.clone(),
            client_headers: b.client_headers.clone(),
            upstream_request_headers: b.upstream_request_headers.clone(),
            auth_resolution: b.auth_resolution.clone(),
            client_body,
            upstream_request_body,
            upstream_response_headers: Some(header_map_to_entries(&self.resp_headers)),
            upstream_response_body: Some(make_body_preview(body, resp_ct, max)),
            upstream_error: None,
        })
    }
}

impl Drop for StreamFinalize {
    fn drop(&mut self) {
        let state = self.state.clone();
        let request_id = self.request_id;
        let status_code = self.status_code;
        let started_at_ms = self.started_at_ms;

        let mut guard = match self.usage_state.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if guard.finished {
            return;
        }
        guard.finished = true;
        let already_logged = guard.logged;
        let usage_for_state = guard.usage.clone();
        let retry_for_state = self.retry.clone();
        let ttfb_ms_for_state = guard.first_chunk_ms;
        let service_tier_for_state = guard.service_tier.clone();
        let stream_error = guard.stream_error;

        let dur = self.start.elapsed().as_millis() as u64;

        if !already_logged {
            guard.logged = true;
            let usage = usage_for_state.clone();
            let http_debug_warn = self.build_http_debug(&guard.buffer, guard.first_chunk_ms, true);
            if should_include_http_warn(self.status_code)
                && !guard.warned_non_success
                && let Some(h) = http_debug_warn.as_ref()
            {
                warn_http_debug(self.status_code, h);
                guard.warned_non_success = true;
            }
            let http_debug = if should_include_http_debug(self.status_code) {
                self.build_http_debug(&guard.buffer, guard.first_chunk_ms, false)
            } else {
                None
            };
            let service_tier = ServiceTierLog {
                actual: guard.service_tier.clone(),
                ..self.service_tier.clone()
            };
            log_request_with_debug(
                Some(request_id),
                &self.service_name,
                &self.method,
                &self.path,
                self.status_code,
                dur,
                guard.first_chunk_ms,
                self.compatibility_station_name.as_deref(),
                self.provider_id.clone(),
                self.endpoint_id.clone(),
                self.provider_endpoint_key.clone(),
                &self.upstream_base_url,
                self.session_id.clone(),
                self.cwd.clone(),
                self.reasoning_effort.clone(),
                service_tier,
                usage,
                self.retry.clone(),
                http_debug,
            );
        }

        let response_body = std::mem::take(&mut guard.buffer);
        drop(guard);
        let resp_ct = self
            .resp_headers
            .get("content-type")
            .and_then(|value| value.to_str().ok());

        let health_update = if stream_error {
            Some(StreamHealthUpdate::FailureAndPenalty {
                error_reason: "upstream_stream_error",
            })
        } else if (200..300).contains(&status_code) {
            Some(StreamHealthUpdate::Success)
        } else {
            let (cls, _hint, _cf_ray) = classify_upstream_response(
                status_code,
                &self.resp_headers,
                response_body.as_slice(),
            );
            if status_code >= 500 || cls.is_some() {
                Some(StreamHealthUpdate::Failure)
            } else {
                None
            }
        };

        let passive_failure = if stream_error {
            Some((
                Some(status_code),
                Some("upstream_stream_error".to_string()),
                summarize_error_body(response_body.as_slice(), resp_ct),
            ))
        } else if (200..300).contains(&status_code) {
            None
        } else {
            let (cls, _hint, _cf_ray) = classify_upstream_response(
                status_code,
                &self.resp_headers,
                response_body.as_slice(),
            );
            Some((
                Some(status_code),
                cls,
                summarize_error_body(response_body.as_slice(), resp_ct),
            ))
        };

        let state_for_passive = state.clone();
        let service_name_for_passive = self.service_name.clone();
        let station_name_for_passive = self.compatibility_station_name.clone();
        let base_url_for_passive = self.upstream_base_url.clone();
        let legacy_lb_for_health = self.legacy_lb.clone();
        let target_for_health = self.target.clone();
        let transport_cooldown_secs = self.transport_cooldown_secs;
        let cooldown_backoff = self.cooldown_backoff;

        tokio::spawn(async move {
            match health_update {
                Some(StreamHealthUpdate::Success) => {
                    record_attempt_success(
                        state.as_ref(),
                        service_name_for_passive.as_str(),
                        legacy_lb_for_health.as_ref(),
                        &target_for_health,
                        crate::lb::COOLDOWN_SECS,
                        cooldown_backoff,
                    )
                    .await;
                }
                Some(StreamHealthUpdate::Failure) => {
                    record_attempt_failure(
                        state.as_ref(),
                        service_name_for_passive.as_str(),
                        legacy_lb_for_health.as_ref(),
                        &target_for_health,
                        crate::lb::COOLDOWN_SECS,
                        cooldown_backoff,
                    )
                    .await;
                }
                Some(StreamHealthUpdate::FailureAndPenalty { error_reason }) => {
                    record_attempt_failure(
                        state.as_ref(),
                        service_name_for_passive.as_str(),
                        legacy_lb_for_health.as_ref(),
                        &target_for_health,
                        crate::lb::COOLDOWN_SECS,
                        cooldown_backoff,
                    )
                    .await;
                    penalize_attempt_target(
                        state.as_ref(),
                        service_name_for_passive.as_str(),
                        legacy_lb_for_health.as_ref(),
                        &target_for_health,
                        transport_cooldown_secs,
                        error_reason,
                        cooldown_backoff,
                    )
                    .await;
                }
                None => {}
            }
            if let Some(station_name_for_passive) = station_name_for_passive.as_deref() {
                if let Some((status_code, error_class, error)) = passive_failure {
                    record_passive_upstream_failure(
                        &state_for_passive,
                        &service_name_for_passive,
                        station_name_for_passive,
                        &base_url_for_passive,
                        status_code,
                        error_class.as_deref(),
                        error,
                    )
                    .await;
                } else {
                    record_passive_upstream_success(
                        &state_for_passive,
                        &service_name_for_passive,
                        station_name_for_passive,
                        &base_url_for_passive,
                        status_code,
                    )
                    .await;
                }
            }
            state
                .finish_request(crate::state::FinishRequestParams {
                    id: request_id,
                    status_code,
                    duration_ms: dur,
                    ended_at_ms: started_at_ms + dur,
                    observed_service_tier: service_tier_for_state,
                    usage: usage_for_state,
                    retry: retry_for_state,
                    ttfb_ms: ttfb_ms_for_state,
                    streaming: true,
                })
                .await;
        });
    }
}

pub(super) async fn build_sse_success_response(
    proxy: &ProxyService,
    legacy_lb: Option<LoadBalancer>,
    target: AttemptTarget,
    resp: reqwest::Response,
    meta: SseSuccessMeta,
) -> Response<Body> {
    let SseSuccessMeta {
        status,
        resp_headers,
        resp_headers_filtered,
        start,
        started_at_ms,
        upstream_start,
        upstream_headers_ms,
        request_body_len,
        upstream_request_body_len,
        debug_base,
        retry,
        session_id,
        cwd,
        effective_effort,
        service_tier,
        request_id,
        is_user_turn,
        is_codex_service,
        transport_cooldown_secs,
        cooldown_backoff,
        method,
        path,
    } = meta;

    if is_user_turn {
        let provider_id = target.provider_id().unwrap_or("-");
        if let Some(provider_endpoint_key) = target.provider_endpoint_key() {
            if let (Some(station_name), Some(upstream_index)) = (
                target.compatibility_station_name(),
                target.compatibility_upstream_index(),
            ) {
                info!(
                    "user turn {} {} using endpoint='{}' provider_id='{}' compat_station='{}' upstream[{}] base_url='{}'",
                    method,
                    path,
                    provider_endpoint_key,
                    provider_id,
                    station_name,
                    upstream_index,
                    target.upstream().base_url
                );
            } else {
                info!(
                    "user turn {} {} using endpoint='{}' provider_id='{}' base_url='{}'",
                    method,
                    path,
                    provider_endpoint_key,
                    provider_id,
                    target.upstream().base_url
                );
            }
        } else {
            info!(
                "user turn {} {} using legacy station='{}' upstream[{}] provider_id='{}' base_url='{}'",
                method,
                path,
                target.compatibility_station_name().unwrap_or("-"),
                target.compatibility_upstream_index().unwrap_or_default(),
                provider_id,
                target.upstream().base_url
            );
        }
    }

    let max_keep = stream_buffer_max_bytes();
    let usage_state = Arc::new(Mutex::new(StreamUsageState::default()));
    let usage_state_inner = usage_state.clone();
    let method_s = method.to_string();
    let path_s = path.clone();
    let target_label = target.log_target_label();
    let compatibility_station_name = target.compatibility_station_name().map(ToOwned::to_owned);
    let compatibility_station_name_for_stream = compatibility_station_name.clone();
    let provider_id = target.provider_id().map(ToOwned::to_owned);
    let endpoint_id = target.endpoint_id();
    let provider_endpoint_key = target.provider_endpoint_key();
    let base_url = target.upstream().base_url.clone();
    let service_name = proxy.service_name.to_string();
    let start_time = start;
    let status_code = status.as_u16();

    let finalize = StreamFinalize {
        service_name: service_name.clone(),
        method: method_s.clone(),
        path: path_s.clone(),
        status_code,
        start: start_time,
        started_at_ms,
        upstream_start,
        upstream_headers_ms,
        request_body_len,
        upstream_request_body_len,
        compatibility_station_name,
        provider_id: provider_id.clone(),
        endpoint_id: endpoint_id.clone(),
        provider_endpoint_key: provider_endpoint_key.clone(),
        upstream_base_url: base_url.clone(),
        retry: retry.clone(),
        session_id: session_id.clone(),
        cwd: cwd.clone(),
        reasoning_effort: effective_effort.clone(),
        service_tier,
        request_id,
        state: proxy.state.clone(),
        resp_headers: resp_headers.clone(),
        debug_base,
        usage_state: usage_state.clone(),
        legacy_lb: legacy_lb.clone(),
        target: target.clone(),
        transport_cooldown_secs,
        cooldown_backoff,
    };

    if is_user_turn && is_codex_service {
        let cfg_snapshot = proxy.config.snapshot().await;
        let lb_states = proxy.lb_states.clone();
        let state = proxy.state.clone();
        let client = proxy.client.clone();
        let service_name = proxy.service_name.to_string();
        if let Some(provider_endpoint) = target.provider_endpoint_ref().cloned() {
            usage_providers::enqueue_poll_for_codex_provider_endpoint(
                client,
                cfg_snapshot,
                lb_states,
                state,
                service_name.as_str(),
                provider_endpoint,
            );
        } else if let (Some(station_name), Some(upstream_index)) = (
            target.compatibility_station_name().map(ToOwned::to_owned),
            target.compatibility_upstream_index(),
        ) {
            usage_providers::enqueue_poll_for_codex_upstream(
                client,
                cfg_snapshot,
                lb_states,
                state,
                service_name.as_str(),
                &station_name,
                upstream_index,
            );
        }
    }

    let stream = resp.bytes_stream().map(move |item| {
        let _finalize = &finalize;

        match item {
            Ok(chunk) => {
                let mut guard = match usage_state_inner.lock() {
                    Ok(g) => g,
                    Err(_) => return Ok(chunk),
                };
                if guard.first_chunk_ms.is_none() {
                    guard.first_chunk_ms = Some(_finalize.upstream_start.elapsed().as_millis() as u64);
                }

                if chunk.len() > max_keep {
                    // Extremely large chunks are unexpected; keep the tail to avoid unbounded growth.
                    guard.buffer.clear();
                    guard.buffer
                        .extend_from_slice(&chunk[chunk.len().saturating_sub(max_keep)..]);
                    guard.usage_scan_pos = 0;
                    guard.service_tier_scan_pos = 0;
                } else {
                    guard.buffer.extend_from_slice(&chunk);
                    trim_stream_buffer(&mut guard, max_keep);
                }
                if !guard.warned_non_success && !(200..300).contains(&status_code) {
                    if should_include_http_warn(status_code)
                        && let Some(h) =
                            _finalize.build_http_debug(&guard.buffer, guard.first_chunk_ms, true)
                    {
                        warn_http_debug(status_code, &h);
                    } else {
                        warn!(
                            "upstream returned non-2xx status {} for {} {} (target: {}); set CODEX_HELPER_HTTP_WARN=0 to disable preview logs (or CODEX_HELPER_HTTP_DEBUG=1 for full debug)",
                            status_code, method_s, path_s, target_label
                        );
                    }
                    guard.warned_non_success = true;
                }
                if guard.logged {
                    return Ok(chunk);
                }
                {
                    let StreamUsageState {
                        buffer,
                        usage_scan_pos,
                        usage,
                        service_tier_scan_pos,
                        service_tier,
                        ..
                    } = &mut *guard;
                    crate::usage::scan_usage_from_sse_bytes_incremental(
                        buffer.as_slice(),
                        usage_scan_pos,
                        usage,
                    );
                    scan_service_tier_from_sse_bytes_incremental(
                        buffer.as_slice(),
                        service_tier_scan_pos,
                        service_tier,
                    );
                }
                if let Some(usage) = guard.usage.clone() {
                    guard.logged = true;
                    let dur = start_time.elapsed().as_millis() as u64;
                    let http_debug = if should_include_http_debug(status_code) {
                        _finalize.build_http_debug(&guard.buffer, guard.first_chunk_ms, false)
                    } else {
                        None
                    };
                    let service_tier = ServiceTierLog {
                        actual: guard.service_tier.clone(),
                        .._finalize.service_tier.clone()
                    };
                    log_request_with_debug(
                        Some(request_id),
                        &service_name,
                        &method_s,
                        &path_s,
                        status.as_u16(),
                        dur,
                        guard.first_chunk_ms,
                        compatibility_station_name_for_stream.as_deref(),
                        provider_id.clone(),
                        endpoint_id.clone(),
                        provider_endpoint_key.clone(),
                        &base_url,
                        session_id.clone(),
                        cwd.clone(),
                        effective_effort.clone(),
                        service_tier,
                        Some(usage),
                        retry.clone(),
                        http_debug,
                    );
                }

                Ok(chunk)
            }
            Err(e) => {
                {
                    let mut guard = match usage_state_inner.lock() {
                        Ok(g) => g,
                        Err(_) => return Err(e),
                    };
                    guard.stream_error = true;
                }
                warn!(
                    "upstream stream error: {} {} status={} target={} base_url={} err={}",
                    method_s, path_s, status_code, target_label, base_url, e
                );
                Err(e)
            }
        }
    });

    let body = Body::from_stream(stream);
    let mut builder = Response::builder().status(status);
    for (name, value) in resp_headers_filtered.iter() {
        builder = builder.header(name, value);
    }
    if resp_headers_filtered.get("content-type").is_none() {
        builder = builder.header("content-type", "text/event-stream");
    }
    builder.body(body).unwrap()
}

pub(super) struct SseSuccessMeta {
    pub(super) status: StatusCode,
    pub(super) resp_headers: HeaderMap,
    pub(super) resp_headers_filtered: HeaderMap,
    pub(super) start: Instant,
    pub(super) started_at_ms: u64,
    pub(super) upstream_start: Instant,
    pub(super) upstream_headers_ms: u64,
    pub(super) request_body_len: usize,
    pub(super) upstream_request_body_len: usize,
    pub(super) debug_base: Option<HttpDebugBase>,
    pub(super) retry: Option<RetryInfo>,
    pub(super) session_id: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) effective_effort: Option<String>,
    pub(super) service_tier: ServiceTierLog,
    pub(super) request_id: u64,
    pub(super) is_user_turn: bool,
    pub(super) is_codex_service: bool,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cooldown_backoff: crate::lb::CooldownBackoff,
    pub(super) method: Method,
    pub(super) path: String,
}
