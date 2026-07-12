use std::io;
use std::pin::Pin;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode};
use futures_util::{Stream, StreamExt, stream};
use tracing::{info, warn};

use crate::lb::LoadBalancer;
use crate::logging::{
    CodexBridgeLog, HttpDebugLog, RetryInfo, ServiceTierLog, log_retry_trace, make_body_preview,
    request_trace_id, should_include_http_debug, should_include_http_warn,
};
use crate::state::{ProxyState, RouteDecisionProvenance, SessionIdentitySource};
use crate::usage_providers;

use super::ProxyService;
use super::attempt_health::{
    penalize_attempt_target, record_attempt_failure, record_attempt_success,
};
use super::attempt_target::AttemptTarget;
use super::classify::{
    UPSTREAM_OVERLOADED_CLASS, UPSTREAM_RATE_LIMITED_CLASS, classify_observed_upstream_response,
};
use super::codex_failure::CodexFailureSse;
use super::concurrency_limits::ConcurrencyPermit;
use super::headers::header_map_to_entries;
use super::http_debug::{HttpDebugBase, warn_http_debug};
use super::passive_health::{record_passive_upstream_failure, record_passive_upstream_success};
use super::provider_evidence::{ResponseEvidenceParams, response_evidence_from_classification};
use super::request_body::scan_service_tier_from_sse_bytes_incremental;
use super::request_observer::{RequestObserver, RequestPublication, RequestPublicationGate};
use super::retry::response_penalty_cooldown_secs;

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

const DEFAULT_CODEX_STREAM_IDLE_TIMEOUT_SECS: u64 = 15 * 60;
const MAX_CODEX_STREAM_IDLE_TIMEOUT_SECS: u64 = 24 * 60 * 60;

fn codex_responses_stream_idle_timeout(is_codex_service: bool, path: &str) -> Option<Duration> {
    if !is_codex_service || !is_codex_responses_sse_path(path) {
        return None;
    }

    let secs = std::env::var("CODEX_HELPER_STREAM_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_CODEX_STREAM_IDLE_TIMEOUT_SECS);
    if secs == 0 {
        return None;
    }

    Some(Duration::from_secs(
        secs.clamp(1, MAX_CODEX_STREAM_IDLE_TIMEOUT_SECS),
    ))
}

fn is_codex_responses_sse_path(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    path.ends_with("/responses") || path.ends_with("/responses/compact")
}

#[derive(Clone, Debug)]
struct StreamErrorInfo {
    class: &'static str,
    kind: &'static str,
    message: String,
}

#[derive(Default)]
struct StreamUsageState {
    buffer: Vec<u8>,
    terminal_publication: RequestPublicationGate,
    stream_error: Option<StreamErrorInfo>,
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
    FailureAndPenalty {
        error_reason: &'static str,
        cooldown_secs: u64,
    },
}

struct StreamPassiveFailure {
    status_code: Option<u16>,
    error_class: Option<String>,
    error: Option<String>,
}

struct StreamTerminalDecision {
    health_update: Option<StreamHealthUpdate>,
    passive_failure: Option<StreamPassiveFailure>,
}

struct StreamTerminalDecisionParams<'a> {
    status_code: u16,
    stream_error: Option<&'a StreamErrorInfo>,
    resp_headers: &'a HeaderMap,
    resp_content_type: Option<&'a str>,
    response_body: &'a [u8],
    transport_cooldown_secs: u64,
    cloudflare_challenge_cooldown_secs: u64,
    cloudflare_timeout_cooldown_secs: u64,
}

fn stream_penalty_reason(class: Option<&str>) -> &'static str {
    match class {
        Some("cloudflare_challenge") => "cloudflare_challenge",
        Some("cloudflare_timeout") => "cloudflare_timeout",
        Some(UPSTREAM_RATE_LIMITED_CLASS) => UPSTREAM_RATE_LIMITED_CLASS,
        Some(UPSTREAM_OVERLOADED_CLASS) => UPSTREAM_OVERLOADED_CLASS,
        _ => "upstream_response_error",
    }
}

fn classify_stream_read_error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_decode() {
        "decode"
    } else if error.is_body() {
        "body"
    } else if error.is_connect() {
        "connect"
    } else if error.is_request() {
        "request"
    } else {
        "other"
    }
}

