use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageMetrics {
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
}

impl UsageMetrics {
    pub fn add_assign(&mut self, other: &UsageMetrics) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }
}

fn to_i64(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n.as_i64().unwrap_or(0),
        Value::String(s) => s.parse::<f64>().ok().map(|f| f as i64).unwrap_or(0),
        _ => 0,
    }
}

fn extract_usage_obj(payload: &Value) -> Option<&Value> {
    if let Some(u) = payload.get("usage") {
        return Some(u);
    }
    if let Some(resp) = payload.get("response")
        && let Some(u) = resp.get("usage")
    {
        return Some(u);
    }
    None
}

fn usage_from_value(usage_obj: &Value) -> Option<UsageMetrics> {
    let mut m = UsageMetrics::default();
    let mut recognized = false;

    if let Some(v) = usage_obj.get("input_tokens") {
        m.input_tokens = to_i64(v);
        recognized = true;
    }
    if let Some(v) = usage_obj.get("output_tokens") {
        m.output_tokens = to_i64(v);
        recognized = true;
    }
    if let Some(v) = usage_obj.get("total_tokens") {
        m.total_tokens = to_i64(v);
        recognized = true;
    }

    // OpenAI Chat Completions compatibility (`prompt_tokens` / `completion_tokens`).
    if let Some(v) = usage_obj.get("prompt_tokens") {
        m.input_tokens = to_i64(v);
        recognized = true;
    }
    if let Some(v) = usage_obj.get("completion_tokens") {
        m.output_tokens = to_i64(v);
        recognized = true;
    }

    // Some providers may expose reasoning tokens directly.
    if let Some(v) = usage_obj.get("reasoning_tokens") {
        m.reasoning_tokens = to_i64(v);
        recognized = true;
    }

    if let Some(details) = usage_obj
        .get("output_tokens_details")
        .and_then(|v| v.as_object())
        && let Some(v) = details.get("reasoning_tokens")
    {
        m.reasoning_tokens = to_i64(v);
        recognized = true;
    }
    if let Some(details) = usage_obj
        .get("completion_tokens_details")
        .and_then(|v| v.as_object())
        && let Some(v) = details.get("reasoning_tokens")
    {
        m.reasoning_tokens = to_i64(v);
        recognized = true;
    }

    // If total isn't provided, derive it from input/output when possible.
    if usage_obj.get("total_tokens").is_none() {
        m.total_tokens = m.input_tokens.saturating_add(m.output_tokens);
    }

    if !recognized {
        return None;
    }
    Some(m)
}

pub fn extract_usage_from_bytes(data: &[u8]) -> Option<UsageMetrics> {
    let text = std::str::from_utf8(data).ok()?.trim();
    if text.is_empty() {
        return None;
    }
    let json: Value = serde_json::from_str(text).ok()?;
    let usage_obj = extract_usage_obj(&json)?;
    usage_from_value(usage_obj)
}

#[allow(dead_code)]
pub fn extract_usage_from_sse_bytes(data: &[u8]) -> Option<UsageMetrics> {
    let text = std::str::from_utf8(data).ok()?;
    let mut last: Option<UsageMetrics> = None;

    for chunk in text.split("\n\n") {
        let lines: Vec<&str> = chunk
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        for line in lines {
            if let Some(rest) = line.strip_prefix("data:") {
                let payload_str = rest.trim();
                if payload_str.is_empty() {
                    continue;
                }
                if let Ok(json) = serde_json::from_str::<Value>(payload_str)
                    && let Some(usage_obj) = extract_usage_obj(&json)
                {
                    if let Some(u) = usage_from_value(usage_obj) {
                        last = Some(u);
                    }
                }
            }
        }
    }

    last
}

/// Incrementally scan SSE bytes for `data: {json}` lines that contain usage information.
///
/// This is designed for streaming scenarios where the response arrives in many chunks:
/// it avoids repeatedly re-parsing the entire buffer (which can become O(n^2)).
///
/// - `scan_pos` is an in/out cursor into `data` (byte index).
/// - `last` stores the latest usage parsed so far (updated in-place).
pub fn scan_usage_from_sse_bytes_incremental(
    data: &[u8],
    scan_pos: &mut usize,
    last: &mut Option<UsageMetrics>,
) {
    let mut i = (*scan_pos).min(data.len());

    while i < data.len() {
        let Some(rel_end) = data[i..].iter().position(|b| *b == b'\n') else {
            break;
        };
        let end = i + rel_end;
        let mut line = &data[i..end];
        i = end.saturating_add(1);

        if line.ends_with(b"\r") {
            line = &line[..line.len().saturating_sub(1)];
        }
        if line.is_empty() {
            continue;
        }

        const DATA_PREFIX: &[u8] = b"data:";
        if !line.starts_with(DATA_PREFIX) {
            continue;
        }
        let mut payload = &line[DATA_PREFIX.len()..];
        while !payload.is_empty() && payload[0].is_ascii_whitespace() {
            payload = &payload[1..];
        }
        if payload.is_empty() || payload == b"[DONE]" {
            continue;
        }

        if let Ok(json) = serde_json::from_slice::<Value>(payload)
            && let Some(usage_obj) = extract_usage_obj(&json)
        {
            if let Some(u) = usage_from_value(usage_obj) {
                *last = Some(u);
            }
        }
    }

    *scan_pos = i;
}

#[cfg(test)]
mod tests {
    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn incremental_sse_scan_matches_full_parse() {
        let sse = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n",
            "\n",
            "event: response.completed\n",
            "data: {\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n",
            "\n"
        );

        let full = extract_usage_from_sse_bytes(sse.as_bytes());
        let mut pos = 0usize;
        let mut last = None;
        scan_usage_from_sse_bytes_incremental(sse.as_bytes(), &mut pos, &mut last);
        assert_eq!(last, full);
    }

    #[test]
    fn incremental_sse_scan_handles_split_lines() {
        let part1 = b"data: {\"response\":{\"usage\":{\"input_tokens\":1";
        let part2 = b",\"output_tokens\":2,\"total_tokens\":3}}}\n\n";
        let mut buf = Vec::new();
        let mut pos = 0usize;
        let mut last = None;

        buf.extend_from_slice(part1);
        scan_usage_from_sse_bytes_incremental(&buf, &mut pos, &mut last);
        assert_eq!(last, None);

        buf.extend_from_slice(part2);
        scan_usage_from_sse_bytes_incremental(&buf, &mut pos, &mut last);
        assert_eq!(
            last,
            Some(UsageMetrics {
                input_tokens: 1,
                output_tokens: 2,
                reasoning_tokens: 0,
                total_tokens: 3,
            })
        );
    }

    #[test]
    fn parses_chat_completions_usage_fields() {
        let json = r#"{
          "id":"chatcmpl_x",
          "object":"chat.completion",
          "usage":{
            "prompt_tokens":9,
            "completion_tokens":12,
            "total_tokens":21,
            "completion_tokens_details":{"reasoning_tokens":5}
          }
        }"#;
        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 9,
                output_tokens: 12,
                reasoning_tokens: 5,
                total_tokens: 21,
            })
        );
    }

    #[test]
    fn unknown_usage_schema_returns_none() {
        let json = r#"{"usage":{"foo":123}}"#;
        assert_eq!(extract_usage_from_bytes(json.as_bytes()), None);
    }
}
