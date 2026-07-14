use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode};
use futures_util::{Stream, StreamExt, stream};
use tracing::{info, warn};

use crate::logging::{
    CodexBridgeLog, HttpDebugLog, RetryInfo, ServiceTierLog, log_control_trace_event,
    make_body_preview, request_trace_id, should_include_http_debug, should_include_http_warn,
    upstream_origin,
};
use crate::runtime_store::{AttemptHandle, AttemptOutcome, EconomicsState};
use crate::sse::{DecodedSseData, decode_sse_event, find_sse_event_end};
use crate::state::{ProxyState, RouteDecisionProvenance, SessionIdentitySource};

use super::ProxyService;
use super::attempt_health::{
    penalize_attempt_target, record_attempt_failure, record_attempt_success,
};
use super::classify::{
    UPSTREAM_OVERLOADED_CLASS, UPSTREAM_RATE_LIMITED_CLASS, classify_observed_upstream_response,
};
use super::codex_failure::CodexFailureSse;
use super::concurrency_limits::ConcurrencyPermit;
use super::headers::header_map_to_entries;
use super::http_debug::{HttpDebugBase, format_reqwest_error_for_retry_chain, warn_http_debug};
use super::provider_evidence::{ResponseEvidenceParams, response_evidence_from_classification};
use super::request_body::merge_response_metadata_from_value;
use super::request_observer::{RequestObserver, RequestPublication, RequestPublicationGate};
use super::retry::response_penalty_cooldown_secs;
use crate::routing_ir::CapturedRouteCandidate;
use crate::state::SessionRouteAffinitySuccess;

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

fn sse_event_max_bytes() -> usize {
    static MAX: OnceLock<usize> = OnceLock::new();
    *MAX.get_or_init(|| {
        std::env::var("CODEX_HELPER_SSE_EVENT_MAX_BYTES")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|&value| value > 0)
            .unwrap_or(64 * 1024 * 1024)
            .clamp(1024 * 1024, 256 * 1024 * 1024)
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
    health: StreamErrorHealth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamErrorHealth {
    Neutral,
    Penalize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StreamProtocolTerminal {
    #[default]
    Pending,
    Succeeded,
    LogicalFailure,
    UpstreamFailure,
}

impl StreamProtocolTerminal {
    fn is_success(self) -> bool {
        matches!(self, Self::Succeeded)
    }

    fn is_upstream_failure(self) -> bool {
        matches!(self, Self::UpstreamFailure)
    }
}

#[derive(Default)]
struct StreamUsageState {
    buffer: VecDeque<u8>,
    terminal_publication: RequestPublicationGate,
    stream_error: Option<StreamErrorInfo>,
    protocol_terminal: StreamProtocolTerminal,
    warned_non_success: bool,
    first_chunk_ms: Option<u64>,
    usage: Option<crate::usage::UsageMetrics>,
    reported_model: Option<String>,
    service_tier: Option<String>,
}

enum StreamHealthUpdate {
    Success,
    Failure,
    FailureAndPenalty { cooldown_secs: u64 },
}

struct StreamTerminalDecision {
    health_update: Option<StreamHealthUpdate>,
}

struct StreamTerminalDecisionParams<'a> {
    status_code: u16,
    stream_error: Option<&'a StreamErrorInfo>,
    protocol_terminal: StreamProtocolTerminal,
    resp_headers: &'a HeaderMap,
    response_body: &'a [u8],
    transport_cooldown_secs: u64,
    cloudflare_challenge_cooldown_secs: u64,
    cloudflare_timeout_cooldown_secs: u64,
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
        protocol_terminal,
        resp_headers,
        response_body,
        transport_cooldown_secs,
        cloudflare_challenge_cooldown_secs,
        cloudflare_timeout_cooldown_secs,
    } = params;

    if let Some(stream_error) = stream_error {
        let health_update = match stream_error.health {
            StreamErrorHealth::Neutral => None,
            StreamErrorHealth::Penalize => Some(StreamHealthUpdate::FailureAndPenalty {
                cooldown_secs: transport_cooldown_secs,
            }),
        };
        return StreamTerminalDecision { health_update };
    }

    if protocol_terminal.is_upstream_failure() {
        return StreamTerminalDecision {
            health_update: Some(StreamHealthUpdate::FailureAndPenalty {
                cooldown_secs: transport_cooldown_secs,
            }),
        };
    }

    if (200..300).contains(&status_code) && protocol_terminal.is_success() {
        return StreamTerminalDecision {
            health_update: Some(StreamHealthUpdate::Success),
        };
    }

    // A downstream cancellation can drop the forwarding stream before the upstream
    // protocol reaches a terminal event. It is a failed lifecycle, but not health
    // evidence against the upstream endpoint.
    if (200..300).contains(&status_code) {
        return StreamTerminalDecision {
            health_update: None,
        };
    }

    let classified_response =
        classify_observed_upstream_response(status_code, resp_headers, response_body);
    let retry_after_secs = classified_response.retry_after_secs();
    let cls = classified_response.class;
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
            cooldown_secs: penalty_cooldown_secs,
        })
    } else if status_code >= 500 || cls.is_some() {
        Some(StreamHealthUpdate::Failure)
    } else {
        None
    };
    StreamTerminalDecision { health_update }
}

