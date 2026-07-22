use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::config::proxy_home_dir;
use crate::local_log_store::{LogRetention, append_line};
use crate::policy_actions::PolicyAction;
use crate::provider_signals::ProviderSignal;
use crate::state::{
    RouteDecisionProvenance, SessionIdentitySource, is_logical_request_success_status,
};
use crate::usage::UsageMetrics;

#[path = "logging/control_trace.rs"]
mod control_trace_impl;

use control_trace_impl::append_control_trace_payload;
pub use control_trace_impl::{
    ControlTraceDetail, ControlTraceLogEntry, control_trace_path, log_control_trace_event,
    read_recent_control_trace_entries,
};

#[derive(Debug, Clone, Copy)]
pub struct HttpDebugOptions {
    pub enabled: bool,
    pub all: bool,
    pub max_body_bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct HttpWarnOptions {
    pub enabled: bool,
    pub all: bool,
    pub max_body_bytes: usize,
}

pub fn should_log_request_body_preview() -> bool {
    // Default OFF: request bodies can be large and often contain sensitive data.
    // Enable explicitly when debugging request payload issues.
    env_bool_default("CODEX_HELPER_HTTP_LOG_REQUEST_BODY", false)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn request_trace_id(service: &str, request_id: u64) -> String {
    static PROCESS_BOOT_UUID: OnceLock<uuid::Uuid> = OnceLock::new();
    request_trace_id_for_boot(
        PROCESS_BOOT_UUID.get_or_init(uuid::Uuid::new_v4),
        service,
        request_id,
    )
}

const REQUEST_TRACE_ID_PREFIX: &str = "ch-trace:v1:";

pub(crate) fn request_trace_id_for_boot(
    boot_uuid: &uuid::Uuid,
    service: &str,
    request_id: u64,
) -> String {
    format!(
        "{REQUEST_TRACE_ID_PREFIX}{boot_uuid}:{}:{request_id}",
        request_trace_service(service)
    )
}

pub(crate) fn is_versioned_request_trace_id(trace_id: &str) -> bool {
    let Some(rest) = trace_id.strip_prefix(REQUEST_TRACE_ID_PREFIX) else {
        return false;
    };
    let Some((boot_uuid, request_identity)) = rest.split_once(':') else {
        return false;
    };
    let Some((service, request_id)) = request_identity.rsplit_once(':') else {
        return false;
    };

    uuid::Uuid::parse_str(boot_uuid).is_ok()
        && !service.is_empty()
        && request_id.parse::<u64>().is_ok()
}

pub(crate) fn legacy_request_trace_id(service: &str, request_id: u64) -> String {
    format!("{}-{request_id}", request_trace_service(service))
}

fn request_trace_service(service: &str) -> &str {
    let service = service.trim();
    if service.is_empty() {
        "request"
    } else {
        service
    }
}

pub(crate) fn upstream_origin(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let origin = url.origin().ascii_serialization();
    (origin != "null").then_some(origin)
}

pub(crate) fn upstream_uri_for_log(value: &str) -> Option<String> {
    let value = value.trim();
    if let Ok(url) = reqwest::Url::parse(value) {
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return None;
        }
        let path = url.path();
        return Some(if path.is_empty() { "/" } else { path }.to_string());
    }

    let uri = value.parse::<axum::http::Uri>().ok()?;
    if !value.starts_with('/') || uri.scheme().is_some() || uri.authority().is_some() {
        return None;
    }
    let path = uri.path();
    Some(if path.is_empty() { "/" } else { path }.to_string())
}

fn route_decision_for_request_log(
    mut route_decision: Option<RouteDecisionProvenance>,
) -> Option<RouteDecisionProvenance> {
    if let Some(route_decision) = route_decision.as_mut() {
        route_decision.effective_upstream_base_url = None;
    }
    route_decision
}

fn http_debug_for_request_log(mut http_debug: HttpDebugLog) -> HttpDebugLog {
    http_debug.client_uri = client_uri_for_log(http_debug.client_uri.as_str());
    http_debug.upstream_origin = http_debug
        .upstream_origin
        .and_then(|value| upstream_origin(value.as_str()));
    http_debug.upstream_uri = http_debug
        .upstream_uri
        .and_then(|value| upstream_uri_for_log(value.as_str()));
    http_debug
}

pub(crate) fn client_uri_for_log(value: &str) -> String {
    if let Ok(url) = reqwest::Url::parse(value)
        && matches!(url.scheme(), "http" | "https")
    {
        let path = url.path();
        return if path.is_empty() { "/" } else { path }.to_string();
    }
    value
        .parse::<axum::http::Uri>()
        .ok()
        .map(|uri| uri.path().to_string())
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| {
            value
                .split(['?', '#'])
                .next()
                .filter(|path| !path.is_empty())
                .unwrap_or("/")
                .to_string()
        })
}

