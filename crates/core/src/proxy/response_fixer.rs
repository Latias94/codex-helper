use std::io::{Cursor, Read};

use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, header};
use flate2::read::GzDecoder;
use serde_json::Value;

const MAX_REPAIRED_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

pub(super) enum CodexCompactSseRepair {
    FinalJson(Bytes),
    UpstreamFailureJson(Bytes),
}

pub(super) struct RemoteCompactionV2ResponseClassification {
    pub(super) downgrade_recommended: bool,
    pub(super) response_shape: &'static str,
}

pub(super) fn maybe_repair_codex_response_body(
    service_name: &str,
    path: &str,
    headers: &HeaderMap,
    body: Bytes,
) -> Bytes {
    if service_name != "codex" || !is_codex_responses_path(path) || looks_like_json(body.as_ref()) {
        return body;
    }

    if response_content_encoding_contains(headers, "gzip") || body.starts_with(&[0x1f, 0x8b]) {
        return decode_gzip_json(body.as_ref())
            .map(Bytes::from)
            .unwrap_or(body);
    }

    body
}

pub(super) fn classify_remote_compaction_v2_response(
    status: StatusCode,
    headers: &HeaderMap,
    body: &Bytes,
) -> RemoteCompactionV2ResponseClassification {
    let values = parse_response_json_values(headers, body.as_ref());
    let compaction_done_items_seen = values
        .iter()
        .map(compaction_output_done_item_count)
        .sum::<usize>();
    let json_compaction_items_seen = values
        .iter()
        .map(json_compaction_output_item_count)
        .sum::<usize>();
    let response_completed_seen = values.iter().any(value_is_response_completed);
    let response_shape = remote_compaction_v2_response_shape(
        status,
        body.as_ref(),
        compaction_done_items_seen,
        json_compaction_items_seen,
        response_completed_seen,
    );
    let valid_v2_stream =
        status.is_success() && compaction_done_items_seen == 1 && response_completed_seen;
    let downgrade_recommended =
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            false
        } else if status.is_success() {
            !valid_v2_stream
        } else {
            is_unsupported_endpoint_status(status)
                || body_mentions_remote_compaction_v2_unsupported(body.as_ref())
        };

    RemoteCompactionV2ResponseClassification {
        downgrade_recommended,
        response_shape,
    }
}

