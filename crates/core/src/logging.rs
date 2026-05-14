use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::config::proxy_home_dir;
use crate::usage::UsageMetrics;

#[path = "logging/control_trace.rs"]
mod control_trace_impl;

use control_trace_impl::append_control_trace_payload;
pub use control_trace_impl::{
    ControlTraceDetail, ControlTraceLogEntry, control_trace_path, log_retry_trace,
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
    let service = service.trim();
    if service.is_empty() {
        format!("request-{request_id}")
    } else {
        format!("{service}-{request_id}")
    }
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
        // Default ON: for non-2xx, record a small header/body preview to help debug upstream errors.
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
    !(200..300).contains(&status_code)
}

pub fn should_include_http_warn(status_code: u16) -> bool {
    let opt = http_warn_options();
    if !opt.enabled {
        return false;
    }
    if opt.all {
        return true;
    }
    !(200..300).contains(&status_code)
}

#[derive(Debug, Serialize, Clone)]
pub struct HeaderEntry {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct AuthResolutionLog {
    /// Where the upstream `Authorization` header value came from (never includes the secret).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization: Option<String>,
    /// Where the upstream `X-API-Key` header value came from (never includes the secret).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_api_key: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct BodyPreview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub encoding: String,
    pub data: String,
    pub truncated: bool,
    pub original_len: usize,
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
        };
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(slice);
    BodyPreview {
        content_type: normalize_content_type(content_type).map(|s| s.to_string()),
        encoding: "base64".to_string(),
        data: b64,
        truncated,
        original_len,
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct HttpDebugLog {
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
    pub target_url: String,
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
    pub station_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    pub upstream_base_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "service_tier_log_is_empty", default)]
    pub service_tier: ServiceTierLog,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_debug: Option<HttpDebugLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_debug_ref: Option<HttpDebugRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryInfo>,
}

#[derive(Debug, Serialize, Clone)]
pub struct HttpDebugRef {
    pub id: String,
    pub file: String,
}

fn route_attempts_is_empty(value: &[RouteAttemptLog]) -> bool {
    value.is_empty()
}

fn route_attempt_avoid_for_station_is_empty(value: &[usize]) -> bool {
    value.is_empty()
}

fn route_attempt_avoided_candidate_indices_is_empty(value: &[usize]) -> bool {
    value.is_empty()
}

fn route_attempt_route_path_is_empty(value: &[String]) -> bool {
    value.is_empty()
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_index: Option<usize>,
    #[serde(
        default,
        skip_serializing_if = "route_attempt_avoid_for_station_is_empty"
    )]
    pub avoid_for_station: Vec<usize>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub skipped: bool,
    pub raw: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RetryInfo {
    pub attempts: u32,
    pub upstream_chain: Vec<String>,
    #[serde(default, skip_serializing_if = "route_attempts_is_empty")]
    pub route_attempts: Vec<RouteAttemptLog>,
}

impl RetryInfo {
    pub fn route_attempts_or_derived(&self) -> Vec<RouteAttemptLog> {
        if self.route_attempts.is_empty() {
            parse_route_attempts_from_chain(&self.upstream_chain)
        } else {
            self.route_attempts.clone()
        }
    }

    pub fn touches_station(&self, station_name: &str) -> bool {
        let station_name = station_name.trim();
        if station_name.is_empty() {
            return false;
        }
        self.route_attempts_or_derived()
            .iter()
            .any(|attempt| attempt.station_name.as_deref() == Some(station_name))
    }

    pub fn touched_other_station(&self, final_station: Option<&str>) -> bool {
        let Some(final_station) = final_station
            .map(str::trim)
            .filter(|station| !station.is_empty())
        else {
            return false;
        };

        self.route_attempts_or_derived()
            .iter()
            .filter_map(|attempt| attempt.station_name.as_deref())
            .any(|station| station != final_station)
    }
}

pub(crate) fn parse_route_attempts_from_chain(chain: &[String]) -> Vec<RouteAttemptLog> {
    chain
        .iter()
        .enumerate()
        .map(|(idx, raw)| parse_route_attempt_from_chain_entry(raw, idx as u32))
        .collect()
}

