use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::Serialize;
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

#[derive(Debug, Serialize)]
pub struct RequestLog<'a> {
    pub timestamp_ms: u64,
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
    pub config_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub upstream_base_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
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

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct RetryInfo {
    pub attempts: u32,
    pub upstream_chain: Vec<String>,
}

#[derive(Debug, Serialize)]
struct HttpDebugLogEntry<'a> {
    pub id: &'a str,
    pub timestamp_ms: u64,
    pub service: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub status_code: u16,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    pub config_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub upstream_base_url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
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

fn log_path() -> PathBuf {
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

pub fn log_retry_trace(mut event: JsonValue) {
    if !retry_trace_enabled() {
        return;
    }

    if let JsonValue::Object(ref mut obj) = event {
        obj.entry("ts_ms".to_string())
            .or_insert_with(|| JsonValue::Number(serde_json::Number::from(now_ms())));
    }

    let opt = request_log_options();
    let path = retry_trace_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let _guard = match log_lock().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };

    if let Ok(line) = serde_json::to_string(&event) {
        rotate_and_prune_if_needed(&path, opt);
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(file, "{}", line);
        }
    }
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
    service: &str,
    method: &str,
    path: &str,
    status_code: u16,
    duration_ms: u64,
    ttfb_ms: Option<u64>,
    config_name: &str,
    provider_id: Option<String>,
    upstream_base_url: &str,
    session_id: Option<String>,
    cwd: Option<String>,
    reasoning_effort: Option<String>,
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
    if let Some(parent) = log_file_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

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
            service,
            method,
            path,
            status_code,
            duration_ms,
            ttfb_ms,
            config_name,
            provider_id: provider_id.clone(),
            upstream_base_url,
            session_id: session_id.clone(),
            cwd: cwd.clone(),
            reasoning_effort: reasoning_effort.clone(),
            usage: usage.clone(),
            retry: retry.clone(),
            http_debug: h,
        };

        let debug_path = debug_log_path();
        if let Some(parent) = debug_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut wrote_debug = false;
        if let Ok(line) = serde_json::to_string(&debug_entry) {
            rotate_and_prune_if_needed(&debug_path, opt);
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&debug_path)
                && writeln!(file, "{}", line).is_ok()
            {
                wrote_debug = true;
            }
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
        service,
        method,
        path,
        status_code,
        duration_ms,
        ttfb_ms,
        config_name,
        provider_id,
        upstream_base_url,
        session_id,
        cwd,
        reasoning_effort,
        usage,
        http_debug: http_debug_for_main,
        http_debug_ref,
        retry,
    };

    rotate_and_prune_if_needed(&log_file_path, opt);
    if let Ok(line) = serde_json::to_string(&entry)
        && let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file_path)
    {
        let _ = writeln!(file, "{}", line);
    }
}
