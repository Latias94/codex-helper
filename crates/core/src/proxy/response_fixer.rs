use std::io::{Cursor, Read};

use axum::body::Bytes;
use axum::http::{HeaderMap, header};
use flate2::read::GzDecoder;
use serde_json::Value;

const MAX_REPAIRED_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

pub(super) enum CodexCompactSseRepair {
    FinalJson(Bytes),
    UpstreamFailureJson(Bytes),
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

fn value_is_compaction_item(value: &Value) -> bool {
    value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|item_type| matches!(item_type, "compaction" | "context_compaction"))
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

    use axum::http::{HeaderMap, HeaderValue, header};
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