pub(super) fn synthesize_remote_compaction_v2_sse_from_compact_response(
    service_name: &str,
    compact_path: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Option<Bytes> {
    let compact_body =
        match maybe_repair_codex_compact_sse_response(service_name, compact_path, headers, body) {
            Some(CodexCompactSseRepair::FinalJson(body)) => body,
            Some(CodexCompactSseRepair::UpstreamFailureJson(_)) => return None,
            None => body.clone(),
        };
    let value = serde_json::from_slice::<Value>(compact_body.as_ref()).ok()?;
    let response = value.get("response").unwrap_or(&value);
    let response_id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp_compact_downgraded");
    let compaction_items = response
        .get("output")
        .and_then(Value::as_array)?
        .iter()
        .filter(|item| value_is_compaction_item(item))
        .collect::<Vec<_>>();
    let [item] = compaction_items.as_slice() else {
        return None;
    };
    let item_json = serde_json::to_string(item).ok()?;
    let completed_json = serde_json::json!({
        "type": "response.completed",
        "response": {
            "id": response_id,
            "output": [],
        }
    });
    let completed_json = serde_json::to_string(&completed_json).ok()?;
    Some(Bytes::from(format!(
        "event: response.output_item.done\ndata: {{\"type\":\"response.output_item.done\",\"item\":{item_json}}}\n\n\
         event: response.completed\ndata: {completed_json}\n\n"
    )))
}

pub(super) fn maybe_repair_codex_compact_sse_response(
    service_name: &str,
    path: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Option<CodexCompactSseRepair> {
    if service_name != "codex"
        || !path.trim_end_matches('/').ends_with("/responses/compact")
        || looks_like_json(body.as_ref())
        || !(response_content_type_contains(headers, "text/event-stream")
            || looks_like_sse(body.as_ref()))
    {
        return None;
    }

    let text = std::str::from_utf8(body.as_ref()).ok()?;
    let mut output_items = Vec::new();
    let mut added_compaction_item = None::<Value>;
    let mut failed_message = None::<String>;

    for event in parse_sse_json_events(text) {
        let event_type = event.event_type();
        if event_type == Some("response.output_item.done") {
            if let Some(item) = event.value.get("item").filter(|item| item.is_object()) {
                output_items.push(item.clone());
            }
            continue;
        }

        if event_type == Some("response.output_item.added") {
            if added_compaction_item.is_none()
                && let Some(item) = event
                    .value
                    .get("item")
                    .filter(|item| item.is_object() && value_is_compaction_item(item))
            {
                added_compaction_item = Some(item.clone());
            }
            continue;
        }

        if matches!(event_type, Some("response.completed" | "response.done")) {
            let mut response = event.value.get("response")?.clone();
            if !output_items.iter().any(value_is_compaction_item)
                && let Some(item) = added_compaction_item.take()
            {
                output_items.push(item);
            }
            if response_is_missing_output(&response)
                && !output_items.is_empty()
                && let Some(response_object) = response.as_object_mut()
            {
                response_object.insert("output".to_string(), Value::Array(output_items));
            } else if !response_output_has_compaction_item(&response)
                && let Some(item) = output_items
                    .iter()
                    .find(|item| value_is_compaction_item(item))
                && let Some(output) = response.get_mut("output").and_then(Value::as_array_mut)
            {
                output.push(item.clone());
            }
            let body = serde_json::to_vec(&response).ok()?;
            return Some(CodexCompactSseRepair::FinalJson(Bytes::from(body)));
        }

        if event_type == Some("response.failed") && failed_message.is_none() {
            failed_message = Some(extract_sse_failure_message(&event.value));
        }
    }

    failed_message.map(|message| {
        let body = serde_json::json!({
            "error": {
                "type": "upstream_error",
                "message": message,
            }
        });
        CodexCompactSseRepair::UpstreamFailureJson(Bytes::from(
            serde_json::to_vec(&body).expect("serialize compact SSE failure"),
        ))
    })
}

fn is_codex_responses_path(path: &str) -> bool {
    let path = path.trim_end_matches('/');
    path.ends_with("/responses") || path.ends_with("/responses/compact")
}

fn response_content_encoding_contains(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|encoding| encoding.trim().eq_ignore_ascii_case(expected))
}

fn response_content_type_contains(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get_all(header::CONTENT_TYPE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .any(|part| part.trim().eq_ignore_ascii_case(expected))
}

fn decode_gzip_json(body: &[u8]) -> Option<Vec<u8>> {
    let mut limited =
        GzDecoder::new(Cursor::new(body)).take((MAX_REPAIRED_RESPONSE_BYTES + 1) as u64);
    let mut out = Vec::new();
    limited.read_to_end(&mut out).ok()?;
    if out.len() > MAX_REPAIRED_RESPONSE_BYTES || !looks_like_json(&out) {
        return None;
    }
    Some(out)
}

fn looks_like_json(bytes: &[u8]) -> bool {
    let Some(first) = bytes.iter().find(|byte| !byte.is_ascii_whitespace()) else {
        return false;
    };
    matches!(first, b'{' | b'[')
}

fn looks_like_sse(bytes: &[u8]) -> bool {
    let trimmed = bytes.trim_ascii_start();
    trimmed.starts_with(b"data:") || trimmed.starts_with(b"event:") || trimmed.starts_with(b":")
}

struct SseJsonEvent {
    event: Option<String>,
    value: Value,
}

impl SseJsonEvent {
    fn event_type(&self) -> Option<&str> {
        self.value
            .get("type")
            .and_then(Value::as_str)
            .or(self.event.as_deref())
    }
}

fn parse_sse_json_events(text: &str) -> Vec<SseJsonEvent> {
    text.replace("\r\n", "\n")
        .split("\n\n")
        .filter_map(parse_sse_json_event)
        .collect()
}

fn parse_sse_json_event(block: &str) -> Option<SseJsonEvent> {
    let mut event = None::<String>;
    let mut data = String::new();

    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim().to_string());
            continue;
        }
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        let chunk = rest.trim_start();
        if chunk == "[DONE]" {
            return None;
        }
        if !data.is_empty() {
            data.push('\n');
        }
        data.push_str(chunk);
    }

    if data.is_empty() {
        return None;
    }

    serde_json::from_str::<Value>(&data)
        .ok()
        .map(|value| SseJsonEvent { event, value })
}