fn parse_route_attempt_from_chain_entry(raw: &str, attempt_index: u32) -> RouteAttemptLog {
    let raw = raw.trim();
    let mut attempt = RouteAttemptLog {
        attempt_index,
        decision: "observed".to_string(),
        raw: raw.to_string(),
        ..Default::default()
    };

    if raw.starts_with("all_upstreams_avoided") {
        attempt.decision = "all_upstreams_avoided".to_string();
        attempt.reason = route_chain_value(raw, "total").map(|total| format!("total={total}"));
        attempt.skipped = true;
        return attempt;
    }

    let (target, metadata, upstream_index) = split_route_chain_entry(raw);
    attempt.upstream_index = upstream_index;
    if let Some(target) = target {
        let (station_name, upstream_base_url) = parse_route_chain_target(target);
        attempt.station_name = station_name;
        attempt.upstream_base_url = upstream_base_url;
    }
    apply_route_chain_identity_metadata(&mut attempt, raw);

    if let Some(status_code) =
        route_chain_value(metadata, "status").and_then(|value| value.parse::<u16>().ok())
    {
        attempt.status_code = Some(status_code);
        attempt.decision = if (200..300).contains(&status_code) {
            "completed".to_string()
        } else {
            "failed_status".to_string()
        };
    }

    if let Some(error_class) =
        route_chain_value(metadata, "class").filter(|value| !value.eq_ignore_ascii_case("-"))
    {
        attempt.error_class = Some(error_class);
    }
    if let Some(model) = route_chain_value(metadata, "model") {
        attempt.model = Some(model);
    }

    if let Some(model) = route_chain_value(metadata, "skipped_unsupported_model") {
        attempt.decision = "skipped_capability_mismatch".to_string();
        attempt.reason = Some("unsupported_model".to_string());
        attempt.model = Some(model);
        attempt.skipped = true;
    }

    if let Some(error) = route_chain_value(metadata, "transport_error") {
        attempt.decision = "failed_transport".to_string();
        attempt.reason = Some(error);
        attempt.error_class = Some("upstream_transport_error".to_string());
    }

    if let Some(error) = route_chain_value(metadata, "target_build_error") {
        attempt.decision = "failed_target_build".to_string();
        attempt.reason = Some(error);
        attempt.error_class = Some("target_build_error".to_string());
    }

    if let Some(error) = route_chain_value(metadata, "body_read_error") {
        attempt.decision = "failed_body_read".to_string();
        attempt.reason = Some(error);
        attempt.error_class = Some("upstream_body_read_error".to_string());
    }
    if let Some(error) = route_chain_value(metadata, "body_too_large") {
        attempt.decision = "failed_body_too_large".to_string();
        attempt.reason = Some(error);
        attempt.error_class = Some("upstream_response_body_too_large".to_string());
    }

    attempt
}

fn apply_route_chain_identity_metadata(attempt: &mut RouteAttemptLog, raw: &str) {
    if let Some(station) = route_chain_value(raw, "station") {
        attempt.station_name = non_empty_str(station.as_str()).map(ToOwned::to_owned);
    }
    if let Some(provider_endpoint_key) = route_chain_value(raw, "endpoint") {
        let provider_endpoint_key = provider_endpoint_key.trim().to_string();
        if !provider_endpoint_key.is_empty() && provider_endpoint_key != "-" {
            let parts = provider_endpoint_key.split('/').collect::<Vec<_>>();
            if parts.len() >= 3 {
                attempt.provider_id = non_empty_str(parts[1]).map(ToOwned::to_owned);
                attempt.endpoint_id = non_empty_str(parts[2]).map(ToOwned::to_owned);
            }
            attempt.provider_endpoint_key = Some(provider_endpoint_key);
        }
    }
    if let Some(group) = route_chain_value(raw, "group").and_then(|value| value.parse::<u32>().ok())
    {
        attempt.preference_group = Some(group);
    }
    if let Some(station) = route_chain_value(raw, "compat_station") {
        attempt.station_name = non_empty_str(station.as_str()).map(ToOwned::to_owned);
    }
    if let Some(index) =
        route_chain_value(raw, "upstream_index").and_then(|value| value.parse::<usize>().ok())
    {
        attempt.upstream_index = Some(index);
    }
    if let Some(url) = route_chain_value(raw, "url") {
        attempt.upstream_base_url = non_empty_str(url.as_str()).map(ToOwned::to_owned);
    }
    if let Some(indices) = route_chain_value(raw, "avoid_candidates") {
        attempt.avoided_candidate_indices = parse_usize_list(indices.as_str());
    }
    if let Some(indices) = route_chain_value(raw, "avoid") {
        attempt.avoid_for_station = parse_usize_list(indices.as_str());
    }
}