fn env_bool(key: &str) -> bool {
    let Ok(v) = std::env::var(key) else {
        return false;
    };
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "y" | "on"
    )
}

fn env_bool_default(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "y" | "on"
        ),
        Err(_) => default,
    }
}

pub fn http_debug_options() -> HttpDebugOptions {
    static OPT: OnceLock<HttpDebugOptions> = OnceLock::new();
    *OPT.get_or_init(|| {
        let enabled = env_bool("CODEX_HELPER_HTTP_DEBUG");
        let all = env_bool("CODEX_HELPER_HTTP_DEBUG_ALL");
        let max_body_bytes = std::env::var("CODEX_HELPER_HTTP_DEBUG_BODY_MAX")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(64 * 1024);
        HttpDebugOptions {
            enabled,
            all,
            max_body_bytes,
        }
    })
}

pub fn http_warn_options() -> HttpWarnOptions {
    static OPT: OnceLock<HttpWarnOptions> = OnceLock::new();
    *OPT.get_or_init(|| {
        // Default ON: for non-2xx, record header-only diagnostics without request or response
        // bodies. The retained limit keeps the existing configuration contract stable.
        // Set CODEX_HELPER_HTTP_WARN=0 to disable.
        let enabled = env_bool_default("CODEX_HELPER_HTTP_WARN", true);
        let all = env_bool_default("CODEX_HELPER_HTTP_WARN_ALL", false);
        let max_body_bytes = std::env::var("CODEX_HELPER_HTTP_WARN_BODY_MAX")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(8 * 1024);
        HttpWarnOptions {
            enabled,
            all,
            max_body_bytes,
        }
    })
}

pub fn should_include_http_debug(status_code: u16) -> bool {
    let opt = http_debug_options();
    if !opt.enabled {
        return false;
    }
    if opt.all {
        return true;
    }
    !is_logical_request_success_status(status_code)
}