fn append_stream_buffer_tail(buffer: &mut VecDeque<u8>, chunk: &[u8], max_keep: usize) {
    if max_keep == 0 {
        buffer.clear();
        return;
    }
    if chunk.len() >= max_keep {
        buffer.clear();
        buffer.extend(&chunk[chunk.len() - max_keep..]);
        return;
    }

    let drop_len = buffer
        .len()
        .saturating_add(chunk.len())
        .saturating_sub(max_keep);
    if drop_len > 0 {
        buffer.drain(..drop_len);
    }
    buffer.extend(chunk);
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
    provider_id: Option<String>,
    endpoint_id: Option<String>,
    provider_endpoint_key: Option<String>,
    upstream_origin: Option<String>,
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
    target: CapturedRouteCandidate,
    transport_cooldown_secs: u64,
    cloudflare_challenge_cooldown_secs: u64,
    cloudflare_timeout_cooldown_secs: u64,
    cooldown_backoff: crate::endpoint_health::CooldownBackoff,
    _concurrency_permit: Option<ConcurrencyPermit>,
    attempt_handle: AttemptHandle,
    route_affinity_success: Option<SessionRouteAffinitySuccess>,
}

struct StreamFinalizeWork {
    state: Arc<ProxyState>,
    request_id: u64,
    attempt_handle: AttemptHandle,
    attempt_outcome: AttemptOutcome,
    attempt_succeeded: bool,
    observer: RequestObserver,
    publication: RequestPublication,
    health_update: Option<StreamHealthUpdate>,
    service_name: String,
    target_for_health: CapturedRouteCandidate,
    cooldown_backoff: crate::endpoint_health::CooldownBackoff,
    route_affinity_success: Option<SessionRouteAffinitySuccess>,
    _concurrency_permit: Option<ConcurrencyPermit>,
}

impl StreamFinalize {
    fn mark_protocol_terminal(&self, terminal: StreamProtocolTerminal) {
        let Ok(mut guard) = self.usage_state.lock() else {
            return;
        };
        if guard.protocol_terminal != StreamProtocolTerminal::Pending {
            return;
        }
        guard.protocol_terminal = terminal;
        if terminal.is_upstream_failure() && guard.stream_error.is_none() {
            guard.stream_error = Some(StreamErrorInfo {
                class: "upstream_response_error",
                health: StreamErrorHealth::Penalize,
            });
        }
    }

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
            upstream_origin: b.upstream_origin.clone(),
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

    fn take_commit_work(&mut self) -> Option<StreamFinalizeWork> {
        let state = self.state.clone();
        let request_id = self.request_id;
        let status_code = self.status_code;
        let started_at_ms = self.started_at_ms;

        let mut guard = match self.usage_state.lock() {
            Ok(g) => g,
            Err(_) => return None,
        };
        if !guard.terminal_publication.mark_published() {
            return None;
        }
        let usage = guard.usage.clone();
        let ttfb_ms = guard.first_chunk_ms;
        let service_tier = ServiceTierLog {
            actual: guard.service_tier.clone(),
            ..self.service_tier.clone()
        };
        let reported_model = guard.reported_model.clone();
        let stream_error = guard.stream_error.clone();
        let protocol_terminal = guard.protocol_terminal;
        let response_body = guard.buffer.iter().copied().collect::<Vec<_>>();

        let dur = self.start.elapsed().as_millis() as u64;

        let http_debug_warn = self.build_http_debug(&response_body, guard.first_chunk_ms, true);
        if should_include_http_warn(self.status_code)
            && !guard.warned_non_success
            && let Some(h) = http_debug_warn.as_ref()
        {
            warn_http_debug(self.status_code, h);
            guard.warned_non_success = true;
        }
        let http_debug = if should_include_http_debug(self.status_code) {
            self.build_http_debug(&response_body, guard.first_chunk_ms, false)
        } else {
            None
        };

        guard.buffer.clear();
        drop(guard);
        let transport_cooldown_secs = self.transport_cooldown_secs;
        let terminal_decision = decide_stream_terminal_response(StreamTerminalDecisionParams {
            status_code,
            stream_error: stream_error.as_ref(),
            protocol_terminal,
            resp_headers: &self.resp_headers,
            response_body: response_body.as_slice(),
            transport_cooldown_secs,
            cloudflare_challenge_cooldown_secs: self.cloudflare_challenge_cooldown_secs,
            cloudflare_timeout_cooldown_secs: self.cloudflare_timeout_cooldown_secs,
        });
        let health_update = terminal_decision.health_update;
        let classified_response =
            classify_observed_upstream_response(status_code, &self.resp_headers, &response_body);
        let evidence_route_facing = matches!(
            &health_update,
            Some(StreamHealthUpdate::FailureAndPenalty { .. })
        );
        let attempt_succeeded = (200..300).contains(&status_code)
            && stream_error.is_none()
            && protocol_terminal.is_success();
        let provider_evidence = response_evidence_from_classification(ResponseEvidenceParams {
            target: &self.target,
            classified_response: &classified_response,
            status_code,
            error_class: stream_error
                .as_ref()
                .map(|error| error.class)
                .or_else(|| {
                    protocol_terminal
                        .is_upstream_failure()
                        .then_some("upstream_response_error")
                })
                .or(classified_response.class.as_deref()),
            route_facing: evidence_route_facing,
        });

        let service_name = self.service_name.clone();
        let target_for_health = self.target.clone();
        let cooldown_backoff = self.cooldown_backoff;
        let observer = RequestObserver::from_parts(
            state.clone(),
            self.service_name.clone(),
            self.method.clone(),
            self.path.clone(),
        );
        let terminal_status_code = if attempt_succeeded || !(200..300).contains(&status_code) {
            status_code
        } else {
            StatusCode::BAD_GATEWAY.as_u16()
        };
        let mut publication = RequestPublication::new_terminal(
            request_id,
            terminal_status_code,
            dur,
            started_at_ms,
            true,
        );
        publication.winning_attempt = attempt_succeeded.then_some(self.attempt_handle);
        publication.ttfb_ms = ttfb_ms;
        publication.provider_id = self.provider_id.clone();
        publication.endpoint_id = self.endpoint_id.clone();
        publication.provider_endpoint_key = self.provider_endpoint_key.clone();
        publication.upstream_origin = self.upstream_origin.clone();
        publication.session_id = self.session_id.clone();
        publication.session_identity_source = self.session_identity_source;
        publication.cwd = self.cwd.clone();
        publication.reasoning_effort = self.reasoning_effort.clone();
        publication.service_tier = service_tier;
        publication.reported_model = reported_model;
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
        }
        publication.http_debug = http_debug;
        let publication = publication.with_route_decision_model();
        let attempt_handle = self.attempt_handle;
        let attempt_outcome = if attempt_succeeded {
            AttemptOutcome::Succeeded
        } else {
            AttemptOutcome::Failed
        };
        let route_affinity_success = self.route_affinity_success.take();