fn parse_usize_list(raw: &str) -> Vec<usize> {
    raw.split(',')
        .filter_map(|part| part.trim().parse::<usize>().ok())
        .collect()
}

fn split_route_chain_entry(raw: &str) -> (Option<&str>, &str, Option<usize>) {
    if let Some(idx_start) = raw.find(" (idx=") {
        let target = raw[..idx_start].trim();
        let after_idx = &raw[idx_start + " (idx=".len()..];
        if let Some(idx_end) = after_idx.find(')') {
            let upstream_index = after_idx[..idx_end].trim().parse::<usize>().ok();
            let metadata = after_idx[idx_end + 1..].trim();
            return (non_empty_str(target), metadata, upstream_index);
        }
        return (non_empty_str(target), after_idx.trim(), None);
    }

    if let Some(metadata_start) = first_route_chain_key_start(raw) {
        let target = raw[..metadata_start].trim();
        let metadata = raw[metadata_start..].trim();
        return (non_empty_str(target), metadata, None);
    }

    (non_empty_str(raw), "", None)
}

fn parse_route_chain_target(target: &str) -> (Option<String>, Option<String>) {
    let target = target.trim();
    if target.is_empty() {
        return (None, None);
    }
    if target.starts_with("http://") || target.starts_with("https://") {
        return (None, Some(target.to_string()));
    }

    if let Some(pos) = target.find(":http://").or_else(|| target.find(":https://")) {
        let station = target[..pos].trim();
        let base_url = target[pos + 1..].trim();
        return (
            non_empty_str(station).map(ToOwned::to_owned),
            non_empty_str(base_url).map(ToOwned::to_owned),
        );
    }

    (None, Some(target.to_string()))
}

fn first_route_chain_key_start(raw: &str) -> Option<usize> {
    ROUTE_CHAIN_KEYS
        .iter()
        .filter_map(|key| find_route_chain_key(raw, format!("{key}=").as_str()))
        .min()
}

fn route_chain_value(raw: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let start = find_route_chain_key(raw, needle.as_str())?;
    let value_start = start + needle.len();
    let value_end = ROUTE_CHAIN_KEYS
        .iter()
        .filter(|candidate| **candidate != key)
        .filter_map(|candidate| {
            raw[value_start..]
                .find(&format!(" {candidate}="))
                .map(|offset| value_start + offset)
        })
        .min()
        .unwrap_or(raw.len());
    non_empty_str(raw[value_start..value_end].trim()).map(ToOwned::to_owned)
}

fn find_route_chain_key(raw: &str, needle: &str) -> Option<usize> {
    let mut offset = 0usize;
    while let Some(pos) = raw[offset..].find(needle) {
        let absolute = offset + pos;
        if absolute == 0 || raw[..absolute].ends_with(' ') {
            return Some(absolute);
        }
        offset = absolute + needle.len();
    }
    None
}

fn non_empty_str(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() { None } else { Some(value) }
}

const ROUTE_CHAIN_KEYS: &[&str] = &[
    "status",
    "class",
    "model",
    "transport_error",
    "target_build_error",
    "body_read_error",
    "body_too_large",
    "skipped_unsupported_model",
    "total",
    "station",
    "endpoint",
    "group",
    "compat_station",
    "upstream_index",
    "url",
    "avoid_candidates",
    "avoid",
];

#[derive(Debug, Serialize)]
struct HttpDebugLogEntry<'a> {
    pub id: &'a str,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    pub upstream_base_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "service_tier_log_is_empty", default)]
    pub service_tier: ServiceTierLog,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryInfo>,
    pub http_debug: HttpDebugLog,
}