pub fn should_include_http_warn(status_code: u16) -> bool {
    let opt = http_warn_options();
    if !opt.enabled {
        return false;
    }
    if opt.all {
        return true;
    }
    !is_logical_request_success_status(status_code)
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct AuthResolutionLog {
    /// Where the upstream `Authorization` header value came from (never includes the secret).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization: Option<String>,
    /// Where the upstream `X-API-Key` header value came from (never includes the secret).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_api_key: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct BodyPreview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub encoding: String,
    pub data: String,
    pub truncated: bool,
    pub original_len: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
}

fn normalize_content_type(content_type: Option<&str>) -> Option<&str> {
    let ct = content_type?.trim();
    let (base, _) = ct.split_once(';').unwrap_or((ct, ""));
    let base = base.trim();
    if base.is_empty() { None } else { Some(base) }
}

fn is_textual_content_type(content_type: Option<&str>) -> bool {
    let Some(ct) = normalize_content_type(content_type) else {
        return false;
    };
    ct.starts_with("text/")
        || ct == "application/json"
        || ct.ends_with("+json")
        || ct == "application/x-www-form-urlencoded"
        || ct == "application/xml"
        || ct.ends_with("+xml")
        || ct == "text/event-stream"
}

pub fn make_body_preview(bytes: &[u8], content_type: Option<&str>, max: usize) -> BodyPreview {
    let original_len = bytes.len();
    let take = original_len.min(max);
    let truncated = original_len > take;
    let slice = &bytes[..take];

    if is_textual_content_type(content_type) {
        let text = String::from_utf8_lossy(slice).into_owned();
        return BodyPreview {
            content_type: normalize_content_type(content_type).map(|s| s.to_string()),
            encoding: "utf8".to_string(),
            data: text,
            truncated,
            original_len,
            window: None,
        };
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(slice);
    BodyPreview {
        content_type: normalize_content_type(content_type).map(|s| s.to_string()),
        encoding: "base64".to_string(),
        data: b64,
        truncated,
        original_len,
        window: None,
    }
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct HttpDebugLog {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_attempt_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body_len: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_request_body_len: Option<usize>,
    /// Time spent waiting for upstream response headers (ms), measured from just before sending the upstream request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_headers_ms: Option<u64>,
    /// Time to first upstream response body chunk (ms), measured from just before sending the upstream request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_first_chunk_ms: Option<u64>,
    /// Time spent reading upstream response body to completion (ms). Only meaningful for non-stream responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_body_read_ms: Option<u64>,
    /// A coarse classification for upstream non-2xx responses (e.g. Cloudflare challenge).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_error_class: Option<String>,
    /// A human-readable hint to help diagnose upstream non-2xx responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_error_hint: Option<String>,
    /// Cloudflare request id when present (from `cf-ray` response header).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_cf_ray: Option<String>,
    pub client_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_uri: Option<String>,
    pub client_headers: Vec<HeaderEntry>,
    pub upstream_request_headers: Vec<HeaderEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_resolution: Option<AuthResolutionLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_body: Option<BodyPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_request_body: Option<BodyPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_response_headers: Option<Vec<HeaderEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_response_body: Option<BodyPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct ServiceTierLog {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
}

fn service_tier_log_is_empty(value: &ServiceTierLog) -> bool {
    value.requested.is_none() && value.effective.is_none() && value.actual.is_none()
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct CodexBridgeLog {
    pub patch_mode: String,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub remote_compaction_v1_request: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub remote_compaction_v2_request: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub downgraded_to_responses_compact: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub responses_websocket_request: bool,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub strips_client_auth: bool,
}

#[derive(Debug, Serialize)]
pub struct RequestLog<'a> {
    pub timestamp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    pub service: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub status_code: u16,
    pub duration_ms: u64,
    /// Time to first byte / first chunk from the upstream (ms).
    /// - For streaming responses: measured to the first response body chunk.
    /// - For non-streaming responses: measured to response headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_identity_source: Option<SessionIdentitySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "service_tier_log_is_empty", default)]
    pub service_tier: ServiceTierLog,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_bridge: Option<CodexBridgeLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_debug: Option<HttpDebugLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_debug_ref: Option<HttpDebugRef>,
    #[serde(default, skip_serializing_if = "http_debug_attempt_refs_is_empty")]
    pub http_debug_attempt_refs: Vec<HttpDebugAttemptRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_decision: Option<RouteDecisionProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryInfo>,
    #[serde(default, skip_serializing_if = "provider_signals_is_empty")]
    pub provider_signals: Vec<ProviderSignal>,
    #[serde(default, skip_serializing_if = "policy_actions_is_empty")]
    pub policy_actions: Vec<PolicyAction>,
}

#[derive(Debug, Serialize, Clone)]
pub struct HttpDebugRef {
    pub id: String,
    pub file: String,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct HttpDebugAttemptRef {
    pub route_attempt_index: u32,
    pub id: String,
    pub file: String,
}

fn http_debug_attempt_refs_is_empty(value: &[HttpDebugAttemptRef]) -> bool {
    value.is_empty()
}

fn route_attempts_is_empty(value: &[RouteAttemptLog]) -> bool {
    value.is_empty()
}

fn route_attempt_avoided_candidate_indices_is_empty(value: &[usize]) -> bool {
    value.is_empty()
}

fn route_attempt_route_path_is_empty(value: &[String]) -> bool {
    value.is_empty()
}

fn provider_signals_is_empty(value: &[ProviderSignal]) -> bool {
    value.is_empty()
}

fn policy_actions_is_empty(value: &[PolicyAction]) -> bool {
    value.is_empty()
}

fn provider_signals_from_retry(retry: Option<&RetryInfo>) -> Vec<ProviderSignal> {
    retry
        .into_iter()
        .flat_map(|retry| retry.route_attempts.iter())
        .flat_map(|attempt| attempt.provider_signals.iter().cloned())
        .collect()
}

fn policy_actions_from_retry(retry: Option<&RetryInfo>) -> Vec<PolicyAction> {
    retry
        .into_iter()
        .flat_map(|retry| retry.route_attempts.iter())
        .flat_map(|attempt| attempt.policy_actions.iter().cloned())
        .collect()
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct RouteAttemptLog {
    pub attempt_index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preference_group: Option<u32>,
    #[serde(default, skip_serializing_if = "route_attempt_route_path_is_empty")]
    pub route_path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_max_attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_max_attempts: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "route_attempt_avoided_candidate_indices_is_empty"
    )]
    pub avoided_candidate_indices: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avoided_total: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_upstreams: Option<usize>,
    pub decision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_headers_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_reason: Option<String>,
    #[serde(default, skip_serializing_if = "provider_signals_is_empty")]
    pub provider_signals: Vec<ProviderSignal>,
    #[serde(default, skip_serializing_if = "policy_actions_is_empty")]
    pub policy_actions: Vec<PolicyAction>,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub skipped: bool,
    #[serde(skip)]
    pub(crate) http_debug: Option<HttpDebugLog>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RetryInfo {
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "route_attempts_is_empty")]
    pub route_attempts: Vec<RouteAttemptLog>,
}

impl RouteAttemptLog {
    pub fn stable_code(&self) -> &str {
        self.code.as_deref().unwrap_or_else(|| {
            route_attempt_code(self.decision.as_str(), self.error_class.as_deref())
        })
    }

    pub fn refresh_code(&mut self) {
        self.code = Some(
            route_attempt_code(self.decision.as_str(), self.error_class.as_deref()).to_string(),
        );
    }
}

fn route_attempt_code(decision: &str, error_class: Option<&str>) -> &'static str {
    if matches!(
        error_class,
        Some("reasoning_guard_triggered" | "reasoning_guard_blocked")
    ) {
        return "failed_reasoning_guard";
    }
    match decision {
        "selected" => "selected",
        "observed" => "observed",
        "completed" => "completed",
        "failed_status" => "failed_status",
        "failed_client_request" => "failed_client_request",
        "failed_reasoning_guard" => "failed_reasoning_guard",
        "failed_lifecycle_store" => "failed_lifecycle_store",
        "failed_transport" => "failed_transport",
        "failed_target_build" => "failed_target_build",
        "failed_body_read" => "failed_body_read",
        "failed_body_too_large" => "failed_body_too_large",
        "skipped_capability_mismatch" => "skipped_capability_mismatch",
        "all_upstreams_avoided" => "all_upstreams_avoided",
        "route_unavailable" => "route_unavailable",
        _ => "unknown",
    }
}

#[derive(Debug, Serialize)]
struct HttpDebugLogEntry<'a> {
    pub id: &'a str,
    pub timestamp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_attempt_index: Option<u32>,
    pub service: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub status_code: u16,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_identity_source: Option<SessionIdentitySource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "service_tier_log_is_empty", default)]
    pub service_tier: ServiceTierLog,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_bridge: Option<CodexBridgeLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_decision: Option<RouteDecisionProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryInfo>,
    pub http_debug: HttpDebugLog,
}