fn decide_stream_terminal_response(
    params: StreamTerminalDecisionParams<'_>,
) -> StreamTerminalDecision {
    let StreamTerminalDecisionParams {
        status_code,
        stream_error,
        resp_headers,
        resp_content_type,
        response_body,
        transport_cooldown_secs,
        cloudflare_challenge_cooldown_secs,
        cloudflare_timeout_cooldown_secs,
    } = params;

    if let Some(stream_error) = stream_error {
        return StreamTerminalDecision {
            health_update: Some(StreamHealthUpdate::FailureAndPenalty {
                error_reason: stream_error.class,
                cooldown_secs: transport_cooldown_secs,
            }),
            passive_failure: Some(StreamPassiveFailure {
                status_code: Some(status_code),
                error_class: Some(stream_error.class.to_string()),
                error: Some(format!("{}: {}", stream_error.kind, stream_error.message)),
            }),
        };
    }

    if (200..300).contains(&status_code) {
        return StreamTerminalDecision {
            health_update: Some(StreamHealthUpdate::Success),
            passive_failure: None,
        };
    }

    let classified_response =
        classify_observed_upstream_response(status_code, resp_headers, response_body);
    let retry_after_secs = classified_response.retry_after_secs();
    let cls = classified_response.class;
    let error_class = cls.clone();
    let penalty_cooldown_secs = response_penalty_cooldown_secs(
        cloudflare_challenge_cooldown_secs,
        cloudflare_timeout_cooldown_secs,
        transport_cooldown_secs,
        cls.as_deref(),
        retry_after_secs,
    );
    let health_update = if matches!(
        cls.as_deref(),
        Some("cloudflare_challenge")
            | Some("cloudflare_timeout")
            | Some(UPSTREAM_RATE_LIMITED_CLASS)
            | Some(UPSTREAM_OVERLOADED_CLASS)
    ) {
        Some(StreamHealthUpdate::FailureAndPenalty {
            error_reason: stream_penalty_reason(cls.as_deref()),
            cooldown_secs: penalty_cooldown_secs,
        })
    } else if status_code >= 500 || cls.is_some() {
        Some(StreamHealthUpdate::Failure)
    } else {
        None
    };
    let passive_failure = Some(StreamPassiveFailure {
        status_code: Some(status_code),
        error_class,
        error: summarize_error_body(response_body, resp_content_type),
    });

    StreamTerminalDecision {
        health_update,
        passive_failure,
    }
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
    session_identity_source: Option<SessionIdentitySource>,
    cwd: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: ServiceTierLog,
    codex_bridge: Option<CodexBridgeLog>,
    route_decision: Option<RouteDecisionProvenance>,
    request_id: u64,
    state: Arc<ProxyState>,
    resp_headers: HeaderMap,
    debug_base: Option<HttpDebugBase>,
    usage_state: Arc<Mutex<StreamUsageState>>,
    legacy_lb: Option<LoadBalancer>,
    target: AttemptTarget,
    transport_cooldown_secs: u64,
    cloudflare_challenge_cooldown_secs: u64,
    cloudflare_timeout_cooldown_secs: u64,
    cooldown_backoff: crate::lb::CooldownBackoff,
    _concurrency_permit: Option<ConcurrencyPermit>,
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
        let classified_response =
            classify_observed_upstream_response(self.status_code, &self.resp_headers, body);
        Some(HttpDebugLog {
            request_body_len: Some(self.request_body_len),
            upstream_request_body_len: Some(self.upstream_request_body_len),
            upstream_headers_ms: Some(self.upstream_headers_ms),
            upstream_first_chunk_ms: first_chunk_ms,
            upstream_body_read_ms: None,
            upstream_error_class: classified_response.class,
            upstream_error_hint: classified_response.hint,
            upstream_cf_ray: classified_response.cf_ray,
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
        if !guard.terminal_publication.mark_published() {
            return;
        }
        let usage = guard.usage.clone();
        let ttfb_ms = guard.first_chunk_ms;
        let service_tier = ServiceTierLog {
            actual: guard.service_tier.clone(),
            ..self.service_tier.clone()
        };
        let stream_error = guard.stream_error.clone();

        let dur = self.start.elapsed().as_millis() as u64;

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

        let response_body = std::mem::take(&mut guard.buffer);
        drop(guard);
        let resp_ct = self
            .resp_headers
            .get("content-type")
            .and_then(|value| value.to_str().ok());
        let transport_cooldown_secs = self.transport_cooldown_secs;
        let terminal_decision = decide_stream_terminal_response(StreamTerminalDecisionParams {
            status_code,
            stream_error: stream_error.as_ref(),
            resp_headers: &self.resp_headers,
            resp_content_type: resp_ct,
            response_body: response_body.as_slice(),
            transport_cooldown_secs,
            cloudflare_challenge_cooldown_secs: self.cloudflare_challenge_cooldown_secs,
            cloudflare_timeout_cooldown_secs: self.cloudflare_timeout_cooldown_secs,
        });
        let health_update = terminal_decision.health_update;
        let passive_failure = terminal_decision.passive_failure;
        let classified_response =
            classify_observed_upstream_response(status_code, &self.resp_headers, &response_body);
        let evidence_route_facing = matches!(
            &health_update,
            Some(StreamHealthUpdate::FailureAndPenalty { .. })
        );
        let provider_evidence = response_evidence_from_classification(ResponseEvidenceParams {
            target: &self.target,
            classified_response: &classified_response,
            status_code,
            error_class: stream_error
                .as_ref()
                .map(|error| error.class)
                .or(classified_response.class.as_deref()),
            route_facing: evidence_route_facing,
            default_cooldown_secs: match &health_update {
                Some(StreamHealthUpdate::FailureAndPenalty { cooldown_secs, .. }) => *cooldown_secs,
                _ => transport_cooldown_secs,
            },
        });

        let state_for_passive = state.clone();
        let service_name_for_passive = self.service_name.clone();
        let station_name_for_passive = self.compatibility_station_name.clone();
        let base_url_for_passive = self.upstream_base_url.clone();
        let legacy_lb_for_health = self.legacy_lb.clone();
        let target_for_health = self.target.clone();
        let cooldown_backoff = self.cooldown_backoff;
        let observer = RequestObserver::from_parts(
            state.clone(),
            self.service_name.clone(),
            self.method.clone(),
            self.path.clone(),
        );
        let mut publication =
            RequestPublication::new_terminal(request_id, status_code, dur, started_at_ms, true);
        publication.ttfb_ms = ttfb_ms;
        publication.station_name = self.compatibility_station_name.clone();
        publication.provider_id = self.provider_id.clone();
        publication.endpoint_id = self.endpoint_id.clone();
        publication.provider_endpoint_key = self.provider_endpoint_key.clone();
        publication.upstream_base_url = self.upstream_base_url.clone();
        publication.session_id = self.session_id.clone();
        publication.session_identity_source = self.session_identity_source;
        publication.cwd = self.cwd.clone();
        publication.reasoning_effort = self.reasoning_effort.clone();
        publication.service_tier = service_tier;
        publication.codex_bridge = self.codex_bridge.clone();
        publication.usage = usage;
        publication.route_decision = self.route_decision.clone();
        publication.retry = self.retry.clone();
        if let Some(retry) = publication.retry.as_mut()
            && let Some(last_attempt) = retry.route_attempts.last_mut()
            && (last_attempt.provider_endpoint_key.as_deref()
                == self.provider_endpoint_key.as_deref()
                || (last_attempt.provider_signals.is_empty()
                    && last_attempt.policy_actions.is_empty()))
        {
            last_attempt.provider_signals = provider_evidence.signals.clone();
            last_attempt.policy_actions = provider_evidence.actions.clone();
        }
        publication.http_debug = http_debug;
        let publication = publication.with_route_decision_model();

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
                Some(StreamHealthUpdate::FailureAndPenalty {
                    error_reason,
                    cooldown_secs,
                }) => {
                    provider_evidence
                        .apply_to_state(service_name_for_passive.as_str(), state.as_ref())
                        .await;
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
                        cooldown_secs,
                        error_reason,
                        cooldown_backoff,
                    )
                    .await;
                }
                None => {}
            }
            if let Some(station_name_for_passive) = station_name_for_passive.as_deref() {
                if let Some(passive_failure) = passive_failure {
                    record_passive_upstream_failure(
                        &state_for_passive,
                        &service_name_for_passive,
                        station_name_for_passive,
                        &base_url_for_passive,
                        passive_failure.status_code,
                        passive_failure.error_class.as_deref(),
                        passive_failure.error,
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
            observer.publish_terminal_once(publication).await;
        });
    }
}

