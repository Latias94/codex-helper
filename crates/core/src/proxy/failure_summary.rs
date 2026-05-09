use axum::http::StatusCode;
use regex::Regex;
use std::sync::LazyLock;

use crate::logging::{RetryInfo, RouteAttemptLog};

const MAX_FAILURE_ATTEMPTS_IN_RESPONSE: usize = 6;
const MAX_FAILURE_DETAIL_CHARS: usize = 180;
const MAX_FAILURE_MESSAGE_CHARS: usize = 2400;

pub(super) fn failed_proxy_client_message(
    status: StatusCode,
    message: &str,
    request_id: u64,
    retry: Option<&RetryInfo>,
    route_attempts: &[RouteAttemptLog],
) -> String {
    let attempts = if route_attempts.is_empty() {
        retry
            .map(RetryInfo::route_attempts_or_derived)
            .unwrap_or_default()
    } else {
        route_attempts.to_vec()
    };
    let sanitized_message = sanitize_failure_text(message, MAX_FAILURE_DETAIL_CHARS);

    if attempts.is_empty() {
        return sanitized_message;
    }

    let actual_attempts = attempts
        .iter()
        .filter(|attempt| !attempt.skipped && attempt.decision != "all_upstreams_avoided")
        .count();
    let mut lines = Vec::new();
    let headline = if actual_attempts == 0 {
        "no usable upstream matched the request"
    } else {
        "all upstream attempts failed"
    };
    lines.push(format!(
        "{headline} (request_id={request_id}, status={}, attempts={}, decisions={})",
        status.as_u16(),
        actual_attempts,
        attempts.len()
    ));

    for (visible_index, attempt) in attempts
        .iter()
        .filter(|attempt| attempt.decision != "selected")
        .take(MAX_FAILURE_ATTEMPTS_IN_RESPONSE)
        .enumerate()
    {
        lines.push(format!(
            "{}. {}",
            visible_index + 1,
            route_attempt_client_line(attempt)
        ));
    }

    let omitted = attempts
        .iter()
        .filter(|attempt| attempt.decision != "selected")
        .count()
        .saturating_sub(MAX_FAILURE_ATTEMPTS_IN_RESPONSE);
    if omitted > 0 {
        lines.push(format!(
            "... {omitted} more route decisions omitted; see requests.jsonl retry.route_attempts"
        ));
    }

    if !sanitized_message.is_empty() && sanitized_message != "no upstreams available" {
        lines.push(format!("last_error: {sanitized_message}"));
    }

    truncate_chars(lines.join("\n").trim(), MAX_FAILURE_MESSAGE_CHARS)
}

fn route_attempt_client_line(attempt: &RouteAttemptLog) -> String {
    let mut parts = Vec::new();
    if let Some(provider_id) = clean_optional_component(attempt.provider_id.as_deref()) {
        parts.push(format!("provider={provider_id}"));
    }
    if let Some(station_name) = clean_optional_component(attempt.station_name.as_deref()) {
        parts.push(format!("station={station_name}"));
    }
    parts.push(upstream_label(attempt));
    parts.push(format!("decision={}", attempt.decision));

    if let Some(provider_attempt) = attempt.provider_attempt
        && let Some(provider_max_attempts) = attempt.provider_max_attempts
    {
        parts.push(format!(
            "provider_attempt={provider_attempt}/{provider_max_attempts}"
        ));
    }
    if let Some(upstream_attempt) = attempt.upstream_attempt
        && let Some(upstream_max_attempts) = attempt.upstream_max_attempts
    {
        parts.push(format!(
            "upstream_attempt={upstream_attempt}/{upstream_max_attempts}"
        ));
    }
    if let Some(status_code) = attempt.status_code {
        parts.push(format!("status={status_code}"));
    }
    if let Some(error_class) = clean_optional_component(attempt.error_class.as_deref()) {
        parts.push(format!("class={error_class}"));
    }
    if let Some(reason) = attempt.reason.as_deref() {
        parts.push(format!(
            "reason={}",
            sanitize_failure_text(reason, MAX_FAILURE_DETAIL_CHARS)
        ));
    } else if let Some(cooldown_reason) = attempt.cooldown_reason.as_deref() {
        parts.push(format!(
            "reason={}",
            sanitize_failure_text(cooldown_reason, MAX_FAILURE_DETAIL_CHARS)
        ));
    }
    if let Some(duration_ms) = attempt.duration_ms {
        parts.push(format!("duration_ms={duration_ms}"));
    }
    if let Some(cooldown_secs) = attempt.cooldown_secs {
        parts.push(format!("cooldown_secs={cooldown_secs}"));
    }
    if let Some(model) = clean_optional_component(attempt.model.as_deref()) {
        parts.push(format!("model={model}"));
    }

    truncate_chars(&parts.join(" "), MAX_FAILURE_DETAIL_CHARS * 2)
}

fn upstream_label(attempt: &RouteAttemptLog) -> String {
    let index = attempt
        .upstream_index
        .map(|idx| idx.to_string())
        .unwrap_or_else(|| "?".to_string());
    let Some(base_url) = attempt.upstream_base_url.as_deref() else {
        return format!("upstream[{index}]=unknown");
    };
    let host = reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_string();
            let port = url
                .port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default();
            Some(format!("{host}{port}"))
        })
        .unwrap_or_else(|| sanitize_failure_text(strip_url_query(base_url).as_str(), 80));

    format!("upstream[{index}]={host}")
}

fn clean_optional_component(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "-")
        .map(|value| sanitize_failure_text(value, 80))
}

fn sanitize_failure_text(value: &str, max_chars: usize) -> String {
    let squashed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let without_query = strip_url_queries(squashed.as_str());
    let redacted = redact_sensitive_tokens(without_query.as_str());
    truncate_chars(redacted.trim(), max_chars)
}