#[derive(Debug, Clone, Copy)]
struct RequestLogOptions {
    retention: LogRetention,
    only_errors: bool,
}

pub fn request_log_path() -> PathBuf {
    proxy_home_dir().join("logs").join("requests.jsonl")
}

fn debug_log_path() -> PathBuf {
    proxy_home_dir().join("logs").join("requests_debug.jsonl")
}

fn log_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn http_debug_split_enabled() -> bool {
    // When HTTP debug is enabled for all requests, splitting is strongly recommended to keep
    // the main request log lightweight. Users can also enable splitting explicitly.
    env_bool_default("CODEX_HELPER_HTTP_DEBUG_SPLIT", true) || http_debug_options().all
}

fn request_log_options() -> RequestLogOptions {
    static OPT: OnceLock<RequestLogOptions> = OnceLock::new();
    *OPT.get_or_init(|| {
        let retention = LogRetention::from_env(
            "CODEX_HELPER_REQUEST_LOG_MAX_BYTES",
            "CODEX_HELPER_REQUEST_LOG_MAX_FILES",
            50 * 1024 * 1024,
            10,
        );
        let only_errors = env_bool("CODEX_HELPER_REQUEST_LOG_ONLY_ERRORS");
        RequestLogOptions {
            retention,
            only_errors,
        }
    })
}

pub fn request_log_retention() -> LogRetention {
    request_log_options().retention
}