fn response_is_missing_output(response: &Value) -> bool {
    response
        .get("output")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
}

fn response_output_has_compaction_item(response: &Value) -> bool {
    response
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|output| output.iter().any(value_is_compaction_item))
}

fn extract_sse_failure_message(value: &Value) -> String {
    for path in [
        &["response", "error", "message"][..],
        &["error", "message"][..],
        &["message"][..],
    ] {
        if let Some(message) = get_string_path(value, path).map(str::trim)
            && !message.is_empty()
        {
            return message.to_string();
        }
    }
    "Upstream compact response failed".to_string()
}

fn parse_response_json_values(headers: &HeaderMap, body: &[u8]) -> Vec<Value> {
    if looks_like_json(body) {
        return serde_json::from_slice::<Value>(body)
            .ok()
            .into_iter()
            .collect();
    }

    if response_content_type_contains(headers, "text/event-stream") || looks_like_sse(body) {
        return std::str::from_utf8(body)
            .ok()
            .map(parse_sse_json_events)
            .unwrap_or_default()
            .into_iter()
            .map(|event| event.value)
            .collect();
    }

    Vec::new()
}

fn value_event_type(value: &Value) -> Option<&str> {
    value.get("type").and_then(Value::as_str)
}

fn value_is_response_completed(value: &Value) -> bool {
    value_event_type(value) == Some("response.completed")
}

fn value_is_compaction_item(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|item_type| matches!(item_type, "compaction" | "context_compaction"))
}

fn compaction_output_done_item_count(value: &Value) -> usize {
    if value_event_type(value) != Some("response.output_item.done") {
        return 0;
    }
    value
        .get("item")
        .filter(|item| value_is_compaction_item(item))
        .map(|_| 1)
        .unwrap_or_default()
}

fn json_compaction_output_item_count(value: &Value) -> usize {
    let top_level = value
        .get("output")
        .and_then(Value::as_array)
        .map(|output| {
            output
                .iter()
                .filter(|item| value_is_compaction_item(item))
                .count()
        })
        .unwrap_or_default();
    let response = value
        .get("response")
        .and_then(|response| response.get("output"))
        .and_then(Value::as_array)
        .map(|output| {
            output
                .iter()
                .filter(|item| value_is_compaction_item(item))
                .count()
        })
        .unwrap_or_default();
    top_level + response
}

fn remote_compaction_v2_response_shape(
    status: StatusCode,
    body: &[u8],
    compaction_done_items_seen: usize,
    json_compaction_items_seen: usize,
    response_completed_seen: bool,
) -> &'static str {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        "remote_compaction_v2_error"
    } else if !status.is_success() && is_unsupported_endpoint_status(status) {
        "remote_compaction_v2_unsupported_status"
    } else if !status.is_success() && body_mentions_remote_compaction_v2_unsupported(body) {
        "remote_compaction_v2_unsupported_error"
    } else if !status.is_success() {
        "remote_compaction_v2_error"
    } else if compaction_done_items_seen == 1 && response_completed_seen {
        "remote_compaction_v2_compaction_stream"
    } else if compaction_done_items_seen > 1 {
        "remote_compaction_v2_duplicate_compaction_items"
    } else if compaction_done_items_seen == 1 {
        "remote_compaction_v2_compaction_without_completed"
    } else if json_compaction_items_seen > 0 {
        "remote_compaction_v2_json_compaction_item"
    } else if response_completed_seen {
        "remote_compaction_v2_completed_without_compaction"
    } else {
        "remote_compaction_v2_responses_success"
    }
}

