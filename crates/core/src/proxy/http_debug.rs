use axum::http::HeaderMap;
use zeroize::Zeroizing;

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
    pub(super) body_redactions: Vec<Zeroizing<Vec<u8>>>,
    pub(super) auth_resolution: Option<AuthResolutionLog>,
    pub(super) client_body_debug: Option<BodyPreview>,
    pub(super) upstream_request_body_debug: Option<BodyPreview>,
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
            (None, None)
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
            upstream_response_headers: Some(redacted_response_headers(
                response_headers,
                &self.body_redactions,
            )),
            upstream_response_body: (!for_warn).then(|| {
                make_redacted_body_preview(
                    response_preview_body,
                    response_content_type,
                    max,
                    &self.body_redactions,
                )
            }),
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
            upstream_response_headers: response_headers
                .map(|headers| redacted_response_headers(headers, &self.body_redactions)),
            upstream_response_body: None,
            upstream_error: Some(upstream_error),
        })
    }
}

fn make_redacted_body_preview(
    bytes: &[u8],
    content_type: Option<&str>,
    max: usize,
    redactions: &[Zeroizing<Vec<u8>>],
) -> BodyPreview {
    let original_len = bytes.len();
    let take = original_len.min(max);
    let mut preview = bytes[..take].to_vec();
    redact_known_values(&mut preview, redactions);
    let mut body = make_body_preview(&preview, content_type, take);
    body.original_len = original_len;
    body.truncated = original_len > take;
    body
}

fn redacted_response_headers(
    headers: &HeaderMap,
    redactions: &[Zeroizing<Vec<u8>>],
) -> Vec<HeaderEntry> {
    header_map_to_entries(headers)
        .into_iter()
        .map(|mut entry| {
            let mut value = Zeroizing::new(entry.value.into_bytes());
            redact_known_values(value.as_mut_slice(), redactions);
            entry.value = String::from_utf8_lossy(value.as_slice()).into_owned();
            entry
        })
        .collect()
}

fn redact_known_values(bytes: &mut [u8], redactions: &[Zeroizing<Vec<u8>>]) {
    for value in redactions {
        redact_body_value(bytes, value.as_slice());
    }
}

fn redact_body_value(bytes: &mut [u8], value: &[u8]) {
    if value.is_empty() {
        return;
    }
    for start in 0..=bytes.len().saturating_sub(value.len()) {
        if bytes[start..].starts_with(value) {
            bytes[start..start + value.len()].fill(b'*');
        }
    }
    redact_partial_body_value(bytes, value, true);
    redact_partial_body_value(bytes, value, false);
}