        Some(StreamFinalizeWork {
            state,
            request_id,
            attempt_handle,
            attempt_outcome,
            attempt_succeeded,
            observer,
            publication,
            health_update,
            service_name,
            target_for_health,
            cooldown_backoff,
            route_affinity_success,
            _concurrency_permit: self._concurrency_permit.take(),
        })
    }

    async fn commit_terminal(&mut self) -> bool {
        let Some(work) = self.take_commit_work() else {
            return false;
        };
        work.commit().await
    }
}

impl StreamFinalizeWork {
    async fn commit(self) -> bool {
        let Self {
            state,
            request_id,
            attempt_handle,
            attempt_outcome,
            attempt_succeeded,
            observer,
            mut publication,
            health_update,
            service_name,
            target_for_health,
            cooldown_backoff,
            route_affinity_success,
            _concurrency_permit,
        } = self;

        if let Err(error) = state.finish_upstream_attempt(
            attempt_handle,
            attempt_outcome,
            crate::logging::now_ms(),
            EconomicsState::Unknown,
        ) {
            tracing::error!(
                request_id,
                error = %error,
                "failed to commit durable streaming attempt terminal"
            );
            return false;
        }
        if attempt_succeeded {
            publication.route_affinity_success = route_affinity_success;
        }
        if !observer.publish_terminal_once(publication).await {
            tracing::error!(
                request_id,
                "failed to commit durable streaming logical terminal"
            );
            return false;
        }
        match health_update {
            Some(StreamHealthUpdate::Success) => {
                record_attempt_success(state.as_ref(), service_name.as_str(), &target_for_health)
                    .await;
            }
            Some(StreamHealthUpdate::Failure) => {
                record_attempt_failure(
                    state.as_ref(),
                    service_name.as_str(),
                    &target_for_health,
                    crate::endpoint_health::COOLDOWN_SECS,
                    cooldown_backoff,
                )
                .await;
            }
            Some(StreamHealthUpdate::FailureAndPenalty { cooldown_secs }) => {
                record_attempt_failure(
                    state.as_ref(),
                    service_name.as_str(),
                    &target_for_health,
                    crate::endpoint_health::COOLDOWN_SECS,
                    cooldown_backoff,
                )
                .await;
                penalize_attempt_target(
                    state.as_ref(),
                    service_name.as_str(),
                    &target_for_health,
                    cooldown_secs,
                    cooldown_backoff,
                )
                .await;
            }
            None => {}
        }
        true
    }
}

impl Drop for StreamFinalize {
    fn drop(&mut self) {
        if let Some(work) = self.take_commit_work() {
            tokio::spawn(async move {
                let _ = work.commit().await;
            });
        }
    }
}

