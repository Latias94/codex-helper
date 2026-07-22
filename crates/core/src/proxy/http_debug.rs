use axum::http::HeaderMap;

use crate::logging::{
    AuthResolutionLog, BodyPreview, HeaderEntry, HttpDebugLog, make_body_preview,
};

use super::classify::classify_observed_upstream_response;
use super::headers::header_map_to_entries;

#[derive(Clone)]
pub(super) struct HttpDebugBase {
    pub(super) route_attempt_index: u32,
    pub(super) debug_max_body_bytes: usize,
    pub(super) warn_max_body_bytes: usize,
    pub(super) request_body_len: usize,
    pub(super) upstream_request_body_len: usize,
    pub(super) client_uri: String,
    pub(super) upstream_origin: Option<String>,
    pub(super) upstream_uri: Option<String>,
    pub(super) client_headers: Vec<HeaderEntry>,
    pub(super) upstream_request_headers: Vec<HeaderEntry>,
    pub(super) auth_resolution: Option<AuthResolutionLog>,
    pub(super) client_body_debug: Option<BodyPreview>,
    pub(super) upstream_request_body_debug: Option<BodyPreview>,
    pub(super) client_body_warn: Option<BodyPreview>,
    pub(super) upstream_request_body_warn: Option<BodyPreview>,
}

pub(super) struct HttpDebugResponseParams<'a> {
    pub(super) status_code: u16,
    pub(super) response_headers: &'a HeaderMap,
    pub(super) response_body: &'a [u8],
    pub(super) response_preview_body: Option<&'a [u8]>,
    pub(super) upstream_headers_ms: u64,
    pub(super) upstream_first_chunk_ms: Option<u64>,
    pub(super) upstream_body_read_ms: Option<u64>,
    pub(super) for_warn: bool,
}

pub(super) struct HttpDebugTransportErrorParams<'a> {
    pub(super) response_headers: Option<&'a HeaderMap>,
    pub(super) upstream_headers_ms: Option<u64>,
    pub(super) upstream_body_read_ms: Option<u64>,
    pub(super) error_class: &'a str,
    pub(super) error_hint: &'a str,
    pub(super) upstream_error: String,
    pub(super) for_warn: bool,
}

impl HttpDebugBase {
    fn max_body_bytes(&self, for_warn: bool) -> usize {
        if for_warn {
            self.warn_max_body_bytes
        } else {
            self.debug_max_body_bytes
        }
    }

    fn request_bodies(&self, for_warn: bool) -> (Option<BodyPreview>, Option<BodyPreview>) {
        if for_warn {
            (
                self.client_body_warn.clone(),
                self.upstream_request_body_warn.clone(),
            )
        } else {
            (
                self.client_body_debug.clone(),
                self.upstream_request_body_debug.clone(),
            )
        }
    }

    pub(super) fn response_log(&self, params: HttpDebugResponseParams<'_>) -> Option<HttpDebugLog> {
        let HttpDebugResponseParams {
            status_code,
            response_headers,
            response_body,
            response_preview_body,
            upstream_headers_ms,
            upstream_first_chunk_ms,
            upstream_body_read_ms,
            for_warn,
        } = params;
        let max = self.max_body_bytes(for_warn);
        if max == 0 {
            return None;
        }
        let response_content_type = response_headers
            .get("content-type")
            .and_then(|value| value.to_str().ok());
        let (client_body, upstream_request_body) = self.request_bodies(for_warn);
        let classified =
            classify_observed_upstream_response(status_code, response_headers, response_body);
        let response_preview_body = response_preview_body.unwrap_or(response_body);

        Some(HttpDebugLog {
            route_attempt_index: Some(self.route_attempt_index),
            request_body_len: Some(self.request_body_len),
            upstream_request_body_len: Some(self.upstream_request_body_len),
            upstream_headers_ms: Some(upstream_headers_ms),
            upstream_first_chunk_ms,
            upstream_body_read_ms,
            upstream_error_class: classified.class,
            upstream_error_hint: classified.hint,
            upstream_cf_ray: classified.cf_ray,
            client_uri: self.client_uri.clone(),
            upstream_origin: self.upstream_origin.clone(),
            upstream_uri: self.upstream_uri.clone(),
            client_headers: self.client_headers.clone(),
            upstream_request_headers: self.upstream_request_headers.clone(),
            auth_resolution: self.auth_resolution.clone(),
            client_body,
            upstream_request_body,
            upstream_response_headers: Some(header_map_to_entries(response_headers)),
            upstream_response_body: Some(make_body_preview(
                response_preview_body,
                response_content_type,
                max,
            )),
            upstream_error: None,
        })
    }

