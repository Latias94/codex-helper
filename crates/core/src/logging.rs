use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::config::proxy_home_dir;
use crate::usage::UsageMetrics;

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

#[derive(Debug, Serialize, Clone, Default, PartialEq, Eq)]
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
    pub station_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct RetryInfo {
    pub attempts: u32,
    pub upstream_chain: Vec<String>,
}

#[derive(Debug, Serialize)]
struct HttpDebugLogEntry<'a> {
    pub id: &'a str,
    pub timestamp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    pub service: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub status_code: u16,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    pub station_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlTraceLogEntry {
    pub ts_ms: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default)]
    pub payload: JsonValue,
}

fn log_path() -> PathBuf {
    proxy_home_dir().join("logs").join("requests.jsonl")
}

fn debug_log_path() -> PathBuf {
    proxy_home_dir().join("logs").join("requests_debug.jsonl")
}

pub fn control_trace_path() -> PathBuf {
    std::env::var("CODEX_HELPER_CONTROL_TRACE_PATH")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| proxy_home_dir().join("logs").join("control_trace.jsonl"))
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

fn control_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_bool_default("CODEX_HELPER_CONTROL_TRACE", true))
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

fn retry_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_bool("CODEX_HELPER_RETRY_TRACE"))
}

fn retry_trace_path() -> PathBuf {
    std::env::var("CODEX_HELPER_RETRY_TRACE_PATH")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| proxy_home_dir().join("logs").join("retry_trace.jsonl"))
}

fn control_trace_read_window(limit: usize) -> usize {
    limit.saturating_mul(4).clamp(80, 400)
}

pub fn read_recent_control_trace_entries(
    limit: usize,
) -> anyhow::Result<Vec<ControlTraceLogEntry>> {
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader};

    let path = control_trace_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let mut ring = VecDeque::with_capacity(control_trace_read_window(limit));
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if ring.len() == ring.capacity() {
            ring.pop_front();
        }
        ring.push_back(line);
    }

    let mut out = Vec::new();
    for line in ring {
        let Ok(entry) = serde_json::from_str::<ControlTraceLogEntry>(&line) else {
            continue;
        };
        out.push(entry);
    }
    out.sort_by_key(|entry| std::cmp::Reverse(entry.ts_ms));
    Ok(out)
}

fn json_u64_field(value: &JsonValue, key: &str) -> Option<u64> {
    value.get(key).and_then(|value| match value {
        JsonValue::Number(number) => number.as_u64(),
        JsonValue::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    })
}

fn json_string_field(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn ensure_log_parent(path: &PathBuf) {
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

fn append_control_trace_payload(
    opt: RequestLogOptions,
    kind: &'static str,
    service: Option<&str>,
    request_id: Option<u64>,
    event: Option<&str>,
    ts_ms: u64,
    payload: JsonValue,
) {
    if !control_trace_enabled() {
        return;
    }
    let path = control_trace_path();
    ensure_log_parent(&path);
    let entry = make_control_trace_entry(kind, service, request_id, event, ts_ms, payload);
    if let Ok(line) = serde_json::to_string(&entry) {
        let _ = append_json_line(&path, opt, &line);
    }
}

fn make_control_trace_entry(
    kind: &'static str,
    service: Option<&str>,
    request_id: Option<u64>,
    event: Option<&str>,
    ts_ms: u64,
    payload: JsonValue,
) -> ControlTraceLogEntry {
    ControlTraceLogEntry {
        ts_ms,
        kind: kind.to_string(),
        service: service.map(str::to_string),
        request_id,
        event: event.map(str::to_string),
        payload,
    }
}

pub fn log_retry_trace(mut event: JsonValue) {
    let legacy_enabled = retry_trace_enabled();
    let unified_enabled = control_trace_enabled();
    if !legacy_enabled && !unified_enabled {
        return;
    }

    if let JsonValue::Object(ref mut obj) = event {
        obj.entry("ts_ms".to_string())
            .or_insert_with(|| JsonValue::Number(serde_json::Number::from(now_ms())));
    }

    let ts_ms = json_u64_field(&event, "ts_ms").unwrap_or_else(now_ms);
    let service = json_string_field(&event, "service");
    let request_id = json_u64_field(&event, "request_id");
    let event_name = json_string_field(&event, "event");
    let opt = request_log_options();
    let _guard = match log_lock().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };

    if legacy_enabled {
        let path = retry_trace_path();
        ensure_log_parent(&path);
        if let Ok(line) = serde_json::to_string(&event) {
            let _ = append_json_line(&path, opt, &line);
        }
    }

    append_control_trace_payload(
        opt,
        "retry_trace",
        service.as_deref(),
        request_id,
        event_name.as_deref(),
        ts_ms,
        event,
    );
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
    station_name: &str,
    provider_id: Option<String>,
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

    static DEBUG_SEQ: AtomicU64 = AtomicU64::new(0);
    let mut http_debug_for_main = http_debug;
    let mut http_debug_ref: Option<HttpDebugRef> = None;

    let log_file_path = log_path();
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
            service,
            method,
            path,
            status_code,
            duration_ms,
            ttfb_ms,
            station_name,
            provider_id: provider_id.clone(),
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
        service,
        method,
        path,
        status_code,
        duration_ms,
        ttfb_ms,
        station_name,
        provider_id,
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
mod tests {
    use super::*;

    #[test]
    fn request_log_serializes_request_id_when_present() {
        let value = serde_json::to_value(RequestLog {
            timestamp_ms: 1,
            request_id: Some(42),
            service: "codex",
            method: "POST",
            path: "/v1/responses",
            status_code: 200,
            duration_ms: 123,
            ttfb_ms: Some(10),
            station_name: "right",
            provider_id: Some("right".to_string()),
            upstream_base_url: "https://example.com/v1",
            session_id: Some("sid-1".to_string()),
            cwd: Some("/workdir".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: ServiceTierLog {
                requested: Some("priority".to_string()),
                effective: Some("priority".to_string()),
                actual: Some("priority".to_string()),
            },
            usage: None,
            http_debug: None,
            http_debug_ref: None,
            retry: None,
        })
        .expect("serialize request log");

        assert_eq!(value["request_id"].as_u64(), Some(42));
    }

    #[test]
    fn json_field_helpers_extract_string_and_numeric_values() {
        let event = serde_json::json!({
            "service": "codex",
            "request_id": "42",
            "ts_ms": 99,
        });

        assert_eq!(
            json_string_field(&event, "service").as_deref(),
            Some("codex")
        );
        assert_eq!(json_u64_field(&event, "request_id"), Some(42));
        assert_eq!(json_u64_field(&event, "ts_ms"), Some(99));
    }

    #[test]
    fn make_control_trace_entry_keeps_kind_event_and_request_id() {
        let entry = make_control_trace_entry(
            "retry_trace",
            Some("codex"),
            Some(7),
            Some("attempt_select"),
            123,
            serde_json::json!({
                "event": "attempt_select",
                "service": "codex",
                "request_id": 7,
            }),
        );

        let value = serde_json::to_value(entry).expect("serialize control trace entry");
        assert_eq!(value["kind"].as_str(), Some("retry_trace"));
        assert_eq!(value["event"].as_str(), Some("attempt_select"));
        assert_eq!(value["request_id"].as_u64(), Some(7));
        assert_eq!(value["service"].as_str(), Some("codex"));
        assert_eq!(value["payload"]["event"].as_str(), Some("attempt_select"));
    }
}