fn is_unsupported_endpoint_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED
    )
}

fn body_mentions_remote_compaction_v2_unsupported(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    let mentions_compaction = text.contains("remote_compaction_v2")
        || text.contains("compaction_trigger")
        || text.contains("compaction")
        || text.contains("compact");
    let mentions_unsupported = text.contains("unsupported")
        || text.contains("not supported")
        || text.contains("not implemented")
        || text.contains("unknown endpoint")
        || text.contains("no route")
        || text.contains("not found");
    mentions_compaction && mentions_unsupported
}

fn get_string_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_str()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    fn gzip_json(body: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(body).expect("gzip write");
        encoder.finish().expect("gzip finish")
    }

    #[test]
    fn response_fixer_decodes_codex_responses_gzip_by_header() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        let body = Bytes::from(gzip_json(br#"{"ok":true}"#));

        let repaired = maybe_repair_codex_response_body("codex", "/v1/responses", &headers, body);

        assert_eq!(repaired.as_ref(), br#"{"ok":true}"#);
    }

    #[test]
    fn response_fixer_decodes_codex_responses_gzip_by_signature() {
        let headers = HeaderMap::new();
        let body = Bytes::from(gzip_json(br#"{"ok":true}"#));

        let repaired = maybe_repair_codex_response_body("codex", "/v1/responses", &headers, body);

        assert_eq!(repaired.as_ref(), br#"{"ok":true}"#);
    }

    #[test]
    fn response_fixer_leaves_non_json_or_non_codex_bodies_untouched() {
        let headers = HeaderMap::new();
        let body = Bytes::from(gzip_json(b"plain text"));

        let repaired =
            maybe_repair_codex_response_body("codex", "/v1/responses", &headers, body.clone());
        assert_eq!(repaired, body);

        let repaired =
            maybe_repair_codex_response_body("claude", "/v1/messages", &headers, body.clone());
        assert_eq!(repaired, body);
    }

    #[test]
    fn classifies_exact_remote_compaction_v2_stream_as_valid() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let body = Bytes::from_static(
            br#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"compaction","encrypted_content":"summary"}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","output":[]}}

"#,
        );

        let classification =
            classify_remote_compaction_v2_response(StatusCode::OK, &headers, &body);

        assert!(!classification.downgrade_recommended);
        assert_eq!(
            classification.response_shape,
            "remote_compaction_v2_compaction_stream"
        );
    }

    #[test]
    fn classifies_successful_non_v2_shape_for_downgrade() {
        let body = Bytes::from_static(
            br#"{"id":"resp_1","output":[{"type":"compaction","encrypted_content":"summary"}]}"#,
        );

        let classification =
            classify_remote_compaction_v2_response(StatusCode::OK, &HeaderMap::new(), &body);

        assert!(classification.downgrade_recommended);
        assert_eq!(
            classification.response_shape,
            "remote_compaction_v2_json_compaction_item"
        );
    }

    #[test]
    fn classifies_explicit_remote_compaction_unsupported_errors_for_downgrade() {
        for (status, body, expected_shape) in [
            (
                StatusCode::NOT_FOUND,
                Bytes::from_static(br#"{"error":"missing"}"#),
                "remote_compaction_v2_unsupported_status",
            ),
            (
                StatusCode::BAD_REQUEST,
                Bytes::from_static(
                    br#"{"error":{"message":"remote compaction is not supported"}}"#,
                ),
                "remote_compaction_v2_unsupported_error",
            ),
        ] {
            let classification =
                classify_remote_compaction_v2_response(status, &HeaderMap::new(), &body);
            assert!(classification.downgrade_recommended);
            assert_eq!(classification.response_shape, expected_shape);
        }
    }

    #[test]
    fn ordinary_remote_compaction_failure_does_not_trigger_protocol_downgrade() {
        let body = Bytes::from_static(br#"{"error":{"message":"upstream timeout"}}"#);

        let classification = classify_remote_compaction_v2_response(
            StatusCode::BAD_GATEWAY,
            &HeaderMap::new(),
            &body,
        );

        assert!(!classification.downgrade_recommended);
        assert_eq!(classification.response_shape, "remote_compaction_v2_error");
    }

    #[test]
    fn remote_compaction_auth_failures_never_trigger_protocol_downgrade() {
        for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
            let body = Bytes::from_static(
                br#"{"error":{"message":"remote compaction is not supported for this credential"}}"#,
            );
            let classification =
                classify_remote_compaction_v2_response(status, &HeaderMap::new(), &body);

            assert!(!classification.downgrade_recommended);
            assert_eq!(classification.response_shape, "remote_compaction_v2_error");
        }
    }

    #[test]
    fn synthesizes_remote_compaction_v2_stream_from_v1_json() {
        let body = Bytes::from_static(
            br#"{"id":"resp_compact_1","output":[{"type":"compaction","encrypted_content":"summary"}]}"#,
        );

        let synthesized = synthesize_remote_compaction_v2_sse_from_compact_response(
            "codex",
            "/v1/responses/compact",
            &HeaderMap::new(),
            &body,
        )
        .expect("synthesize remote compaction v2 SSE");
        let events = parse_sse_json_events(
            std::str::from_utf8(synthesized.as_ref()).expect("synthesized SSE UTF-8"),
        );

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type(), Some("response.output_item.done"));
        assert_eq!(events[0].value["item"]["type"], "compaction");
        assert_eq!(events[0].value["item"]["encrypted_content"], "summary");
        assert_eq!(events[1].event_type(), Some("response.completed"));
        assert_eq!(
            events[1].value["response"]["id"].as_str(),
            Some("resp_compact_1")
        );
        assert_eq!(
            events[1].value["response"]["output"]
                .as_array()
                .map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn refuses_to_synthesize_ambiguous_compaction_output() {
        let body = Bytes::from_static(
            br#"{"output":[{"type":"compaction"},{"type":"context_compaction"}]}"#,
        );

        assert!(
            synthesize_remote_compaction_v2_sse_from_compact_response(
                "codex",
                "/v1/responses/compact",
                &HeaderMap::new(),
                &body,
            )
            .is_none()
        );
    }

    #[test]
    fn response_fixer_extracts_compact_final_response_from_sse() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let body = Bytes::from_static(
            br#"event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"compaction","encrypted_content":"summary"}]}}

data: [DONE]

"#,
        );

        let Some(CodexCompactSseRepair::FinalJson(repaired)) =
            maybe_repair_codex_compact_sse_response(
                "codex",
                "/v1/responses/compact",
                &headers,
                &body,
            )
        else {
            panic!("expected compact SSE final response repair");
        };
        let value: Value = serde_json::from_slice(repaired.as_ref()).expect("json");

        assert_eq!(value["id"].as_str(), Some("resp_1"));
        assert_eq!(value["output"][0]["type"].as_str(), Some("compaction"));
    }

    #[test]
    fn response_fixer_rebuilds_compact_output_from_done_items_when_final_output_is_empty() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let body = Bytes::from_static(
            br#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"compaction","encrypted_content":"summary-from-item"}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","output":[]}}

"#,
        );

        let Some(CodexCompactSseRepair::FinalJson(repaired)) =
            maybe_repair_codex_compact_sse_response(
                "codex",
                "/v1/responses/compact",
                &headers,
                &body,
            )
        else {
            panic!("expected compact SSE final response repair");
        };
        let value: Value = serde_json::from_slice(repaired.as_ref()).expect("json");

        assert_eq!(value["id"].as_str(), Some("resp_1"));
        assert_eq!(value["output"][0]["type"].as_str(), Some("compaction"));
        assert_eq!(
            value["output"][0]["encrypted_content"].as_str(),
            Some("summary-from-item")
        );
    }

    #[test]
    fn response_fixer_supplements_missing_compaction_when_final_output_is_nonempty() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let body = Bytes::from_static(
            br#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"compaction","encrypted_content":"summary-from-done"}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"message","id":"msg_1"}]}}

"#,
        );

        let Some(CodexCompactSseRepair::FinalJson(repaired)) =
            maybe_repair_codex_compact_sse_response(
                "codex",
                "/v1/responses/compact",
                &headers,
                &body,
            )
        else {
            panic!("expected compact SSE final response repair");
        };
        let value: Value = serde_json::from_slice(repaired.as_ref()).expect("json");

        assert_eq!(value["output"].as_array().map(Vec::len), Some(2));
        assert_eq!(value["output"][0]["type"].as_str(), Some("message"));
        assert_eq!(value["output"][1]["type"].as_str(), Some("compaction"));
        assert_eq!(
            value["output"][1]["encrypted_content"].as_str(),
            Some("summary-from-done")
        );
    }

    #[test]
    fn response_fixer_uses_added_compaction_only_when_done_has_none() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let body = Bytes::from_static(
            br#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"message","id":"msg_1"}}

event: response.output_item.added
data: {"type":"response.output_item.added","item":{"type":"compaction","encrypted_content":"summary-from-added"}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"message","id":"msg_1"}]}}

"#,
        );

        let Some(CodexCompactSseRepair::FinalJson(repaired)) =
            maybe_repair_codex_compact_sse_response(
                "codex",
                "/v1/responses/compact",
                &headers,
                &body,
            )
        else {
            panic!("expected compact SSE final response repair");
        };
        let value: Value = serde_json::from_slice(repaired.as_ref()).expect("json");

        assert_eq!(value["output"].as_array().map(Vec::len), Some(2));
        assert_eq!(value["output"][1]["type"].as_str(), Some("compaction"));
        assert_eq!(
            value["output"][1]["encrypted_content"].as_str(),
            Some("summary-from-added")
        );
    }

    #[test]
    fn response_fixer_does_not_duplicate_existing_compaction_output() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let body = Bytes::from_static(
            br#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"compaction","encrypted_content":"event-summary"}}

event: response.completed
data: {"type":"response.completed","response":{"id":"resp_1","output":[{"type":"compaction","encrypted_content":"final-summary"}]}}

"#,
        );

        let Some(CodexCompactSseRepair::FinalJson(repaired)) =
            maybe_repair_codex_compact_sse_response(
                "codex",
                "/v1/responses/compact",
                &headers,
                &body,
            )
        else {
            panic!("expected compact SSE final response repair");
        };
        let value: Value = serde_json::from_slice(repaired.as_ref()).expect("json");

        assert_eq!(value["output"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            value["output"][0]["encrypted_content"].as_str(),
            Some("final-summary")
        );
    }

    #[test]
    fn response_fixer_extracts_compact_failure_from_sse() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let body = Bytes::from_static(
            br#"event: response.failed
data: {"type":"response.failed","response":{"error":{"message":"compact unavailable"}}}

"#,
        );

        let Some(CodexCompactSseRepair::UpstreamFailureJson(repaired)) =
            maybe_repair_codex_compact_sse_response(
                "codex",
                "/v1/responses/compact",
                &headers,
                &body,
            )
        else {
            panic!("expected compact SSE failure repair");
        };
        let value: Value = serde_json::from_slice(repaired.as_ref()).expect("json");

        assert_eq!(
            value["error"]["message"].as_str(),
            Some("compact unavailable")
        );
    }
}
