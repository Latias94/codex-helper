use axum::http::HeaderMap;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const ROUTING_MISMATCH_CAPABILITY_CLASS: &str = "routing_mismatch_capability";
pub(super) const UPSTREAM_RATE_LIMITED_CLASS: &str = "upstream_rate_limited";
pub(super) const UPSTREAM_OVERLOADED_CLASS: &str = "upstream_overloaded";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UpstreamThrottleSignal {
    pub class: &'static str,
    pub hint: &'static str,
    pub retry_after_secs: Option<u64>,
    pub strong: bool,
}

fn header_value_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn header_value_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u64>().ok())
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn looks_like_cloudflare_challenge_html(headers: &HeaderMap, body: &[u8]) -> bool {
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !ct.starts_with("text/html") {
        return false;
    }
    contains_bytes(body, b"__CF$cv$params")
        || contains_bytes(body, b"/cdn-cgi/")
        || contains_bytes(body, b"challenge-platform")
        || contains_bytes(body, b"cf-chl-")
}

fn looks_like_json(headers: &HeaderMap) -> bool {
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    ct.contains("application/json") || ct.contains("+json")
}

fn json_get_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

fn json_get_u64(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(json_value_to_u64)
}

fn json_value_to_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| {
        v.as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<u64>().ok())
    })
}

fn extract_error_message(v: &Value) -> Option<String> {
    if let Some(err) = v.get("error") {
        if let Some(msg) = json_get_str(err, "message") {
            return Some(msg.to_string());
        }
        if let Some(msg) = json_get_str(err, "error") {
            return Some(msg.to_string());
        }
    }
    json_get_str(v, "message").map(|s| s.to_string())
}

fn extract_error_type(v: &Value) -> Option<String> {
    if let Some(err) = v.get("error") {
        if let Some(t) = json_get_str(err, "type") {
            return Some(t.to_string());
        }
        if let Some(t) = json_get_str(err, "code") {
            return Some(t.to_string());
        }
    }

    // Anthropic-style: { "type": "error", "error": { "type": "...", ... } }
    if let Some(t) = json_get_str(v, "type")
        && t == "error"
        && let Some(err) = v.get("error")
        && let Some(et) = json_get_str(err, "type")
    {
        return Some(et.to_string());
    }

    None
}

fn extract_error_code(v: &Value) -> Option<String> {
    if let Some(err) = v.get("error")
        && let Some(code) = json_get_str(err, "code")
    {
        return Some(code.to_string());
    }

    json_get_str(v, "code").map(|s| s.to_string())
}

fn lower_body_text(body: &[u8]) -> String {
    String::from_utf8_lossy(body).to_ascii_lowercase()
}

fn throttle_json_text(v: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(t) = extract_error_type(v) {
        parts.push(t);
    }
    if let Some(code) = extract_error_code(v) {
        parts.push(code);
    }
    if let Some(msg) = extract_error_message(v) {
        parts.push(msg);
    }
    parts.join(" ").to_ascii_lowercase()
}

fn throttle_message_indicates_overloaded(message: &str) -> bool {
    message.contains("overloaded")
        || message.contains("no capacity available")
        || message.contains("no capacity")
        || message.contains("selected model is at capacity")
        || message.contains("model is at capacity")
        || message.contains("capacity exhausted")
        || message.contains("capacity_exhausted")
        || message.contains("model_capacity_exhausted")
        || (message.contains("capacity")
            && (message.contains("at capacity")
                || message.contains("capacity available")
                || message.contains("exhausted")))
}

fn throttle_message_indicates_rate_limited(message: &str) -> bool {
    message.contains("rate_limit_error")
        || message.contains("rate_limit_exceeded")
        || message.contains("rate limit exceeded")
        || message.contains("rate limit")
        || message.contains("too many requests")
        || message.contains("usage_limit_reached")
        || message.contains("quota exhausted")
        || message.contains("quota_exhausted")
        || message.contains("resource exhausted")
        || message.contains("exceeded your account's rate limit")
        || message.contains("rate limited")
}

