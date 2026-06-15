use axum::http::HeaderMap;
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClassifiedUpstreamResponse {
    pub class: Option<String>,
    pub hint: Option<String>,
    pub cf_ray: Option<String>,
    pub throttle_signal: Option<UpstreamThrottleSignal>,
}

impl ClassifiedUpstreamResponse {
    pub fn retry_after_secs(&self) -> Option<u64> {
        self.throttle_signal
            .as_ref()
            .and_then(|signal| signal.retry_after_secs)
    }
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
        if let Some(msg) = err.as_str() {
            return Some(msg.to_string());
        }
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
        if let Some(t) = json_get_str(err, "status") {
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

    json_get_str(v, "status").map(|s| s.to_string())
}

fn lower_body_text(body: &[u8]) -> String {
    String::from_utf8_lossy(body).to_ascii_lowercase()
}

fn push_json_scalar_text(parts: &mut Vec<String>, value: &Value) {
    match value {
        Value::String(s) if !s.trim().is_empty() => parts.push(s.to_string()),
        Value::Number(n) => parts.push(n.to_string()),
        _ => {}
    }
}

fn push_json_field_text(parts: &mut Vec<String>, v: &Value, key: &str) {
    if let Some(value) = v.get(key) {
        push_json_scalar_text(parts, value);
    }
}

fn collect_throttle_json_text_parts(v: &Value, parts: &mut Vec<String>) {
    for key in [
        "@type",
        "type",
        "code",
        "status",
        "reason",
        "message",
        "error",
        "retryDelay",
        "retry_delay",
        "quotaResetDelay",
        "quota_reset_delay",
        "reset_seconds",
        "resetSeconds",
    ] {
        push_json_field_text(parts, v, key);
    }

    if let Some(metadata) = v.get("metadata").and_then(|value| value.as_object()) {
        for (key, value) in metadata {
            parts.push(key.to_string());
            push_json_scalar_text(parts, value);
        }
    }

    if let Some(details) = v.get("details").and_then(|value| value.as_array()) {
        for detail in details {
            collect_throttle_json_text_parts(detail, parts);
        }
    }
}

fn throttle_json_text(v: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(err) = v.get("error") {
        if let Some(message) = err.as_str() {
            parts.push(message.to_string());
        } else {
            collect_throttle_json_text_parts(err, &mut parts);
        }
    }
    collect_throttle_json_text_parts(v, &mut parts);
    parts.join(" ").to_ascii_lowercase()
}

fn throttle_message_indicates_overloaded(message: &str) -> bool {
    message.contains("overloaded")
        || message.contains("overloaded_error")
        || message.contains("no capacity available")
        || message.contains("no capacity")
        || message.contains("no available channel")
        || message.contains("no channel available")
        || message.contains("get channel failed")
        || message.contains("selected model is at capacity")
        || message.contains("model is at capacity")
        || message.contains("capacity exhausted")
        || message.contains("capacity_exhausted")
        || message.contains("model_capacity_exhausted")
        || message.contains("concurrency limit")
        || message.contains("concurrency_limit")
        || message.contains("too many pending requests")
        || message.contains("pending requests")
        || message.contains("connection limit")
        || message.contains("websocket_connection_limit_reached")
        || message.contains("conn_queue_full")
        || message.contains("queue full")
        || message.contains("maximum concurrent")
        || message.contains("max concurrent")
        || message.contains("too many concurrent")
        || message.contains("并发")
        || message.contains("排队")
        || (message.contains("capacity")
            && (message.contains("at capacity")
                || message.contains("capacity available")
                || message.contains("exhausted")))
}

fn throttle_message_indicates_rate_limited(message: &str) -> bool {
    message.contains("rate_limit_error")
        || message.contains("rate_limit_exceeded")
        || message.contains("rate_limit_reached")
        || message.contains("rate_limit_total_reached")
        || message.contains("rate limit exceeded")
        || message.contains("rate limit reached")
        || message.contains("rate limit")
        || message.contains("rate_limited")
        || message.contains("too many requests")
        || message.contains("usage_limit_reached")
        || message.contains("usage limit")
        || message.contains("usage_limit")
        || message.contains("insufficient_quota")
        || message.contains("insufficient_user_quota")
        || message.contains("insufficient user quota")
        || message.contains("pre_consume_token_quota_failed")
        || message.contains("quota exceeded")
        || message.contains("quota_exceeded")
        || message.contains("quota exhausted")
        || message.contains("quota_exhausted")
        || message.contains("resource exhausted")
        || message.contains("resource_exhausted")
        || message.contains("exceeded your account's rate limit")
        || message.contains("rate limited")
        || message.contains("billing_error")
        || message.contains("insufficient balance")
        || message.contains("billing issue")
        || message.contains("model_cooldown")
        || message.contains("请求数限制")
        || message.contains("总请求数限制")
        || message.contains("额度不足")
        || message.contains("余额不足")
        || message.contains("预扣费额度失败")
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

fn duration_to_secs_ceil(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(if duration.subsec_nanos() > 0 { 1 } else { 0 })
}

fn parse_decimal_seconds_ceil(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let (whole, frac) = value.split_once('.').unwrap_or((value, ""));
    let mut secs = if whole.is_empty() {
        0
    } else {
        whole.parse::<u64>().ok()?
    };
    if frac.chars().any(|ch| ch != '0') {
        secs = secs.saturating_add(1);
    }
    (secs > 0).then_some(secs)
}

fn parse_duration_secs_text(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(seconds) = value.parse::<u64>() {
        return (seconds > 0).then_some(seconds);
    }

    let lower = value.to_ascii_lowercase();
    if let Some(ms) = lower.strip_suffix("ms") {
        let millis = ms.trim().parse::<u64>().ok()?;
        let secs = millis.div_ceil(1_000);
        return (secs > 0).then_some(secs);
    }
    if let Some(seconds) = lower.strip_suffix('s') {
        let seconds = seconds.trim();
        let seconds = seconds.strip_prefix("pt").unwrap_or(seconds);
        return parse_decimal_seconds_ceil(seconds);
    }
    None
}

fn json_value_to_duration_secs(v: &Value) -> Option<u64> {
    json_value_to_u64(v)
        .filter(|value| *value > 0)
        .or_else(|| v.as_str().and_then(parse_duration_secs_text))
}

fn json_get_duration_secs(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(json_value_to_duration_secs)
}

fn retry_after_secs_from_header(headers: &HeaderMap, name: &str) -> Option<u64> {
    let raw = headers.get(name)?.to_str().ok()?.trim();
    if raw.is_empty() {
        return None;
    }
    parse_duration_secs_text(raw).or_else(|| {
        httpdate::parse_http_date(raw)
            .ok()
            .and_then(|when| when.duration_since(SystemTime::now()).ok())
            .map(duration_to_secs_ceil)
            .filter(|value| *value > 0)
    })
}

fn retry_after_secs_from_details(v: &Value) -> Option<u64> {
    for key in [
        "retryDelay",
        "retry_delay",
        "quotaResetDelay",
        "quota_reset_delay",
    ] {
        if let Some(secs) = json_get_duration_secs(v, key) {
            return Some(secs);
        }
    }
    if let Some(metadata) = v.get("metadata").and_then(|value| value.as_object()) {
        for key in [
            "retryDelay",
            "retry_delay",
            "quotaResetDelay",
            "quota_reset_delay",
        ] {
            if let Some(secs) = metadata.get(key).and_then(json_value_to_duration_secs) {
                return Some(secs);
            }
        }
    }
    if let Some(details) = v.get("details").and_then(|value| value.as_array()) {
        for detail in details {
            if let Some(secs) = retry_after_secs_from_details(detail) {
                return Some(secs);
            }
        }
    }
    None
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
        json_get_u64(v, "reset_seconds"),
        json_get_u64(v, "resetSeconds"),
        v.get("error")
            .and_then(|err| json_get_u64(err, "retry_after")),
        v.get("error")
            .and_then(|err| json_get_u64(err, "retryAfter")),
        v.get("error")
            .and_then(|err| json_get_u64(err, "resets_in_seconds")),
        v.get("error")
            .and_then(|err| json_get_u64(err, "resetsInSeconds")),
        v.get("error")
            .and_then(|err| json_get_u64(err, "reset_seconds")),
        v.get("error")
            .and_then(|err| json_get_u64(err, "resetSeconds")),
    ];
    reset_secs_candidates
        .into_iter()
        .flatten()
        .next()
        .filter(|value| *value > 0)
        .or_else(|| retry_after_secs_from_details(v))
        .or_else(|| v.get("error").and_then(retry_after_secs_from_details))
}

fn retry_after_secs_from_response(headers: &HeaderMap, body: &[u8]) -> Option<u64> {
    header_value_u64(headers, "retry-after-ms")
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

pub(super) fn classify_observed_upstream_response(
    status_code: u16,
    headers: &HeaderMap,
    body: &[u8],
) -> ClassifiedUpstreamResponse {
    let throttle_signal = classify_upstream_throttle_response(status_code, headers, body);
    let (class, hint, cf_ray) = classify_upstream_response(status_code, headers, body);
    ClassifiedUpstreamResponse {
        class,
        hint,
        cf_ray,
        throttle_signal,
    }
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
    fn classifies_sub2api_flat_error_string_rate_limit() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let body =
            br#"{"error":"rate limit exceeded","message":"Too many requests, please try again later"}"#;

        let signal =
            classify_upstream_throttle_response(429, &headers, body).expect("rate limited signal");
        assert_eq!(signal.class, UPSTREAM_RATE_LIMITED_CLASS);
        assert!(signal.strong);

        let classified = classify_observed_upstream_response(429, &headers, body);
        assert_eq!(
            classified.class.as_deref(),
            Some(UPSTREAM_RATE_LIMITED_CLASS)
        );
        assert_eq!(classified.retry_after_secs(), None);
    }

    #[test]
    fn classifies_quota_and_billing_errors_as_rate_limited() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let body = r#"{"error":{"type":"new_api_error","code":"insufficient_user_quota","message":"用户额度不足，预扣费额度失败"}}"#
            .as_bytes();

        let signal = classify_upstream_throttle_response(403, &headers, body)
            .expect("quota exhausted signal");
        assert_eq!(signal.class, UPSTREAM_RATE_LIMITED_CLASS);
        assert!(signal.strong);

        let body = br#"{"error":{"type":"billing_error","code":"insufficient_quota","message":"insufficient balance or billing issue"}}"#;
        let signal = classify_upstream_throttle_response(402, &headers, body)
            .expect("billing exhausted signal");
        assert_eq!(signal.class, UPSTREAM_RATE_LIMITED_CLASS);
    }