fn append_json_line(path: &PathBuf, opt: RequestLogOptions, line: &str) -> bool {
    append_line(path, opt.retention, line).is_ok()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn log_committed_request_with_debug(
    request_id: Option<u64>,
    service: &str,
    method: &str,
    path: &str,
    status_code: u16,
    duration_ms: u64,
    ttfb_ms: Option<u64>,
    provider_id: Option<String>,
    endpoint_id: Option<String>,
    provider_endpoint_key: Option<String>,
    upstream_origin: Option<String>,
    session_id: Option<String>,
    session_identity_source: Option<SessionIdentitySource>,
    cwd: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: ServiceTierLog,
    codex_bridge: Option<CodexBridgeLog>,
    usage: Option<UsageMetrics>,
    route_decision: Option<RouteDecisionProvenance>,
    retry: Option<RetryInfo>,
    http_debug: Option<HttpDebugLog>,
) {
    let opt = request_log_options();
    if opt.only_errors && is_logical_request_success_status(status_code) {
        return;
    }

    let ts = now_ms();
    let trace_id = request_id.map(|id| request_trace_id(service, id));
    let upstream_origin = upstream_origin.and_then(|value| self::upstream_origin(value.as_str()));
    let route_decision = route_decision_for_request_log(route_decision);
    let http_debug = http_debug.map(http_debug_for_request_log);
    let terminal_route_attempt_index = http_debug
        .as_ref()
        .and_then(|debug| debug.route_attempt_index);
    let mut http_debug_attempts = retry
        .as_ref()
        .into_iter()
        .flat_map(|retry| retry.route_attempts.iter())
        .filter_map(|attempt| {
            attempt
                .http_debug
                .clone()
                .map(|debug| (attempt.clone(), http_debug_for_request_log(debug)))
        })
        .collect::<Vec<_>>();
    if let (Some(attempt_index), Some(terminal_debug)) =
        (terminal_route_attempt_index, http_debug.as_ref())
    {
        if let Some((_, debug)) = http_debug_attempts
            .iter_mut()
            .find(|(attempt, _)| attempt.attempt_index == attempt_index)
        {
            *debug = terminal_debug.clone();
        } else {
            let attempt = retry
                .as_ref()
                .and_then(|retry| {
                    retry
                        .route_attempts
                        .iter()
                        .find(|attempt| attempt.attempt_index == attempt_index)
                })
                .cloned()
                .unwrap_or_else(|| RouteAttemptLog {
                    attempt_index,
                    provider_id: provider_id.clone(),
                    endpoint_id: endpoint_id.clone(),
                    provider_endpoint_key: provider_endpoint_key.clone(),
                    status_code: Some(status_code),
                    model: model.clone(),
                    upstream_headers_ms: ttfb_ms,
                    duration_ms: Some(duration_ms),
                    ..RouteAttemptLog::default()
                });
            http_debug_attempts.push((attempt, terminal_debug.clone()));
        }
    }
    http_debug_attempts.sort_by_key(|(attempt, _)| attempt.attempt_index);

    static DEBUG_SEQ: AtomicU64 = AtomicU64::new(0);
    let mut http_debug_for_main = http_debug;
    let mut http_debug_ref: Option<HttpDebugRef> = None;
    let mut http_debug_attempt_refs = Vec::new();

    let log_file_path = request_log_path();
    let split_http_debug = http_debug_split_enabled();

    let _guard = match log_lock().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };

    // Optional: write large http_debug blobs to a separate file and keep only a reference in requests.jsonl.
    if split_http_debug && let Some(h) = http_debug_for_main.take() {
        let seq = DEBUG_SEQ.fetch_add(1, Ordering::Relaxed);
        let id = format!("{ts}-{seq}");
        let debug_entry = HttpDebugLogEntry {
            id: &id,
            timestamp_ms: ts,
            request_id,
            trace_id: trace_id.clone(),
            route_attempt_index: h.route_attempt_index,
            service,
            method,
            path,
            status_code,
            duration_ms,
            ttfb_ms,
            provider_id: provider_id.clone(),
            endpoint_id: endpoint_id.clone(),
            provider_endpoint_key: provider_endpoint_key.clone(),
            upstream_origin: upstream_origin.clone(),
            session_id: session_id.clone(),
            session_identity_source,
            cwd: cwd.clone(),
            model: model.clone(),
            reasoning_effort: reasoning_effort.clone(),
            service_tier: service_tier.clone(),
            codex_bridge: codex_bridge.clone(),
            usage: usage.clone(),
            route_decision: route_decision.clone(),
            retry: retry.clone(),
            http_debug: h,
        };

        let debug_path = debug_log_path();
        let mut wrote_debug = false;
        if let Ok(line) = serde_json::to_string(&debug_entry) {
            wrote_debug = append_json_line(&debug_path, opt, &line);
        }

        if wrote_debug {
            http_debug_ref = Some(HttpDebugRef {
                id,
                file: "requests_debug.jsonl".to_string(),
            });
        } else {
            // If we failed to write the debug entry, fall back to inline logging to avoid losing data.
            let HttpDebugLogEntry { http_debug, .. } = debug_entry;
            http_debug_for_main = Some(http_debug);
        }
    }

    if split_http_debug {
        if let (Some(route_attempt_index), Some(reference)) =
            (terminal_route_attempt_index, http_debug_ref.as_ref())
        {
            http_debug_attempt_refs.push(HttpDebugAttemptRef {
                route_attempt_index,
                id: reference.id.clone(),
                file: reference.file.clone(),
            });
        }

        let debug_path = debug_log_path();
        for (attempt, http_debug) in http_debug_attempts {
            if Some(attempt.attempt_index) == terminal_route_attempt_index
                && http_debug_ref.is_some()
            {
                continue;
            }

            let seq = DEBUG_SEQ.fetch_add(1, Ordering::Relaxed);
            let id = format!("{ts}-{seq}");
            let debug_entry = HttpDebugLogEntry {
                id: &id,
                timestamp_ms: ts,
                request_id,
                trace_id: trace_id.clone(),
                route_attempt_index: Some(attempt.attempt_index),
                service,
                method,
                path,
                status_code: attempt.status_code.unwrap_or(status_code),
                duration_ms: attempt.duration_ms.unwrap_or(duration_ms),
                ttfb_ms: attempt.upstream_headers_ms,
                provider_id: attempt.provider_id.or_else(|| provider_id.clone()),
                endpoint_id: attempt.endpoint_id.or_else(|| endpoint_id.clone()),
                provider_endpoint_key: attempt
                    .provider_endpoint_key
                    .or_else(|| provider_endpoint_key.clone()),
                upstream_origin: http_debug
                    .upstream_origin
                    .clone()
                    .or_else(|| upstream_origin.clone()),
                session_id: session_id.clone(),
                session_identity_source,
                cwd: cwd.clone(),
                model: attempt.model.or_else(|| model.clone()),
                reasoning_effort: reasoning_effort.clone(),
                service_tier: service_tier.clone(),
                codex_bridge: codex_bridge.clone(),
                usage: usage.clone(),
                route_decision: route_decision.clone(),
                retry: retry.clone(),
                http_debug,
            };
            let wrote_debug = serde_json::to_string(&debug_entry)
                .ok()
                .is_some_and(|line| append_json_line(&debug_path, opt, &line));
            if wrote_debug {
                http_debug_attempt_refs.push(HttpDebugAttemptRef {
                    route_attempt_index: attempt.attempt_index,
                    id,
                    file: "requests_debug.jsonl".to_string(),
                });
            }
        }
        http_debug_attempt_refs.sort_by_key(|reference| reference.route_attempt_index);
    }

    let provider_signals = provider_signals_from_retry(retry.as_ref());
    let policy_actions = policy_actions_from_retry(retry.as_ref());

    let entry = RequestLog {
        timestamp_ms: ts,
        request_id,
        trace_id,
        service,
        method,
        path,
        status_code,
        duration_ms,
        ttfb_ms,
        provider_id,
        endpoint_id,
        provider_endpoint_key,
        upstream_origin,
        session_id,
        session_identity_source,
        cwd,
        model,
        reasoning_effort,
        service_tier,
        codex_bridge,
        usage,
        http_debug: http_debug_for_main,
        http_debug_ref,
        http_debug_attempt_refs,
        route_decision,
        retry,
        provider_signals,
        policy_actions,
    };

    if let Ok(line) = serde_json::to_string(&entry) {
        let _ = append_json_line(&log_file_path, opt, &line);
    }

    if let Ok(payload) = serde_json::to_value(&entry) {
        append_control_trace_payload(
            opt,
            "request_completed",
            Some(service),
            request_id,
            Some("request_completed"),
            ts,
            payload,
        );
    }
}

#[cfg(test)]
#[path = "logging/tests.rs"]
mod tests;