fn throttle_hint_for_class(class: &'static str) -> &'static str {
    match class {
        UPSTREAM_RATE_LIMITED_CLASS => {
            "检测到速率限制/配额限制信号（429、Retry-After、rate_limit_error、usage_limit_reached 等）；建议等待窗口重置或切换到其他 relay。"
        }
        UPSTREAM_OVERLOADED_CLASS => {
            "检测到容量耗尽/过载信号（503、529、no capacity、selected model is at capacity 等）；建议冷却后重试或切换到其他 relay。"
        }
        _ => "检测到上游暂时不可用信号。",
    }
}

fn retry_after_secs_from_header(headers: &HeaderMap, name: &str) -> Option<u64> {
    header_value_u64(headers, name)
}

fn retry_after_secs_from_json(v: &Value) -> Option<u64> {
    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();

    let retry_after_ms_candidates = [
        json_get_u64(v, "retry_after_ms"),
        json_get_u64(v, "retryAfterMs"),
        v.get("error")
            .and_then(|err| json_get_u64(err, "retry_after_ms")),
        v.get("error")
            .and_then(|err| json_get_u64(err, "retryAfterMs")),
    ];
    if let Some(ms) = retry_after_ms_candidates.into_iter().flatten().next() {
        let secs = ms.div_ceil(1_000);
        if secs > 0 {
            return Some(secs);
        }
    }

    let resets_at_candidates = [
        json_get_u64(v, "resets_at"),
        json_get_u64(v, "resetsAt"),
        v.get("error")
            .and_then(|err| json_get_u64(err, "resets_at")),
        v.get("error").and_then(|err| json_get_u64(err, "resetsAt")),
    ];
    if let Some(resets_at) = resets_at_candidates.into_iter().flatten().next()
        && resets_at > now_secs
    {
        return Some(resets_at - now_secs);
    }

    let reset_secs_candidates = [
        json_get_u64(v, "retry_after"),
        json_get_u64(v, "retryAfter"),
        json_get_u64(v, "resets_in_seconds"),
        json_get_u64(v, "resetsInSeconds"),
        v.get("error")
            .and_then(|err| json_get_u64(err, "retry_after")),
        v.get("error")
            .and_then(|err| json_get_u64(err, "retryAfter")),
        v.get("error")
            .and_then(|err| json_get_u64(err, "resets_in_seconds")),
        v.get("error")
            .and_then(|err| json_get_u64(err, "resetsInSeconds")),
    ];
    reset_secs_candidates
        .into_iter()
        .flatten()
        .next()
        .filter(|value| *value > 0)
}

fn retry_after_secs_from_response(headers: &HeaderMap, body: &[u8]) -> Option<u64> {
    retry_after_secs_from_header(headers, "retry-after-ms")
        .map(|ms| ms.div_ceil(1_000))
        .filter(|value| *value > 0)
        .or_else(|| retry_after_secs_from_header(headers, "retry-after").filter(|value| *value > 0))
        .or_else(|| {
            if body.is_empty() {
                return None;
            }
            serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|v| retry_after_secs_from_json(&v))
        })
}

pub(super) fn classify_upstream_throttle_response(
    status_code: u16,
    headers: &HeaderMap,
    body: &[u8],
) -> Option<UpstreamThrottleSignal> {
    if (200..300).contains(&status_code) {
        return None;
    }

    let lower = lower_body_text(body);
    let json = serde_json::from_slice::<Value>(body).ok();
    let json_text = json.as_ref().map(throttle_json_text);
    let text = json_text.as_deref().unwrap_or(lower.as_str());
    let retry_after_secs = retry_after_secs_from_response(headers, body)
        .or_else(|| json.as_ref().and_then(retry_after_secs_from_json));

    if throttle_message_indicates_overloaded(text) {
        return Some(UpstreamThrottleSignal {
            class: UPSTREAM_OVERLOADED_CLASS,
            hint: throttle_hint_for_class(UPSTREAM_OVERLOADED_CLASS),
            retry_after_secs,
            strong: true,
        });
    }

    if throttle_message_indicates_rate_limited(text) {
        return Some(UpstreamThrottleSignal {
            class: UPSTREAM_RATE_LIMITED_CLASS,
            hint: throttle_hint_for_class(UPSTREAM_RATE_LIMITED_CLASS),
            retry_after_secs,
            strong: true,
        });
    }

    if retry_after_secs.is_some() && matches!(status_code, 429 | 503 | 529) {
        let class = match status_code {
            429 => UPSTREAM_RATE_LIMITED_CLASS,
            503 | 529 => UPSTREAM_OVERLOADED_CLASS,
            _ => UPSTREAM_RATE_LIMITED_CLASS,
        };
        return Some(UpstreamThrottleSignal {
            class,
            hint: throttle_hint_for_class(class),
            retry_after_secs,
            strong: true,
        });
    }

    match status_code {
        429 => Some(UpstreamThrottleSignal {
            class: UPSTREAM_RATE_LIMITED_CLASS,
            hint: throttle_hint_for_class(UPSTREAM_RATE_LIMITED_CLASS),
            retry_after_secs: None,
            strong: true,
        }),
        503 | 529 => Some(UpstreamThrottleSignal {
            class: UPSTREAM_OVERLOADED_CLASS,
            hint: throttle_hint_for_class(UPSTREAM_OVERLOADED_CLASS),
            retry_after_secs: None,
            strong: false,
        }),
        _ => None,
    }
}