struct StreamForwardState {
    upstream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    finalize: StreamFinalize,
    idle_timeout: Option<Duration>,
    terminal_after_current_item: bool,
    max_keep: usize,
}

impl StreamForwardState {
    async fn next_body_item(mut self) -> Option<(Result<Bytes, io::Error>, Self)> {
        if self.terminal_after_current_item {
            return None;
        }

        let upstream_item = match self.idle_timeout {
            Some(timeout) => match tokio::time::timeout(timeout, self.upstream.next()).await {
                Ok(item) => item,
                Err(_) => {
                    let item = self.handle_idle_timeout(timeout);
                    return Some((item, self));
                }
            },
            None => self.upstream.next().await,
        };

        match upstream_item {
            Some(Ok(chunk)) => Some((Ok(self.handle_chunk(chunk)), self)),
            Some(Err(error)) => {
                let item = self.handle_read_error(error);
                Some((item, self))
            }
            None => None,
        }
    }

    fn handle_chunk(&mut self, chunk: Bytes) -> Bytes {
        let finalize = &self.finalize;
        let mut guard = match finalize.usage_state.lock() {
            Ok(g) => g,
            Err(_) => return chunk,
        };
        if guard.first_chunk_ms.is_none() {
            guard.first_chunk_ms = Some(finalize.upstream_start.elapsed().as_millis() as u64);
        }

        if chunk.len() > self.max_keep {
            // Extremely large chunks are unexpected; keep the tail to avoid unbounded growth.
            guard.buffer.clear();
            guard
                .buffer
                .extend_from_slice(&chunk[chunk.len().saturating_sub(self.max_keep)..]);
            guard.usage_scan_pos = 0;
            guard.service_tier_scan_pos = 0;
        } else {
            guard.buffer.extend_from_slice(&chunk);
            trim_stream_buffer(&mut guard, self.max_keep);
        }
        if !guard.warned_non_success && !(200..300).contains(&finalize.status_code) {
            if should_include_http_warn(finalize.status_code)
                && let Some(h) =
                    finalize.build_http_debug(&guard.buffer, guard.first_chunk_ms, true)
            {
                warn_http_debug(finalize.status_code, &h);
            } else {
                warn!(
                    "upstream returned non-2xx status {} for {} {} (target: {}); set CODEX_HELPER_HTTP_WARN=0 to disable preview logs (or CODEX_HELPER_HTTP_DEBUG=1 for full debug)",
                    finalize.status_code,
                    finalize.method,
                    finalize.path,
                    finalize.target.log_target_label()
                );
            }
            guard.warned_non_success = true;
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

        chunk
    }

    fn handle_read_error(&mut self, error: reqwest::Error) -> Result<Bytes, io::Error> {
        let kind = classify_stream_read_error_kind(&error);
        let message = format!("Upstream stream failed: {error}");
        self.handle_stream_failure("upstream_stream_error", kind, message, io::ErrorKind::Other)
    }

    fn handle_idle_timeout(&mut self, timeout: Duration) -> Result<Bytes, io::Error> {
        let message = format!(
            "Upstream stream idle timeout after {}s without bytes",
            timeout.as_secs()
        );
        self.handle_stream_failure(
            "upstream_stream_idle_timeout",
            "idle_timeout",
            message,
            io::ErrorKind::TimedOut,
        )
    }

    fn handle_stream_failure(
        &mut self,
        class: &'static str,
        kind: &'static str,
        message: String,
        io_kind: io::ErrorKind,
    ) -> Result<Bytes, io::Error> {
        let (first_chunk_ms, buffered_bytes) =
            if let Ok(mut guard) = self.finalize.usage_state.lock() {
                let first_chunk_ms = guard.first_chunk_ms;
                let buffered_bytes = guard.buffer.len();
                guard.stream_error = Some(StreamErrorInfo {
                    class,
                    kind,
                    message: message.clone(),
                });
                (first_chunk_ms, buffered_bytes)
            } else {
                (None, 0)
            };

        let trace_id = request_trace_id(
            self.finalize.service_name.as_str(),
            self.finalize.request_id,
        );
        log_retry_trace(serde_json::json!({
            "event": "upstream_stream_error",
            "service": self.finalize.service_name,
            "request_id": self.finalize.request_id,
            "trace_id": trace_id,
            "session_id": self.finalize.session_id,
            "method": self.finalize.method,
            "path": self.finalize.path,
            "status": self.finalize.status_code,
            "target": self.finalize.target.log_target_label(),
            "provider_endpoint_key": self.finalize.provider_endpoint_key,
            "base_url": self.finalize.upstream_base_url,
            "class": class,
            "stream_error_kind": kind,
            "first_chunk_ms": first_chunk_ms,
            "buffered_bytes": buffered_bytes,
            "error": message,
        }));

        warn!(
            request_id = self.finalize.request_id,
            trace_id = %trace_id,
            session_id = self.finalize.session_id.as_deref().unwrap_or("-"),
            method = %self.finalize.method,
            path = %self.finalize.path,
            status = self.finalize.status_code,
            target = %self.finalize.target.log_target_label(),
            base_url = %self.finalize.upstream_base_url,
            error_class = class,
            stream_error_kind = kind,
            first_chunk_ms = ?first_chunk_ms,
            buffered_bytes,
            error = %message,
            "upstream stream error"
        );

        self.terminal_after_current_item = true;
        if self.finalize.service_name == "codex" && is_codex_responses_sse_path(&self.finalize.path)
        {
            let model = self
                .finalize
                .route_decision
                .as_ref()
                .and_then(|decision| decision.effective_model.as_ref())
                .map(|model| model.value.as_str());
            return Ok(Bytes::from(
                CodexFailureSse::stream_error(&message, model, class).to_event_string(),
            ));
        }

        Err(io::Error::new(io_kind, message))
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
        session_identity_source,
        cwd,
        effective_effort,
        service_tier,
        codex_bridge,
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
        route_decision,
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
    let method_s = method.to_string();
    let path_s = path.clone();
    let compatibility_station_name = target.compatibility_station_name().map(ToOwned::to_owned);
    let provider_id = target.provider_id().map(ToOwned::to_owned);
    let endpoint_id = target.endpoint_id();
    let provider_endpoint_key = target.provider_endpoint_key();
    let base_url = target.upstream().base_url.clone();
    let service_name = proxy.service_name.to_string();
    let status_code = status.as_u16();
    let stream_idle_timeout = codex_responses_stream_idle_timeout(is_codex_service, &path_s);

    let finalize = StreamFinalize {
        service_name: service_name.clone(),
        method: method_s.clone(),
        path: path_s.clone(),
        status_code,
        start,
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
        session_identity_source,
        cwd: cwd.clone(),
        reasoning_effort: effective_effort.clone(),
        service_tier,
        codex_bridge: codex_bridge.clone(),
        route_decision,
        request_id,
        state: proxy.state.clone(),
        resp_headers: resp_headers.clone(),
        debug_base,
        usage_state: usage_state.clone(),
        legacy_lb: legacy_lb.clone(),
        target: target.clone(),
        transport_cooldown_secs,
        cloudflare_challenge_cooldown_secs,
        cloudflare_timeout_cooldown_secs,
        cooldown_backoff,
        _concurrency_permit: concurrency_permit,
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
        }
    }

    let stream_state = StreamForwardState {
        upstream: Box::pin(resp.bytes_stream()),
        finalize,
        idle_timeout: stream_idle_timeout,
        terminal_after_current_item: false,
        max_keep,
    };
    let stream = stream::unfold(
        stream_state,
        |state| async move { state.next_body_item().await },
    );

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
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) cwd: Option<String>,
    pub(super) effective_effort: Option<String>,
    pub(super) service_tier: ServiceTierLog,
    pub(super) codex_bridge: Option<CodexBridgeLog>,
    pub(super) route_decision: Option<RouteDecisionProvenance>,
    pub(super) request_id: u64,
    pub(super) is_user_turn: bool,
    pub(super) is_codex_service: bool,
    pub(super) transport_cooldown_secs: u64,
    pub(super) cloudflare_challenge_cooldown_secs: u64,
    pub(super) cloudflare_timeout_cooldown_secs: u64,
    pub(super) cooldown_backoff: crate::lb::CooldownBackoff,
    pub(super) method: Method,
    pub(super) path: String,
    pub(super) concurrency_permit: Option<ConcurrencyPermit>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_terminal_failure_keeps_stream_error_kind_in_passive_error() {
        let headers = HeaderMap::new();
        let stream_error = StreamErrorInfo {
            class: "upstream_stream_error",
            kind: "decode",
            message: "bad sse".to_string(),
        };

        let decision = decide_stream_terminal_response(StreamTerminalDecisionParams {
            status_code: 200,
            stream_error: Some(&stream_error),
            resp_headers: &headers,
            resp_content_type: None,
            response_body: b"",
            transport_cooldown_secs: 30,
            cloudflare_challenge_cooldown_secs: 0,
            cloudflare_timeout_cooldown_secs: 0,
        });

        let passive_failure = decision.passive_failure.expect("passive failure");
        assert_eq!(
            passive_failure.error_class.as_deref(),
            Some("upstream_stream_error")
        );
        assert_eq!(passive_failure.error.as_deref(), Some("decode: bad sse"));
    }
}
