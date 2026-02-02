use axum::http::HeaderMap;
use serde_json::Value;

fn header_value_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
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