pub(super) fn class_is_health_neutral(class: Option<&str>) -> bool {
    matches!(class, Some(ROUTING_MISMATCH_CAPABILITY_CLASS))
}

fn capability_message_indicates_mismatch(message: &str) -> bool {
    let m = message.to_ascii_lowercase();

    let model_mismatch = m.contains("model")
        && (m.contains("not supported")
            || m.contains("unsupported")
            || m.contains("does not support")
            || m.contains("not available")
            || m.contains("unavailable")
            || m.contains("does not exist")
            || m.contains("do not have access")
            || m.contains("no access"));

    let service_tier_mismatch = (m.contains("service_tier") || m.contains("service tier"))
        && (m.contains("not supported")
            || m.contains("unsupported")
            || m.contains("does not support")
            || m.contains("not available")
            || m.contains("unavailable"));

    let reasoning_mismatch = (m.contains("reasoning")
        || m.contains("reasoning_effort")
        || m.contains("reasoning.effort"))
        && (m.contains("not supported")
            || m.contains("unsupported")
            || m.contains("does not support")
            || m.contains("not available")
            || m.contains("unavailable"));

    model_mismatch || service_tier_mismatch || reasoning_mismatch
}

fn capability_type_indicates_mismatch(error_type: &str) -> bool {
    let t = error_type.to_ascii_lowercase();
    t.contains("unsupported_model")
        || t.contains("model_not_found")
        || t.contains("unsupported_value")
        || t.contains("unsupported_parameter")
}