    #[test]
    fn classifies_concurrency_and_pending_queue_as_overloaded() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let body = br#"{"error":{"type":"rate_limit_error","code":"websocket_connection_limit_reached","message":"Concurrency limit exceeded for websocket, please retry later"}}"#;

        let signal =
            classify_upstream_throttle_response(429, &headers, body).expect("overloaded signal");
        assert_eq!(signal.class, UPSTREAM_OVERLOADED_CLASS);
        assert!(signal.strong);

        let body = br#"{"error":{"message":"Too many pending requests for this account"}}"#;
        let signal =
            classify_upstream_throttle_response(429, &headers, body).expect("overloaded signal");
        assert_eq!(signal.class, UPSTREAM_OVERLOADED_CLASS);
    }

    #[test]
    fn classifies_google_rpc_retry_info_and_resource_exhausted() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let body = br#"{
            "error": {
                "code": 429,
                "status": "RESOURCE_EXHAUSTED",
                "message": "Quota exhausted",
                "details": [
                    {"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"3.200s"},
                    {"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"QUOTA_EXHAUSTED","metadata":{"quotaResetDelay":"4s"}}
                ]
            }
        }"#;

        let signal =
            classify_upstream_throttle_response(429, &headers, body).expect("rate limited signal");
        assert_eq!(signal.class, UPSTREAM_RATE_LIMITED_CLASS);
        assert_eq!(signal.retry_after_secs, Some(4));

        let body = br#"{
            "error": {
                "code": 429,
                "status": "RESOURCE_EXHAUSTED",
                "message": "Selected model is at capacity",
                "details": [{"reason":"MODEL_CAPACITY_EXHAUSTED"}]
            }
        }"#;

        let signal =
            classify_upstream_throttle_response(429, &headers, body).expect("overloaded signal");
        assert_eq!(signal.class, UPSTREAM_OVERLOADED_CLASS);
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
    fn retry_after_secs_supports_http_date_header() {
        let mut headers = HeaderMap::new();
        let when = SystemTime::now() + Duration::from_secs(3);
        let http_date = httpdate::fmt_http_date(when);
        headers.insert(
            "retry-after",
            HeaderValue::from_str(&http_date).expect("valid http date header"),
        );

        let secs = retry_after_secs_from_response(&headers, b"{}")
            .expect("http-date retry-after should parse");
        assert!(
            (1..=4).contains(&secs),
            "unexpected retry-after secs: {secs}"
        );
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