fn redact_partial_body_value(bytes: &mut [u8], value: &[u8], leading: bool) {
    const MIN_PARTIAL_MATCH_BYTES: usize = 4;

    let maximum = value.len().min(bytes.len()).saturating_sub(1);
    for length in (MIN_PARTIAL_MATCH_BYTES..=maximum).rev() {
        let (preview, secret) = if leading {
            (&bytes[..length], &value[value.len() - length..])
        } else {
            (&bytes[bytes.len() - length..], &value[..length])
        };
        if preview == secret {
            if leading {
                bytes[..length].fill(b'*');
            } else {
                let start = bytes.len() - length;
                bytes[start..].fill(b'*');
            }
            return;
        }
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

    // Warnings are enabled by default. Never permit a caller to turn them into a
    // body sink by accidentally passing a full debug record.
    let mut warning = http_debug.clone();
    warning.client_body = None;
    warning.upstream_request_body = None;
    warning.upstream_response_body = None;

    let mut json = serde_json::to_string(&warning).ok()?;
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
    use axum::http::{HeaderMap, HeaderValue};
    use zeroize::Zeroizing;

    use super::{
        HttpDebugBase, HttpDebugResponseParams, HttpDebugTransportErrorParams,
        format_reqwest_error_for_retry_chain, warn_http_debug_json,
    };
    use crate::logging::{HeaderEntry, HttpDebugLog, make_body_preview};

    fn debug_base(redactions: Vec<Zeroizing<Vec<u8>>>) -> HttpDebugBase {
        let request_body = make_body_preview(b"request-body", Some("text/plain"), 1024);
        HttpDebugBase {
            route_attempt_index: 0,
            debug_max_body_bytes: 1024,
            warn_max_body_bytes: 1024,
            request_body_len: request_body.original_len,
            upstream_request_body_len: request_body.original_len,
            client_uri: "/v1/responses".to_string(),
            upstream_origin: Some("https://example.com".to_string()),
            upstream_uri: Some("/v1/responses".to_string()),
            client_headers: Vec::new(),
            upstream_request_headers: Vec::new(),
            body_redactions: redactions,
            auth_resolution: None,
            client_body_debug: Some(request_body.clone()),
            upstream_request_body_debug: Some(request_body.clone()),
        }
    }

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

    #[test]
    fn warning_logs_omit_request_and_response_bodies() {
        let secret = "warning-header-secret-2cb1";
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-upstream-credential-echo",
            format!("Bearer {secret}")
                .parse()
                .expect("valid header value"),
        );
        let log = debug_base(vec![
            Zeroizing::new(format!("Bearer {secret}").into_bytes()),
            Zeroizing::new(secret.as_bytes().to_vec()),
        ])
        .response_log(HttpDebugResponseParams {
            status_code: 500,
            response_headers: &headers,
            response_body: format!("upstream response body {secret}").as_bytes(),
            response_preview_body: None,
            upstream_headers_ms: 10,
            upstream_first_chunk_ms: None,
            upstream_body_read_ms: Some(12),
            for_warn: true,
        })
        .expect("warning log");

        assert!(log.client_body.is_none());
        assert!(log.upstream_request_body.is_none());
        assert!(log.upstream_response_body.is_none());

        let warning_json = warn_http_debug_json(&log).expect("warning JSON");
        assert!(!warning_json.contains(secret));
    }

    #[test]
    fn warning_sink_strips_body_fields_from_full_debug_records() {
        let secret = "warning-body-secret-b5f7";
        let body = make_body_preview(secret.as_bytes(), Some("text/plain"), 1024);
        let mut log = make_http_debug_log("small");
        log.client_body = Some(body.clone());
        log.upstream_request_body = Some(body.clone());
        log.upstream_response_body = Some(body);

        let warning_json = warn_http_debug_json(&log).expect("warning JSON");

        assert!(!warning_json.contains(secret));
        assert!(!warning_json.contains("\"client_body\":"));
        assert!(!warning_json.contains("\"upstream_request_body\":"));
        assert!(!warning_json.contains("\"upstream_response_body\":"));
    }

    #[test]
    fn transport_warning_redacts_known_credentials_in_custom_response_headers() {
        let secret = "transport-header-secret-7d3c";
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-upstream-credential-echo",
            secret.parse().expect("valid header value"),
        );
        let log = debug_base(vec![Zeroizing::new(secret.as_bytes().to_vec())])
            .transport_error_log(HttpDebugTransportErrorParams {
                response_headers: Some(&headers),
                upstream_headers_ms: Some(4),
                upstream_body_read_ms: Some(8),
                error_class: "upstream_body_read_error",
                error_hint: "upstream body read failed",
                upstream_error: "upstream HTTP transport failed".to_string(),
                for_warn: true,
            })
            .expect("warning log");

        let warning_json = warn_http_debug_json(&log).expect("warning JSON");
        assert!(!warning_json.contains(secret));
    }

    #[test]
    fn debug_response_redacts_known_credentials_in_body_and_custom_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let secret = "native-secret-93ac";
        headers.insert(
            "x-upstream-credential-echo",
            format!("Bearer {secret}")
                .parse()
                .expect("valid header value"),
        );
        let response = format!(r#"{{"authorization":"Bearer {secret}","api_key":"{secret}"}}"#);
        let log = debug_base(vec![
            Zeroizing::new(format!("Bearer {secret}").into_bytes()),
            Zeroizing::new(secret.as_bytes().to_vec()),
        ])
        .response_log(HttpDebugResponseParams {
            status_code: 500,
            response_headers: &headers,
            response_body: response.as_bytes(),
            response_preview_body: None,
            upstream_headers_ms: 10,
            upstream_first_chunk_ms: None,
            upstream_body_read_ms: Some(12),
            for_warn: false,
        })
        .expect("debug log");

        let body = log
            .upstream_response_body
            .as_ref()
            .expect("debug response body")
            .data
            .as_str();
        assert!(!body.contains(secret));
        assert!(body.contains("***"));

        let debug_json = serde_json::to_string(&log).expect("debug JSON");
        assert!(!debug_json.contains(secret));
    }
}