    pub(super) fn transport_error_log(
        &self,
        params: HttpDebugTransportErrorParams<'_>,
    ) -> Option<HttpDebugLog> {
        let HttpDebugTransportErrorParams {
            response_headers,
            upstream_headers_ms,
            upstream_body_read_ms,
            error_class,
            error_hint,
            upstream_error,
            for_warn,
        } = params;
        if self.max_body_bytes(for_warn) == 0 {
            return None;
        }
        let (client_body, upstream_request_body) = self.request_bodies(for_warn);

        Some(HttpDebugLog {
            route_attempt_index: Some(self.route_attempt_index),
            request_body_len: Some(self.request_body_len),
            upstream_request_body_len: Some(self.upstream_request_body_len),
            upstream_headers_ms,
            upstream_first_chunk_ms: None,
            upstream_body_read_ms,
            upstream_error_class: Some(error_class.to_string()),
            upstream_error_hint: Some(error_hint.to_string()),
            upstream_cf_ray: response_headers
                .and_then(|headers| headers.get("cf-ray"))
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned),
            client_uri: self.client_uri.clone(),
            upstream_origin: self.upstream_origin.clone(),
            upstream_uri: self.upstream_uri.clone(),
            client_headers: self.client_headers.clone(),
            upstream_request_headers: self.upstream_request_headers.clone(),
            auth_resolution: self.auth_resolution.clone(),
            client_body,
            upstream_request_body,
            upstream_response_headers: response_headers.map(header_map_to_entries),
            upstream_response_body: None,
            upstream_error: Some(upstream_error),
        })
    }
}

pub(super) fn format_reqwest_error_for_retry_chain(error: &reqwest::Error) -> String {
    let mut flags: Vec<&'static str> = Vec::new();
    if error.is_timeout() {
        flags.push("timeout");
    }
    if error.is_connect() {
        flags.push("connect");
    }

    let mut out = "upstream HTTP transport failed".to_string();
    if !flags.is_empty() {
        out.push_str(" (flags: ");
        out.push_str(&flags.join(","));
        out.push(')');
    }
    out
}

fn warn_http_debug_json(http_debug: &HttpDebugLog) -> Option<String> {
    const MAX_CHARS: usize = 2048;

    let mut json = serde_json::to_string(http_debug).ok()?;
    if json.chars().count() > MAX_CHARS {
        json = json.chars().take(MAX_CHARS).collect::<String>() + "...[TRUNCATED_FOR_LOG]";
    }
    Some(json)
}

pub(super) fn warn_http_debug(status_code: u16, http_debug: &HttpDebugLog) {
    let Some(json) = warn_http_debug_json(http_debug) else {
        return;
    };
    tracing::warn!("upstream non-2xx http_debug={json} status_code={status_code}");
}

#[cfg(test)]
mod tests {
    use super::{format_reqwest_error_for_retry_chain, warn_http_debug_json};
    use crate::logging::{HeaderEntry, HttpDebugLog};

    fn make_http_debug_log(value: &str) -> HttpDebugLog {
        HttpDebugLog {
            route_attempt_index: Some(0),
            request_body_len: Some(12),
            upstream_request_body_len: Some(12),
            upstream_headers_ms: Some(4),
            upstream_first_chunk_ms: Some(6),
            upstream_body_read_ms: Some(8),
            upstream_error_class: Some("test_error".to_string()),
            upstream_error_hint: Some(value.to_string()),
            upstream_cf_ray: None,
            client_uri: "/v1/responses".to_string(),
            upstream_origin: Some("https://example.com".to_string()),
            upstream_uri: Some("/v1/responses".to_string()),
            client_headers: vec![HeaderEntry {
                name: "content-type".to_string(),
                value: "application/json".to_string(),
            }],
            upstream_request_headers: vec![HeaderEntry {
                name: "x-test".to_string(),
                value: value.to_string(),
            }],
            auth_resolution: None,
            client_body: None,
            upstream_request_body: None,
            upstream_response_headers: None,
            upstream_response_body: None,
            upstream_error: None,
        }
    }

    #[test]
    fn warn_http_debug_json_truncates_large_payloads() {
        let log = make_http_debug_log(&"x".repeat(4000));

        let json = warn_http_debug_json(&log).expect("json");

        assert!(json.ends_with("...[TRUNCATED_FOR_LOG]"));
        assert!(json.chars().count() > 2048);
    }

    #[test]
    fn warn_http_debug_json_keeps_small_payloads() {
        let log = make_http_debug_log("small");

        let json = warn_http_debug_json(&log).expect("json");

        assert!(json.contains("\"upstream_error_hint\":\"small\""));
        assert!(!json.ends_with("...[TRUNCATED_FOR_LOG]"));
    }

    #[test]
    fn format_reqwest_error_for_retry_chain_sanitizes_output() {
        let poisoned = "https://user:secret@example.test:99999/private/secret-path?token=hidden";
        let error = reqwest::Client::new()
            .get(poisoned)
            .build()
            .expect_err("invalid url should fail");

        let formatted = format_reqwest_error_for_retry_chain(&error);

        assert!(!formatted.is_empty());
        for secret in ["user:secret", "secret-path", "token=hidden", "example.test"] {
            assert!(!formatted.contains(secret));
        }
    }
}