fn strip_url_queries(value: &str) -> String {
    static URL_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"https?://[^\s"'<>)]+"#).expect("valid url regex"));

    URL_RE
        .replace_all(value, |captures: &regex::Captures<'_>| {
            strip_url_query(&captures[0])
        })
        .into_owned()
}

fn strip_url_query(value: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(value) else {
        return value
            .split_once('?')
            .map(|(base, _)| base.to_string())
            .unwrap_or_else(|| value.to_string());
    };
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn redact_sensitive_tokens(value: &str) -> String {
    static JSON_SECRET_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)("(?:api[-_]?key|auth[-_]?token|access[-_]?token|token|secret|key)"\s*:\s*")[^"]+(")"#,
        )
        .expect("valid json secret regex")
    });
    static KEY_VALUE_SECRET_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)((?:api[-_]?key|auth[-_]?token|access[-_]?token|token|secret|key)\s*[:=]\s*)["']?[^"',&}\s]+"#,
        )
        .expect("valid key-value secret regex")
    });
    static BEARER_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)bearer\s+[A-Za-z0-9._~+/=-]+"#).expect("valid bearer regex")
    });
    static OPENAI_KEY_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"sk-[A-Za-z0-9_-]{8,}"#).expect("valid key regex"));

    let out = JSON_SECRET_RE
        .replace_all(value, "${1}[REDACTED]${2}")
        .into_owned();
    let out = KEY_VALUE_SECRET_RE
        .replace_all(out.as_str(), "${1}[REDACTED]")
        .into_owned();
    let out = BEARER_RE
        .replace_all(out.as_str(), "Bearer [REDACTED]")
        .into_owned();
    OPENAI_KEY_RE
        .replace_all(out.as_str(), "sk-[REDACTED]")
        .into_owned()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let keep = max_chars.saturating_sub(15);
    let mut out = value.chars().take(keep).collect::<String>();
    out.push_str("...[truncated]");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(decision: &str) -> RouteAttemptLog {
        RouteAttemptLog {
            attempt_index: 0,
            provider_id: Some("alpha".to_string()),
            provider_attempt: Some(1),
            upstream_attempt: Some(1),
            provider_max_attempts: Some(2),
            upstream_max_attempts: Some(1),
            station_name: Some("primary".to_string()),
            upstream_base_url: Some("https://api.example.com/v1?api_key=secret".to_string()),
            upstream_index: Some(0),
            decision: decision.to_string(),
            raw: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn failed_proxy_client_message_summarizes_route_attempts() {
        let mut first = attempt("failed_status");
        first.status_code = Some(429);
        first.error_class = Some("rate_limit".to_string());
        first.duration_ms = Some(42);
        let mut second = attempt("failed_transport");
        second.provider_id = Some("beta".to_string());
        second.station_name = Some("backup".to_string());
        second.upstream_index = Some(1);
        second.reason = Some("operation timed out".to_string());
        second.error_class = Some("upstream_transport_error".to_string());

        let message = failed_proxy_client_message(
            StatusCode::BAD_GATEWAY,
            r#"{"error":"last failed"}"#,
            12,
            None,
            &[first, second],
        );

        assert!(message.contains("all upstream attempts failed"));
        assert!(message.contains("request_id=12"));
        assert!(message.contains("provider=alpha"));
        assert!(message.contains("station=backup"));
        assert!(message.contains("upstream[1]=api.example.com"));
        assert!(message.contains("status=429"));
        assert!(message.contains("class=upstream_transport_error"));
        assert!(message.contains("last_error:"));
    }

    #[test]
    fn failed_proxy_client_message_summarizes_unusable_upstreams() {
        let mut skipped = attempt("skipped_capability_mismatch");
        skipped.skipped = true;
        skipped.reason = Some("unsupported_model".to_string());
        skipped.model = Some("gpt-5".to_string());

        let message = failed_proxy_client_message(
            StatusCode::BAD_GATEWAY,
            "no upstreams available",
            7,
            None,
            &[skipped],
        );

        assert!(message.contains("no usable upstream matched the request"));
        assert!(message.contains("attempts=0"));
        assert!(message.contains("reason=unsupported_model"));
        assert!(!message.contains("last_error:"));
    }

    #[test]
    fn failed_proxy_client_message_redacts_sensitive_values() {
        let mut failed = attempt("failed_transport");
        failed.reason = Some(
            r#"Authorization: Bearer sk-testsecret123 api_key=abc123 token:"hidden""#.to_string(),
        );

        let message = failed_proxy_client_message(
            StatusCode::BAD_GATEWAY,
            r#"{"api_key":"abc123","error":"Bearer sk-testsecret123"}"#,
            3,
            None,
            &[failed],
        );

        assert!(!message.contains("sk-testsecret123"));
        assert!(!message.contains("abc123"));
        assert!(!message.contains("?api_key="));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn failed_proxy_client_message_truncates_long_details() {
        let mut failed = attempt("failed_transport");
        failed.reason = Some("x".repeat(1000));

        let message = failed_proxy_client_message(
            StatusCode::BAD_GATEWAY,
            "y".repeat(1000).as_str(),
            4,
            None,
            &[failed],
        );

        assert!(message.contains("...[truncated]"));
        assert!(message.chars().count() <= MAX_FAILURE_MESSAGE_CHARS);
    }

    #[test]
    fn failed_proxy_client_message_without_attempts_returns_sanitized_error() {
        let message = failed_proxy_client_message(
            StatusCode::BAD_GATEWAY,
            "plain error token=secret",
            1,
            None,
            &[],
        );

        assert_eq!(message, "plain error token=[REDACTED]");
    }
}
