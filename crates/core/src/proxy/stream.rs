use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Response, StatusCode};
use futures_util::StreamExt;
use tracing::{info, warn};

use crate::lb::LoadBalancer;
use crate::logging::{
    HttpDebugLog, RetryInfo, log_request_with_debug, make_body_preview, should_include_http_debug,
    should_include_http_warn,
};
use crate::state::ProxyState;
use crate::usage_providers;

use super::classify::classify_upstream_response;
use super::{
    HttpDebugBase, ProxyService, SelectedUpstream, header_map_to_entries, warn_http_debug,
};

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
    config_name: String,
    provider_id: Option<String>,
    upstream_base_url: String,
    retry: Option<RetryInfo>,
    session_id: Option<String>,
    cwd: Option<String>,
    reasoning_effort: Option<String>,
    request_id: u64,
    state: Arc<ProxyState>,
    resp_headers: HeaderMap,
    debug_base: Option<HttpDebugBase>,
    usage_state: Arc<Mutex<StreamUsageState>>,
    lb: LoadBalancer,
    upstream_index: usize,
    transport_cooldown_secs: u64,
    cooldown_backoff: crate::lb::CooldownBackoff,
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
            log_request_with_debug(
                &self.service_name,
                &self.method,
                &self.path,
                self.status_code,
                dur,
                guard.first_chunk_ms,
                &self.config_name,
                self.provider_id.clone(),
                &self.upstream_base_url,
                self.session_id.clone(),
                self.cwd.clone(),
                self.reasoning_effort.clone(),
                usage,
                self.retry.clone(),
                http_debug,
            );
        }

        drop(guard);

        if stream_error {
            self.lb.record_result_with_backoff(
                self.upstream_index,
                false,
                crate::lb::COOLDOWN_SECS,
                self.cooldown_backoff,
            );
            self.lb.penalize_with_backoff(
                self.upstream_index,
                self.transport_cooldown_secs,
                "upstream_stream_error",
                self.cooldown_backoff,
            );
        }

        tokio::spawn(async move {
            state
                .finish_request(crate::state::FinishRequestParams {
                    id: request_id,
                    status_code,
                    duration_ms: dur,
                    ended_at_ms: started_at_ms + dur,
                    usage: usage_for_state,
                    retry: retry_for_state,
                    ttfb_ms: ttfb_ms_for_state,
                })
                .await;
        });
    }
}

pub(super) async fn build_sse_success_response(
    proxy: &ProxyService,
    lb: LoadBalancer,
    selected: SelectedUpstream,
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
        request_id,
        is_user_turn,
        is_codex_service,
        transport_cooldown_secs,
        cooldown_backoff,
        method,
        path,
    } = meta;

    if is_user_turn {
        let provider_id = selected
            .upstream
            .tags
            .get("provider_id")
            .map(|s| s.as_str())
            .unwrap_or("-");
        info!(
            "user turn {} {} using config '{}' upstream[{}] provider_id='{}' base_url='{}'",
            method,
            path,
            selected.config_name,
            selected.index,
            provider_id,
            selected.upstream.base_url
        );
    }

    let max_keep = stream_buffer_max_bytes();
    let usage_state = Arc::new(Mutex::new(StreamUsageState::default()));
    let usage_state_inner = usage_state.clone();
    let method_s = method.to_string();
    let path_s = path.clone();
    let config_name = selected.config_name.clone();
    let provider_id = selected.upstream.tags.get("provider_id").cloned();
    let base_url = selected.upstream.base_url.clone();
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
        config_name: config_name.clone(),
        provider_id: provider_id.clone(),
        upstream_base_url: base_url.clone(),
        retry: retry.clone(),
        session_id: session_id.clone(),
        cwd: cwd.clone(),
        reasoning_effort: effective_effort.clone(),
        request_id,
        state: proxy.state.clone(),
        resp_headers: resp_headers.clone(),
        debug_base,
        usage_state: usage_state.clone(),
        lb: lb.clone(),
        upstream_index: selected.index,
        transport_cooldown_secs,
        cooldown_backoff,
    };

    if is_user_turn && is_codex_service {
        let cfg_snapshot = proxy.config.snapshot().await;
        tokio::spawn({
            let cfg = cfg_snapshot;
            let lb_states = proxy.lb_states.clone();
            let config_name = selected.config_name.clone();
            let upstream_index = selected.index;
            async move {
                usage_providers::poll_for_codex_upstream(
                    cfg,
                    lb_states,
                    &config_name,
                    upstream_index,
                )
                .await;
            }
        });
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

                // Keep collecting / scanning even for long streams; cap memory by discarding already-scanned bytes.
                if guard.buffer.len().saturating_add(chunk.len()) > max_keep && guard.usage_scan_pos > 0 {
                    let pos = guard.usage_scan_pos;
                    guard.buffer.drain(..pos);
                    guard.usage_scan_pos = 0;
                }
                if chunk.len() > max_keep {
                    // Extremely large chunks are unexpected; keep the tail to avoid unbounded growth.
                    guard.buffer.clear();
                    guard.buffer
                        .extend_from_slice(&chunk[chunk.len().saturating_sub(max_keep)..]);
                    guard.usage_scan_pos = 0;
                } else {
                    guard.buffer.extend_from_slice(&chunk);
                }
                if !guard.warned_non_success && !(200..300).contains(&status_code) {
                    if should_include_http_warn(status_code)
                        && let Some(h) =
                            _finalize.build_http_debug(&guard.buffer, guard.first_chunk_ms, true)
                    {
                        warn_http_debug(status_code, &h);
                    } else {
                        warn!(
                            "upstream returned non-2xx status {} for {} {} (config: {}); set CODEX_HELPER_HTTP_WARN=0 to disable preview logs (or CODEX_HELPER_HTTP_DEBUG=1 for full debug)",
                            status_code, method_s, path_s, config_name
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
                        ..
                    } = &mut *guard;
                    crate::usage::scan_usage_from_sse_bytes_incremental(
                        buffer.as_slice(),
                        usage_scan_pos,
                        usage,
                    );
                }
                // If the buffer grew large, drop already-scanned bytes to cap memory.
                if guard.buffer.len() > max_keep && guard.usage_scan_pos > 0 {
                    let pos = guard.usage_scan_pos;
                    guard.buffer.drain(..pos);
                    guard.usage_scan_pos = 0;
                }
                if let Some(usage) = guard.usage.clone() {
                    guard.logged = true;
                    let dur = start_time.elapsed().as_millis() as u64;
                    let http_debug = if should_include_http_debug(status_code) {
                        _finalize.build_http_debug(&guard.buffer, guard.first_chunk_ms, false)
                    } else {
                        None
                    };
                    log_request_with_debug(
                        &service_name,
                        &method_s,
                        &path_s,
                        status.as_u16(),
                        dur,
                        guard.first_chunk_ms,
                        &config_name,
                        provider_id.clone(),
                        &base_url,
                        session_id.clone(),
                        cwd.clone(),
                        effective_effort.clone(),
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
                    "upstream stream error: {} {} status={} config={} base_url={} err={}",
                    method_s, path_s, status_code, config_name, base_url, e
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
    pub(super) request_id: u64,
    pub(super) is_user_turn: bool,
    pub(super) is_codex_service: bool,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cooldown_backoff: crate::lb::CooldownBackoff,
    pub(super) method: Method,
    pub(super) path: String,
}