enum SseTerminalGateProgress {
    NeedMore,
    Forward(Bytes),
    Terminal {
        before: Bytes,
        withheld: Bytes,
        terminal: StreamProtocolTerminal,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SseEventTooLarge;

#[derive(Default)]
struct SseTerminalGate {
    pending: Vec<u8>,
    search_pos: usize,
    inspected_bytes: usize,
}

impl SseTerminalGate {
    fn push(
        &mut self,
        chunk: &[u8],
        max_event_bytes: usize,
        mut observe: impl FnMut(&serde_json::Value),
    ) -> Result<SseTerminalGateProgress, SseEventTooLarge> {
        self.pending.extend_from_slice(chunk);
        let mut consumed = 0;

        loop {
            let Some(event_end) = find_sse_event_end(
                &self.pending,
                self.search_pos.max(consumed),
                &mut self.inspected_bytes,
            ) else {
                if self.pending.len().saturating_sub(consumed) > max_event_bytes {
                    return Err(SseEventTooLarge);
                }
                if consumed == 0 {
                    self.search_pos = self.pending.len().saturating_sub(2);
                    return Ok(SseTerminalGateProgress::NeedMore);
                }

                let remainder = self.pending.split_off(consumed);
                let forward = std::mem::replace(&mut self.pending, remainder);
                self.search_pos = self.pending.len().saturating_sub(2);
                return Ok(SseTerminalGateProgress::Forward(Bytes::from(forward)));
            };

            if event_end.saturating_sub(consumed) > max_event_bytes {
                return Err(SseEventTooLarge);
            }
            let terminal = inspect_sse_event(&self.pending[consumed..event_end], &mut observe);
            if let Some(terminal) = terminal {
                if self.pending.len().saturating_sub(consumed) > max_event_bytes {
                    return Err(SseEventTooLarge);
                }
                let withheld = self.pending.split_off(consumed);
                let before = std::mem::take(&mut self.pending);
                self.search_pos = 0;
                return Ok(SseTerminalGateProgress::Terminal {
                    before: Bytes::from(before),
                    withheld: Bytes::from(withheld),
                    terminal,
                });
            }

            consumed = event_end;
            self.search_pos = consumed;
        }
    }

    fn finish(
        &mut self,
        max_event_bytes: usize,
        mut observe: impl FnMut(&serde_json::Value),
    ) -> Result<Option<(Bytes, StreamProtocolTerminal)>, SseEventTooLarge> {
        if self.pending.is_empty() {
            return Ok(None);
        }
        if self.pending.len() > max_event_bytes {
            return Err(SseEventTooLarge);
        }
        let terminal = inspect_sse_event(&self.pending, &mut observe);
        Ok(terminal.map(|terminal| {
            self.search_pos = 0;
            (Bytes::from(std::mem::take(&mut self.pending)), terminal)
        }))
    }

    #[cfg(test)]
    fn inspected_bytes(&self) -> usize {
        self.inspected_bytes
    }
}

fn inspect_sse_event(
    event: &[u8],
    observe: &mut impl FnMut(&serde_json::Value),
) -> Option<StreamProtocolTerminal> {
    let decoded = decode_sse_event(event);
    let event_terminal = decoded
        .event_type
        .as_deref()
        .and_then(|event_type| terminal_from_event_type(event_type.as_bytes()));
    let data_terminal = match decoded.data {
        DecodedSseData::Missing | DecodedSseData::Invalid => None,
        DecodedSseData::Done => Some(StreamProtocolTerminal::Succeeded),
        DecodedSseData::Json(value) => {
            let terminal = value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .and_then(|event_type| terminal_from_event_type(event_type.as_bytes()));
            observe(&value);
            terminal
        }
    };

    match (event_terminal, data_terminal) {
        (None, terminal) => terminal,
        (Some(event), Some(data)) if event == data => Some(event),
        (Some(_), Some(_)) => Some(StreamProtocolTerminal::UpstreamFailure),
        (Some(StreamProtocolTerminal::Succeeded), None) => {
            Some(StreamProtocolTerminal::UpstreamFailure)
        }
        (Some(terminal), None) => Some(terminal),
    }
}

fn terminal_from_event_type(event_type: &[u8]) -> Option<StreamProtocolTerminal> {
    match event_type {
        b"response.completed" | b"message_stop" => Some(StreamProtocolTerminal::Succeeded),
        b"response.incomplete" | b"response.cancelled" | b"response.canceled" => {
            Some(StreamProtocolTerminal::LogicalFailure)
        }
        b"response.failed" | b"error" => Some(StreamProtocolTerminal::UpstreamFailure),
        _ => None,
    }
}

#[cfg(test)]
fn drain_sse_before_success_terminal(pending: &mut Vec<u8>) -> SseTerminalGateProgress {
    let mut gate = SseTerminalGate::default();
    let progress = gate
        .push(&std::mem::take(pending), usize::MAX, |_| {})
        .expect("unbounded test gate");
    *pending = gate.pending;
    progress
}

struct StreamForwardState {
    upstream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    finalize: StreamFinalize,
    idle_timeout: Option<Duration>,
    gate_success_terminal: bool,
    is_sse: bool,
    terminal_committed: bool,
    finished: bool,
    sse_terminal_gate: SseTerminalGate,
    non_sse_pending: Vec<u8>,
    withheld_terminal: Option<(Bytes, StreamProtocolTerminal)>,
    max_keep: usize,
    max_sse_event_bytes: usize,
}

fn merge_stream_sse_value(usage_state: &Arc<Mutex<StreamUsageState>>, value: &serde_json::Value) {
    let Ok(mut guard) = usage_state.lock() else {
        return;
    };
    crate::usage::merge_usage_from_json_value(value, &mut guard.usage);
    let StreamUsageState {
        reported_model,
        service_tier,
        ..
    } = &mut *guard;
    merge_response_metadata_from_value(value, reported_model, service_tier);
}

impl StreamForwardState {
    async fn next_body_item(mut self) -> Option<(Result<Bytes, io::Error>, Self)> {
        loop {
            if self.finished {
                return None;
            }

            if let Some((withheld, terminal)) = self.withheld_terminal.take() {
                self.finalize.mark_protocol_terminal(terminal);
                if self.finalize.commit_terminal().await {
                    self.terminal_committed = true;
                    return Some((Ok(withheld), self));
                }
                let item = self.handle_terminal_commit_failure();
                return Some((item, self));
            }

            let upstream_item = match self.idle_timeout {
                Some(timeout) => match tokio::time::timeout(timeout, self.upstream.next()).await {
                    Ok(item) => item,
                    Err(_) => {
                        if self.terminal_committed {
                            self.finished = true;
                            return None;
                        }
                        let item = self.handle_idle_timeout(timeout);
                        if self.finalize.commit_terminal().await {
                            return Some((item, self));
                        }
                        let item = self.handle_terminal_commit_failure();
                        return Some((item, self));
                    }
                },
                None => self.upstream.next().await,
            };

            match upstream_item {
                Some(Ok(chunk)) => {
                    let chunk = self.handle_chunk(chunk);
                    if self.terminal_committed || !self.gate_success_terminal {
                        return Some((Ok(chunk), self));
                    }

                    if !self.is_sse {
                        if self.non_sse_pending.len().saturating_add(chunk.len()) > self.max_keep {
                            let message = format!(
                                "Upstream non-SSE stream exceeded the {} byte safety limit",
                                self.max_keep
                            );
                            let item = self.handle_stream_failure(
                                "local_stream_limit",
                                "non_sse_stream_too_large",
                                message,
                                StreamErrorHealth::Neutral,
                                io::ErrorKind::InvalidData,
                            );
                            if self.finalize.commit_terminal().await {
                                return Some((item, self));
                            }
                            let item = self.handle_terminal_commit_failure();
                            return Some((item, self));
                        }
                        self.non_sse_pending.extend_from_slice(&chunk);
                        continue;
                    }

                    let usage_state = self.finalize.usage_state.clone();
                    let progress =
                        self.sse_terminal_gate
                            .push(&chunk, self.max_sse_event_bytes, |value| {
                                merge_stream_sse_value(&usage_state, value)
                            });
                    match progress {
                        Err(SseEventTooLarge) => {
                            let message = format!(
                                "Upstream SSE event exceeded the {} byte safety limit",
                                self.max_sse_event_bytes
                            );
                            let item = self.handle_stream_failure(
                                "local_stream_limit",
                                "sse_event_too_large",
                                message,
                                StreamErrorHealth::Neutral,
                                io::ErrorKind::InvalidData,
                            );
                            if self.finalize.commit_terminal().await {
                                return Some((item, self));
                            }
                            let item = self.handle_terminal_commit_failure();
                            return Some((item, self));
                        }
                        Ok(SseTerminalGateProgress::NeedMore) => {}
                        Ok(SseTerminalGateProgress::Forward(bytes)) => {
                            return Some((Ok(bytes), self));
                        }
                        Ok(SseTerminalGateProgress::Terminal {
                            before,
                            withheld,
                            terminal,
                        }) => {
                            self.withheld_terminal = Some((withheld, terminal));
                            if !before.is_empty() {
                                return Some((Ok(before), self));
                            }
                        }
                    }
                }
                Some(Err(error)) => {
                    if self.terminal_committed {
                        self.finished = true;
                        return None;
                    }
                    let item = self.handle_read_error(error);
                    if self.finalize.commit_terminal().await {
                        return Some((item, self));
                    }
                    let item = self.handle_terminal_commit_failure();
                    return Some((item, self));
                }
                None => {
                    if self.terminal_committed {
                        self.finished = true;
                        return None;
                    }
                    if !self.gate_success_terminal {
                        if self.finalize.commit_terminal().await {
                            self.finished = true;
                            return None;
                        }
                        let item = self.handle_terminal_commit_failure();
                        return Some((item, self));
                    }

                    if !self.is_sse {
                        self.withheld_terminal = Some((
                            Bytes::from(std::mem::take(&mut self.non_sse_pending)),
                            StreamProtocolTerminal::Succeeded,
                        ));
                        continue;
                    }

                    let usage_state = self.finalize.usage_state.clone();
                    match self
                        .sse_terminal_gate
                        .finish(self.max_sse_event_bytes, |value| {
                            merge_stream_sse_value(&usage_state, value)
                        }) {
                        Ok(Some((withheld, terminal))) => {
                            self.withheld_terminal = Some((withheld, terminal));
                            continue;
                        }
                        Err(SseEventTooLarge) => {
                            let message = format!(
                                "Upstream SSE event exceeded the {} byte safety limit",
                                self.max_sse_event_bytes
                            );
                            let item = self.handle_stream_failure(
                                "local_stream_limit",
                                "sse_event_too_large",
                                message,
                                StreamErrorHealth::Neutral,
                                io::ErrorKind::InvalidData,
                            );
                            if self.finalize.commit_terminal().await {
                                return Some((item, self));
                            }
                            let item = self.handle_terminal_commit_failure();
                            return Some((item, self));
                        }
                        Ok(None) => {}
                    }

                    let item = self.handle_stream_failure(
                        "upstream_stream_error",
                        "missing_terminal",
                        "Upstream SSE stream ended before a terminal event".to_string(),
                        StreamErrorHealth::Penalize,
                        io::ErrorKind::UnexpectedEof,
                    );
                    if self.finalize.commit_terminal().await {
                        return Some((item, self));
                    }
                    let item = self.handle_terminal_commit_failure();
                    return Some((item, self));
                }
            }
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

        append_stream_buffer_tail(&mut guard.buffer, &chunk, self.max_keep);
        if !guard.warned_non_success && !(200..300).contains(&finalize.status_code) {
            let response_body = guard.buffer.iter().copied().collect::<Vec<_>>();
            if should_include_http_warn(finalize.status_code)
                && let Some(h) =
                    finalize.build_http_debug(&response_body, guard.first_chunk_ms, true)
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

        chunk
    }

    fn handle_read_error(&mut self, error: reqwest::Error) -> Result<Bytes, io::Error> {
        let kind = classify_stream_read_error_kind(&error);
        let message = format!(
            "Upstream stream failed: {}",
            format_reqwest_error_for_retry_chain(&error)
        );
        self.handle_stream_failure(
            "upstream_stream_error",
            kind,
            message,
            StreamErrorHealth::Penalize,
            io::ErrorKind::Other,
        )
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
            StreamErrorHealth::Penalize,
            io::ErrorKind::TimedOut,
        )
    }

    fn handle_stream_failure(
        &mut self,
        class: &'static str,
        kind: &'static str,
        message: String,
        health: StreamErrorHealth,
        io_kind: io::ErrorKind,
    ) -> Result<Bytes, io::Error> {
        let (first_chunk_ms, buffered_bytes) =
            if let Ok(mut guard) = self.finalize.usage_state.lock() {
                let first_chunk_ms = guard.first_chunk_ms;
                let buffered_bytes = guard.buffer.len();
                guard.stream_error = Some(StreamErrorInfo { class, health });
                if guard.protocol_terminal == StreamProtocolTerminal::Pending {
                    guard.protocol_terminal = match health {
                        StreamErrorHealth::Neutral => StreamProtocolTerminal::LogicalFailure,
                        StreamErrorHealth::Penalize => StreamProtocolTerminal::UpstreamFailure,
                    };
                }
                (first_chunk_ms, buffered_bytes)
            } else {
                (None, 0)
            };

        let trace_id = request_trace_id(
            self.finalize.service_name.as_str(),
            self.finalize.request_id,
        );
        log_control_trace_event(serde_json::json!({
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
            "upstream_origin": self.finalize.upstream_origin,
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
            upstream_origin = self.finalize.upstream_origin.as_deref().unwrap_or("-"),
            error_class = class,
            stream_error_kind = kind,
            first_chunk_ms = ?first_chunk_ms,
            buffered_bytes,
            error = %message,
            "upstream stream error"
        );

        self.finished = true;
        if self.is_sse
            && self.finalize.service_name == "codex"
            && is_codex_responses_sse_path(&self.finalize.path)
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

    fn handle_terminal_commit_failure(&mut self) -> Result<Bytes, io::Error> {
        const CLASS: &str = "terminal_commit_failed";
        let message = "Failed to commit the durable request terminal";
        self.finished = true;
        tracing::error!(
            request_id = self.finalize.request_id,
            "withholding streaming success because terminal commit failed"
        );
        if self.is_sse
            && self.finalize.service_name == "codex"
            && is_codex_responses_sse_path(&self.finalize.path)
        {
            let model = self
                .finalize
                .route_decision
                .as_ref()
                .and_then(|decision| decision.effective_model.as_ref())
                .map(|model| model.value.as_str());
            return Ok(Bytes::from(
                CodexFailureSse::stream_error(message, model, CLASS).to_event_string(),
            ));
        }
        Err(io::Error::other(message))
    }
}

pub(super) async fn build_sse_success_response(
    proxy: &ProxyService,
    target: CapturedRouteCandidate,
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
        attempt_handle,
        route_affinity_success,
    } = meta;

    let provider_endpoint_key = target.provider_endpoint_key();
    let upstream_origin = upstream_origin(target.base_url());
    if is_user_turn {
        info!(
            method = %method,
            path = %path,
            provider_endpoint_key = %provider_endpoint_key,
            provider_id = target.provider_id(),
            upstream_origin = upstream_origin.as_deref().unwrap_or("-"),
            "user turn routed to upstream endpoint"
        );
    }

    let max_keep = stream_buffer_max_bytes();
    let usage_state = Arc::new(Mutex::new(StreamUsageState::default()));
    let method_s = method.to_string();
    let path_s = path.clone();
    let provider_id = Some(target.provider_id().to_owned());
    let endpoint_id = Some(target.endpoint_id().to_owned());
    let service_name = proxy.service_name.to_string();
    let status_code = status.as_u16();
    let stream_idle_timeout = codex_responses_stream_idle_timeout(is_codex_service, &path_s);
    let is_sse = resp_headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value.split(';').next().is_some_and(|media_type| {
                media_type.trim().eq_ignore_ascii_case("text/event-stream")
            })
        })
        .unwrap_or(true);
    let gate_success_terminal = status.is_success();

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
        provider_id: provider_id.clone(),
        endpoint_id: endpoint_id.clone(),
        provider_endpoint_key: Some(provider_endpoint_key.clone()),
        upstream_origin,
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
        target: target.clone(),
        transport_cooldown_secs,
        cloudflare_challenge_cooldown_secs,
        cloudflare_timeout_cooldown_secs,
        cooldown_backoff,
        _concurrency_permit: concurrency_permit,
        attempt_handle,
        route_affinity_success,
    };

    if is_user_turn && is_codex_service {
        let cfg_snapshot = proxy.config.snapshot().await;
        let state = proxy.state.clone();
        let client = proxy.client.clone();
        let service_name = proxy.service_name.to_string();
        super::providers_api::enqueue_provider_balance_probe(
            client,
            cfg_snapshot,
            state,
            service_name.as_str(),
            target.provider_endpoint().clone(),
        );
    }

    let stream_state = StreamForwardState {
        upstream: Box::pin(resp.bytes_stream()),
        finalize,
        idle_timeout: stream_idle_timeout,
        gate_success_terminal,
        is_sse,
        terminal_committed: false,
        finished: false,
        sse_terminal_gate: SseTerminalGate::default(),
        non_sse_pending: Vec::new(),
        withheld_terminal: None,
        max_keep,
        max_sse_event_bytes: sse_event_max_bytes(),
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
    pub(super) cooldown_backoff: crate::endpoint_health::CooldownBackoff,
    pub(super) method: Method,
    pub(super) path: String,
    pub(super) concurrency_permit: Option<ConcurrencyPermit>,
    pub(super) attempt_handle: AttemptHandle,
    pub(super) route_affinity_success: Option<SessionRouteAffinitySuccess>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_terminal_gate_forwards_nonterminal_events_and_withholds_split_success() {
        let mut pending = b"event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n\
event: response.compl"
            .to_vec();

        let SseTerminalGateProgress::Forward(forwarded) =
            drain_sse_before_success_terminal(&mut pending)
        else {
            panic!("nonterminal event should be forwarded");
        };
        assert!(String::from_utf8_lossy(&forwarded).contains("response.output_text.delta"));
        assert_eq!(pending, b"event: response.compl");

        pending.extend_from_slice(
            b"eted\ndata: {\"type\":\"response.completed\"}\n\ndata: [DONE]\n\n",
        );
        let SseTerminalGateProgress::Terminal {
            before,
            withheld,
            terminal,
        } = drain_sse_before_success_terminal(&mut pending)
        else {
            panic!("completed event should be withheld");
        };
        assert!(before.is_empty());
        let withheld = String::from_utf8_lossy(&withheld);
        assert!(withheld.contains("response.completed"));
        assert!(withheld.contains("[DONE]"));
        assert_eq!(terminal, StreamProtocolTerminal::Succeeded);
        assert!(pending.is_empty());
    }

    #[test]
    fn sse_terminal_gate_recognizes_done_with_crlf_boundaries() {
        let mut pending = b"data: [DONE]\r\n\r\n".to_vec();
        let SseTerminalGateProgress::Terminal {
            before,
            withheld,
            terminal,
        } = drain_sse_before_success_terminal(&mut pending)
        else {
            panic!("done event should be withheld");
        };
        assert!(before.is_empty());
        assert_eq!(withheld.as_ref(), b"data: [DONE]\r\n\r\n");
        assert_eq!(terminal, StreamProtocolTerminal::Succeeded);
    }

    #[test]
    fn sse_terminal_gate_withholds_failed_terminal() {
        let mut pending = b"event: response.failed\n\
data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"upstream rejected\"}}}\n\n"
            .to_vec();

        let SseTerminalGateProgress::Terminal {
            before,
            withheld,
            terminal,
        } = drain_sse_before_success_terminal(&mut pending)
        else {
            panic!("failed terminal should be withheld");
        };

        assert!(before.is_empty());
        assert!(String::from_utf8_lossy(&withheld).contains("response.failed"));
        assert_eq!(terminal, StreamProtocolTerminal::UpstreamFailure);
    }

    #[test]
    fn sse_terminal_gate_withholds_anthropic_message_stop() {
        let mut gate = SseTerminalGate::default();
        let usage_state = Arc::new(Mutex::new(StreamUsageState::default()));
        let message_start = b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-sonnet-4-5\",\"service_tier\":\"standard_only\",\"usage\":{\"input_tokens\":12}}}\n\n";
        let event = b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

        let observed_state = usage_state.clone();
        assert!(matches!(
            gate.push(message_start, message_start.len(), |value| {
                merge_stream_sse_value(&observed_state, value)
            }),
            Ok(SseTerminalGateProgress::Forward(_))
        ));
        let observed_state = usage_state.clone();
        let progress = gate
            .push(event, event.len(), |value| {
                merge_stream_sse_value(&observed_state, value)
            })
            .expect("Anthropic terminal event");
        let SseTerminalGateProgress::Terminal {
            before,
            withheld,
            terminal,
        } = progress
        else {
            panic!("message_stop should be withheld as a successful terminal");
        };
        assert!(before.is_empty());
        assert_eq!(withheld.as_ref(), event);
        assert_eq!(terminal, StreamProtocolTerminal::Succeeded);
        let state = usage_state.lock().expect("usage state");
        assert_eq!(
            state.usage.as_ref().map(|usage| usage.input_tokens),
            Some(12)
        );
        assert_eq!(state.reported_model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(state.service_tier.as_deref(), Some("standard_only"));
    }

    #[test]
    fn sse_terminal_gate_scans_fragmented_events_linearly() {
        let mut gate = SseTerminalGate::default();
        let mut event = b"data: ".to_vec();
        event.extend(std::iter::repeat_n(b'x', 4 * 1024));
        event.extend_from_slice(b"\n\n");
        let mut forwarded = Bytes::new();

        for byte in &event {
            match gate.push(&[*byte], 8 * 1024, |_| {}) {
                Ok(SseTerminalGateProgress::NeedMore) => {}
                Ok(SseTerminalGateProgress::Forward(bytes)) => forwarded = bytes,
                Ok(SseTerminalGateProgress::Terminal { .. }) => {
                    panic!("nonterminal event must not be withheld")
                }
                Err(error) => panic!("fragmented event should stay within the limit: {error:?}"),
            }
        }

        assert_eq!(forwarded.as_ref(), event.as_slice());
        assert!(gate.inspected_bytes() <= event.len() * 4);
    }

    #[test]
    fn sse_terminal_gate_rejects_a_closed_oversized_event() {
        let mut gate = SseTerminalGate::default();
        let event = b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"oversized\"}\n\n";

        assert!(gate.push(event, 32, |_| {}).is_err());
    }

    #[test]
    fn sse_event_larger_than_diagnostic_tail_is_allowed_under_protocol_cap() {
        let diagnostic_tail_bytes = 1024 * 1024;
        let mut event = b"data: ".to_vec();
        event.extend(std::iter::repeat_n(b'x', diagnostic_tail_bytes + 1));
        event.extend_from_slice(b"\n\n");
        let protocol_cap = event.len();
        let mut gate = SseTerminalGate::default();

        let progress = gate
            .push(&event, protocol_cap, |_| {})
            .expect("event below the independent protocol cap");

        let SseTerminalGateProgress::Forward(forwarded) = progress else {
            panic!("large nonterminal event should be forwarded");
        };
        assert_eq!(forwarded.as_ref(), event.as_slice());
    }

    #[test]
    fn sse_terminal_gate_parses_multiline_terminal_data_once() {
        let mut gate = SseTerminalGate::default();
        let event = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\n",
            "data: \"response\":{\"model\":\"gpt-5.6-sol\",\"service_tier\":\"priority\",\n",
            "data: \"usage\":{\"input_tokens\":1000,\"input_tokens_details\":{\"cached_tokens\":100,\"cache_write_tokens\":200}}}}\n\n",
        );
        let usage_state = Arc::new(Mutex::new(StreamUsageState::default()));
        let observed_state = usage_state.clone();
        let mut observed = 0;

        let progress = gate
            .push(event.as_bytes(), event.len(), |value| {
                observed += 1;
                merge_stream_sse_value(&observed_state, value);
            })
            .expect("multiline event");

        let SseTerminalGateProgress::Terminal { terminal, .. } = progress else {
            panic!("completed event should be withheld");
        };
        assert_eq!(terminal, StreamProtocolTerminal::Succeeded);
        assert_eq!(observed, 1);
        let state = usage_state.lock().expect("usage state");
        let usage = state.usage.as_ref().expect("usage");
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.cached_input_tokens, 100);
        assert_eq!(usage.cache_creation_input_tokens, 200);
        let buckets = usage
            .canonical_usage_buckets(crate::usage::CacheAccountingConvention::INCLUDED_IN_INPUT);
        assert_eq!(buckets.ordinary_input_tokens, 700);
        assert_eq!(buckets.cache_read_input_tokens, 100);
        assert_eq!(buckets.cache_write_input_tokens, 200);
        assert_eq!(state.reported_model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(state.service_tier.as_deref(), Some("priority"));
    }

    #[test]
    fn sse_terminal_gate_recognizes_line_boundaries_at_every_split() {
        for event in [
            b"data: [DONE]\n\n".as_slice(),
            b"data: [DONE]\r\n\r\n".as_slice(),
            b"data: [DONE]\r\r".as_slice(),
            b"data: [DONE]\r\n\r".as_slice(),
        ] {
            for split in 1..event.len() {
                let mut gate = SseTerminalGate::default();
                assert!(matches!(
                    gate.push(&event[..split], event.len(), |_| {}),
                    Ok(SseTerminalGateProgress::NeedMore)
                ));
                let progress = gate
                    .push(&event[split..], event.len(), |_| {})
                    .expect("split SSE event");
                let (withheld, terminal) = match progress {
                    SseTerminalGateProgress::Terminal {
                        withheld, terminal, ..
                    } => (withheld, terminal),
                    SseTerminalGateProgress::NeedMore => gate
                        .finish(event.len(), |_| {})
                        .expect("EOF event")
                        .expect("EOF terminal"),
                    SseTerminalGateProgress::Forward(_) => {
                        panic!("split {split} forwarded terminal event {event:?}")
                    }
                };
                assert_eq!(withheld.as_ref(), event, "split {split} for {event:?}");
                assert_eq!(
                    terminal,
                    StreamProtocolTerminal::Succeeded,
                    "split {split} for {event:?}"
                );
            }
        }
    }

    #[test]
    fn logical_failure_terminals_do_not_penalize_endpoint_health() {
        let headers = HeaderMap::new();

        for event_type in [
            "response.incomplete",
            "response.cancelled",
            "response.canceled",
        ] {
            let event = format!("event: {event_type}\ndata: {{\"type\":\"{event_type}\"}}\n\n");
            let terminal =
                inspect_sse_event(event.as_bytes(), &mut |_| {}).expect("logical failure terminal");
            assert_eq!(terminal, StreamProtocolTerminal::LogicalFailure);

            let decision = decide_stream_terminal_response(StreamTerminalDecisionParams {
                status_code: 200,
                stream_error: None,
                protocol_terminal: terminal,
                resp_headers: &headers,
                response_body: event.as_bytes(),
                transport_cooldown_secs: 30,
                cloudflare_challenge_cooldown_secs: 0,
                cloudflare_timeout_cooldown_secs: 0,
            });
            assert!(decision.health_update.is_none(), "{event_type}");
        }
    }

    #[test]
    fn upstream_failure_terminals_penalize_endpoint_health() {
        let headers = HeaderMap::new();

        for event_type in ["response.failed", "error"] {
            let event = format!("event: {event_type}\ndata: {{\"type\":\"{event_type}\"}}\n\n");
            let terminal = inspect_sse_event(event.as_bytes(), &mut |_| {})
                .expect("upstream failure terminal");
            assert_eq!(terminal, StreamProtocolTerminal::UpstreamFailure);

            let decision = decide_stream_terminal_response(StreamTerminalDecisionParams {
                status_code: 200,
                stream_error: None,
                protocol_terminal: terminal,
                resp_headers: &headers,
                response_body: event.as_bytes(),
                transport_cooldown_secs: 30,
                cloudflare_challenge_cooldown_secs: 0,
                cloudflare_timeout_cooldown_secs: 0,
            });
            assert!(matches!(
                decision.health_update,
                Some(StreamHealthUpdate::FailureAndPenalty { cooldown_secs: 30 })
            ));
        }
    }

    #[test]
    fn completed_header_conflicts_fail_closed() {
        for event in [
            concat!(
                "event: response.completed\n",
                "data: {\"type\":\"response.failed\"}\n\n"
            ),
            concat!(
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\"\n\n"
            ),
        ] {
            assert_eq!(
                inspect_sse_event(event.as_bytes(), &mut |_| {}),
                Some(StreamProtocolTerminal::UpstreamFailure)
            );
        }
    }

    #[test]
    fn local_stream_limit_does_not_penalize_endpoint_health() {
        let headers = HeaderMap::new();
        let stream_error = StreamErrorInfo {
            class: "local_stream_limit",
            health: StreamErrorHealth::Neutral,
        };

        let decision = decide_stream_terminal_response(StreamTerminalDecisionParams {
            status_code: 200,
            stream_error: Some(&stream_error),
            protocol_terminal: StreamProtocolTerminal::LogicalFailure,
            resp_headers: &headers,
            response_body: b"",
            transport_cooldown_secs: 30,
            cloudflare_challenge_cooldown_secs: 0,
            cloudflare_timeout_cooldown_secs: 0,
        });

        assert!(decision.health_update.is_none());
    }

    #[test]
    fn stream_terminal_failure_penalizes_endpoint() {
        let headers = HeaderMap::new();
        let stream_error = StreamErrorInfo {
            class: "upstream_stream_error",
            health: StreamErrorHealth::Penalize,
        };

        let decision = decide_stream_terminal_response(StreamTerminalDecisionParams {
            status_code: 200,
            stream_error: Some(&stream_error),
            protocol_terminal: StreamProtocolTerminal::UpstreamFailure,
            resp_headers: &headers,
            response_body: b"",
            transport_cooldown_secs: 30,
            cloudflare_challenge_cooldown_secs: 0,
            cloudflare_timeout_cooldown_secs: 0,
        });

        assert!(matches!(
            decision.health_update,
            Some(StreamHealthUpdate::FailureAndPenalty { cooldown_secs: 30 })
        ));
    }

    #[test]
    fn interrupted_stream_does_not_publish_upstream_health() {
        let headers = HeaderMap::new();

        let decision = decide_stream_terminal_response(StreamTerminalDecisionParams {
            status_code: 200,
            stream_error: None,
            protocol_terminal: StreamProtocolTerminal::Pending,
            resp_headers: &headers,
            response_body: b"",
            transport_cooldown_secs: 30,
            cloudflare_challenge_cooldown_secs: 0,
            cloudflare_timeout_cooldown_secs: 0,
        });

        assert!(decision.health_update.is_none());
    }
}
