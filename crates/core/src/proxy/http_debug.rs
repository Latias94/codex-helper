use crate::logging::{AuthResolutionLog, BodyPreview, HeaderEntry, HttpDebugLog};

#[derive(Clone)]
pub(super) struct HttpDebugBase {
    pub(super) debug_max_body_bytes: usize,
    pub(super) warn_max_body_bytes: usize,
    #[allow(dead_code)]
    pub(super) request_body_len: usize,
    #[allow(dead_code)]
    pub(super) upstream_request_body_len: usize,
    pub(super) client_uri: String,
    pub(super) upstream_origin: Option<String>,
    pub(super) client_headers: Vec<HeaderEntry>,
    pub(super) upstream_request_headers: Vec<HeaderEntry>,
    pub(super) auth_resolution: Option<AuthResolutionLog>,
    pub(super) client_body_debug: Option<BodyPreview>,
    pub(super) upstream_request_body_debug: Option<BodyPreview>,
    pub(super) client_body_warn: Option<BodyPreview>,
    pub(super) upstream_request_body_warn: Option<BodyPreview>,
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