pub(super) fn classify_upstream_response(
    status_code: u16,
    headers: &HeaderMap,
    body: &[u8],
) -> (Option<String>, Option<String>, Option<String>) {
    let cf_ray = header_value_str(headers, "cf-ray");
    let server = header_value_str(headers, "server")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let looks_cf = server.contains("cloudflare") || cf_ray.is_some();

    if looks_cf && status_code == 524 {
        return (
            Some("cloudflare_timeout".to_string()),
            Some(
                "Cloudflare 524：通常表示源站在规定时间内未返回响应；建议检查上游服务耗时、首包是否及时输出（SSE），以及 Cloudflare/WAF 规则。"
                    .to_string(),
            ),
            cf_ray,
        );
    }

    if looks_like_cloudflare_challenge_html(headers, body) {
        return (
            Some("cloudflare_challenge".to_string()),
            Some(
                "检测到 Cloudflare/WAF 拦截页（text/html + cdn-cgi/challenge 标记）；通常不是 API JSON 错误，请检查 WAF 规则、UA/头部、以及是否需要放行该路径。"
                    .to_string(),
            ),
            cf_ray,
        );
    }

    if matches!(status_code, 400 | 404 | 409 | 422) && !body.is_empty() {
        if looks_like_json(headers)
            && let Ok(v) = serde_json::from_slice::<Value>(body)
        {
            let msg = extract_error_message(&v);
            let err_type = extract_error_type(&v);
            if msg
                .as_deref()
                .is_some_and(capability_message_indicates_mismatch)
                || err_type
                    .as_deref()
                    .is_some_and(capability_type_indicates_mismatch)
                    && msg
                        .as_deref()
                        .is_some_and(capability_message_indicates_mismatch)
            {
                return (
                    Some(ROUTING_MISMATCH_CAPABILITY_CLASS.to_string()),
                    Some(
                        "检测到模型/fast/service-tier/reasoning 能力不匹配；这属于路由兼容性问题，不应计入上游健康惩罚。"
                            .to_string(),
                    ),
                    cf_ray,
                );
            }
        } else {
            let text = String::from_utf8_lossy(body);
            if capability_message_indicates_mismatch(text.as_ref()) {
                return (
                    Some(ROUTING_MISMATCH_CAPABILITY_CLASS.to_string()),
                    Some(
                        "检测到模型/fast/service-tier/reasoning 能力不匹配；这属于路由兼容性问题，不应计入上游健康惩罚。"
                            .to_string(),
                    ),
                    cf_ray,
                );
            }
        }
    }

    if let Some(signal) = classify_upstream_throttle_response(status_code, headers, body) {
        return (
            Some(signal.class.to_string()),
            Some(signal.hint.to_string()),
            cf_ray,
        );
    }

    // Be conservative for 4xx classification: we only mark a subset of obvious client-side mistakes
    // as non-retryable. Statuses like 401/403/404 are often provider/configuration-specific and
    // should still be eligible for provider-level failover.
    if matches!(status_code, 400 | 409 | 413 | 415 | 422)
        && looks_like_json(headers)
        && !body.is_empty()
        && let Ok(v) = serde_json::from_slice::<Value>(body)
    {
        if let Some(t) = extract_error_type(&v) {
            let t = t.to_ascii_lowercase();
            let non_retryable_type = t == "invalid_request_error"
                || t == "validation_error"
                || t == "bad_request"
                || t == "context_limit"
                || t == "context_length_exceeded"
                || t == "token_limit"
                || t == "content_filter";
            if non_retryable_type {
                return (
                    Some("client_error_non_retryable".to_string()),
                    Some(
                        "检测到更可能是请求参数/限制类错误（非瞬态）；建议直接修正请求，而不是重试或切换 provider。"
                            .to_string(),
                    ),
                    cf_ray,
                );
            }
        }

        if let Some(msg) = extract_error_message(&v) {
            let m = msg.to_ascii_lowercase();
            let non_retryable = (m.contains("tool_use") && m.contains("must be unique"))
                || m.contains("all messages must have non-empty content")
                || (m.contains("string should match pattern") && m.contains("srvtoolu_"))
                || (m.contains("unexpected") && m.contains("tool_use_id"))
                || (m.contains("json") && (m.contains("parse") || m.contains("invalid")))
                || (m.contains("schema") && m.contains("validation"));

            if non_retryable {
                return (
                    Some("client_error_non_retryable".to_string()),
                    Some(
                        "检测到更可能是请求格式/参数错误（非瞬态）；建议直接修正请求，而不是重试或切换 provider。"
                            .to_string(),
                    ),
                    cf_ray,
                );
            }
        }
    }

    (None, None, cf_ray)
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::http::HeaderValue;

    #[test]
    fn classifies_rate_limited_429_with_retry_after() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("7"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let body = br#"{"error":{"type":"usage_limit_reached","message":"Too many requests","resets_in_seconds":12}}"#;

        let signal =
            classify_upstream_throttle_response(429, &headers, body).expect("rate limited signal");
        assert_eq!(signal.class, UPSTREAM_RATE_LIMITED_CLASS);
        assert!(signal.strong);
        assert_eq!(signal.retry_after_secs, Some(7));
        assert!(signal.hint.contains("速率限制"));

        let (class, hint, _) = classify_upstream_response(429, &headers, body);
        assert_eq!(class.as_deref(), Some(UPSTREAM_RATE_LIMITED_CLASS));
        assert!(
            hint.as_deref()
                .is_some_and(|value| value.contains("速率限制"))
        );
    }

    #[test]
    fn classifies_overloaded_capacity_messages_even_on_400() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let body =
            br#"{"error":{"type":"invalid_request_error","code":"MODEL_CAPACITY_EXHAUSTED","message":"Selected model is at capacity. Please try a different model."}}"#;

        let signal =
            classify_upstream_throttle_response(400, &headers, body).expect("overloaded signal");
        assert_eq!(signal.class, UPSTREAM_OVERLOADED_CLASS);
        assert!(signal.strong);

        let (class, hint, _) = classify_upstream_response(400, &headers, body);
        assert_eq!(class.as_deref(), Some(UPSTREAM_OVERLOADED_CLASS));
        assert!(
            hint.as_deref()
                .is_some_and(|value| value.contains("容量耗尽"))
        );
    }

    #[test]
    fn classifies_529_as_overloaded_without_body_keywords() {
        let headers = HeaderMap::new();
        let body = b"";

        let signal =
            classify_upstream_throttle_response(529, &headers, body).expect("overloaded signal");
        assert_eq!(signal.class, UPSTREAM_OVERLOADED_CLASS);
        assert!(!signal.strong);

        let (class, _, _) = classify_upstream_response(529, &headers, body);
        assert_eq!(class.as_deref(), Some(UPSTREAM_OVERLOADED_CLASS));
    }

    #[test]
    fn retry_after_secs_supports_retry_after_ms_and_resets_in_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after-ms", HeaderValue::from_static("1500"));
        assert_eq!(retry_after_secs_from_response(&headers, b"{}"), Some(2));

        let headers = HeaderMap::new();
        let body = br#"{"error":{"type":"usage_limit_reached","resets_in_seconds":12}}"#;
        assert_eq!(retry_after_secs_from_response(&headers, body), Some(12));
    }

    #[test]
    fn does_not_classify_success_body_as_throttle() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let body = br#"{"message":"docs mention rate limit handling"}"#;

        assert!(classify_upstream_throttle_response(200, &headers, body).is_none());
        let (class, _, _) = classify_upstream_response(200, &headers, body);
        assert_eq!(class, None);
    }
}