#[derive(Debug, Clone, Copy)]
struct RequestLogOptions {
    max_bytes: u64,
    max_files: usize,
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
        let max_bytes = std::env::var("CODEX_HELPER_REQUEST_LOG_MAX_BYTES")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(50 * 1024 * 1024);
        let max_files = std::env::var("CODEX_HELPER_REQUEST_LOG_MAX_FILES")
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(10);
        let only_errors = env_bool("CODEX_HELPER_REQUEST_LOG_ONLY_ERRORS");
        RequestLogOptions {
            max_bytes,
            max_files,
            only_errors,
        }
    })
}

fn ensure_log_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
}

fn append_json_line(path: &PathBuf, opt: RequestLogOptions, line: &str) -> bool {
    rotate_and_prune_if_needed(path, opt);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        return writeln!(file, "{}", line).is_ok();
    }
    false
}

fn rotate_and_prune_if_needed(path: &PathBuf, opt: RequestLogOptions) {
    if opt.max_bytes == 0 {
        return;
    }
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() < opt.max_bytes {
        return;
    }

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let prefix = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("requests");
    let rotated_name = format!("{prefix}.{ts}.jsonl");
    let rotated_path = path.with_file_name(rotated_name);
    let _ = fs::rename(path, &rotated_path);

    let Some(dir) = path.parent() else {
        return;
    };
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut rotated: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.starts_with(&format!("{prefix}.")) && s.ends_with(".jsonl"))
                .unwrap_or(false)
        })
        .collect();
    if rotated.len() <= opt.max_files {
        return;
    }
    rotated.sort();
    let remove_count = rotated.len().saturating_sub(opt.max_files);
    for p in rotated.into_iter().take(remove_count) {
        let _ = fs::remove_file(p);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn log_request_with_debug(
    request_id: Option<u64>,
    service: &str,
    method: &str,
    path: &str,
    status_code: u16,
    duration_ms: u64,
    ttfb_ms: Option<u64>,
    station_name: Option<&str>,
    provider_id: Option<String>,
    endpoint_id: Option<String>,
    provider_endpoint_key: Option<String>,
    upstream_base_url: &str,
    session_id: Option<String>,
    cwd: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: ServiceTierLog,
    usage: Option<UsageMetrics>,
    retry: Option<RetryInfo>,
    http_debug: Option<HttpDebugLog>,
) {
    let opt = request_log_options();
    if opt.only_errors && (200..300).contains(&status_code) {
        return;
    }

    let ts = now_ms();
    let trace_id = request_id.map(|id| request_trace_id(service, id));

    static DEBUG_SEQ: AtomicU64 = AtomicU64::new(0);
    let mut http_debug_for_main = http_debug;
    let mut http_debug_ref: Option<HttpDebugRef> = None;

    let log_file_path = request_log_path();
    ensure_log_parent(&log_file_path);

    let _guard = match log_lock().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };

    // Optional: write large http_debug blobs to a separate file and keep only a reference in requests.jsonl.
    if http_debug_split_enabled()
        && let Some(h) = http_debug_for_main.take()
    {
        let seq = DEBUG_SEQ.fetch_add(1, Ordering::Relaxed);
        let id = format!("{ts}-{seq}");
        let debug_entry = HttpDebugLogEntry {
            id: &id,
            timestamp_ms: ts,
            request_id,
            trace_id: trace_id.clone(),
            service,
            method,
            path,
            status_code,
            duration_ms,
            ttfb_ms,
            station_name,
            provider_id: provider_id.clone(),
            endpoint_id: endpoint_id.clone(),
            provider_endpoint_key: provider_endpoint_key.clone(),
            upstream_base_url,
            session_id: session_id.clone(),
            cwd: cwd.clone(),
            reasoning_effort: reasoning_effort.clone(),
            service_tier: service_tier.clone(),
            usage: usage.clone(),
            retry: retry.clone(),
            http_debug: h,
        };

        let debug_path = debug_log_path();
        ensure_log_parent(&debug_path);
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
        station_name,
        provider_id,
        endpoint_id,
        provider_endpoint_key,
        upstream_base_url,
        session_id,
        cwd,
        reasoning_effort,
        service_tier,
        usage,
        http_debug: http_debug_for_main,
        http_debug_ref,
        retry,
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
