use serde::{Deserialize, Serialize};
use serde_json::Value;

fn i64_is_zero(value: &i64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CacheInputAccounting {
    #[default]
    DirectReadSeparate,
    DirectReadIncludedInInput,
}

impl CacheInputAccounting {
    pub fn for_service(service: &str) -> Self {
        match service.trim().to_ascii_lowercase().as_str() {
            "codex" | "gemini" => Self::DirectReadIncludedInInput,
            _ => Self::DirectReadSeparate,
        }
    }

    fn includes_direct_read_in_input(self) -> bool {
        matches!(self, Self::DirectReadIncludedInInput)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheUsageBreakdown {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub direct_cache_read_input_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub effective_input_tokens: i64,
    pub denominator_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UsageMetrics {
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    pub reasoning_output_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    pub cached_input_tokens: i64,
    #[serde(
        default,
        alias = "cache_read_tokens",
        skip_serializing_if = "i64_is_zero"
    )]
    pub cache_read_input_tokens: i64,
    #[serde(
        default,
        alias = "cache_creation_tokens",
        skip_serializing_if = "i64_is_zero"
    )]
    pub cache_creation_input_tokens: i64,
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    pub cache_creation_5m_input_tokens: i64,
    #[serde(default, skip_serializing_if = "i64_is_zero")]
    pub cache_creation_1h_input_tokens: i64,
}

impl UsageMetrics {
    pub fn add_assign(&mut self, other: &UsageMetrics) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(other.reasoning_output_tokens_total());
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(other.cache_read_input_tokens);
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(other.cache_creation_input_tokens);
        self.cache_creation_5m_input_tokens = self
            .cache_creation_5m_input_tokens
            .saturating_add(other.cache_creation_5m_input_tokens);
        self.cache_creation_1h_input_tokens = self
            .cache_creation_1h_input_tokens
            .saturating_add(other.cache_creation_1h_input_tokens);
    }

    pub fn reasoning_output_tokens_total(&self) -> i64 {
        self.reasoning_output_tokens.max(self.reasoning_tokens)
    }

    pub fn cache_creation_tokens_total(&self) -> i64 {
        let by_ttl = self
            .cache_creation_5m_input_tokens
            .saturating_add(self.cache_creation_1h_input_tokens);
        self.cache_creation_input_tokens.max(by_ttl)
    }

    pub fn has_cache_tokens(&self) -> bool {
        self.cached_input_tokens > 0
            || self.cache_read_input_tokens > 0
            || self.cache_creation_tokens_total() > 0
    }

    pub fn cache_read_tokens_total(&self) -> i64 {
        self.cached_input_tokens
            .max(0)
            .saturating_add(self.cache_read_input_tokens.max(0))
    }

    pub fn cache_usage_breakdown(&self, accounting: CacheInputAccounting) -> CacheUsageBreakdown {
        let input = self.input_tokens.max(0);
        let cached = self.cached_input_tokens.max(0);
        let direct_read = self.cache_read_input_tokens.max(0);
        let read = cached.saturating_add(direct_read);
        let create = self.cache_creation_tokens_total().max(0);
        let included_read = if accounting.includes_direct_read_in_input() {
            read
        } else {
            cached
        };
        let effective_input = input.saturating_sub(included_read);
        let denom = effective_input.saturating_add(create).saturating_add(read);

        CacheUsageBreakdown {
            input_tokens: input,
            cached_input_tokens: cached,
            direct_cache_read_input_tokens: direct_read,
            cache_read_input_tokens: read,
            cache_creation_input_tokens: create,
            effective_input_tokens: effective_input,
            denominator_tokens: denom,
        }
    }

    pub fn cache_hit_rate_with_accounting(&self, accounting: CacheInputAccounting) -> Option<f64> {
        let breakdown = self.cache_usage_breakdown(accounting);
        if breakdown.denominator_tokens <= 0 {
            return None;
        }
        Some(breakdown.cache_read_input_tokens as f64 / breakdown.denominator_tokens as f64)
    }

    pub fn cache_hit_rate_for_service(&self, service: &str) -> Option<f64> {
        self.cache_hit_rate_with_accounting(CacheInputAccounting::for_service(service))
    }

    pub fn cache_hit_rate(&self) -> Option<f64> {
        self.cache_hit_rate_with_accounting(CacheInputAccounting::default())
    }

    pub fn effective_input_tokens_with_accounting(&self, accounting: CacheInputAccounting) -> i64 {
        self.cache_usage_breakdown(accounting)
            .effective_input_tokens
    }

    pub fn cache_denominator_tokens_with_accounting(
        &self,
        accounting: CacheInputAccounting,
    ) -> Option<i64> {
        let denom = self.cache_usage_breakdown(accounting).denominator_tokens;
        if denom <= 0 {
            return None;
        }
        Some(denom)
    }

    fn derived_total_tokens(&self) -> i64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_input_tokens)
            .saturating_add(self.cache_creation_tokens_total())
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
    let mut total_provided = false;

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
        total_provided = true;
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
        let value = to_i64(v);
        m.reasoning_tokens = value;
        m.reasoning_output_tokens = value;
        recognized = true;
    }
    if let Some(v) = usage_obj.get("reasoning_output_tokens") {
        let value = to_i64(v);
        m.reasoning_output_tokens = value;
        m.reasoning_tokens = m.reasoning_tokens.max(value);
        recognized = true;
    }

    if let Some(details) = usage_obj
        .get("output_tokens_details")
        .and_then(|v| v.as_object())
        && let Some(v) = details.get("reasoning_tokens")
    {
        let value = to_i64(v);
        m.reasoning_tokens = value;
        m.reasoning_output_tokens = value;
        recognized = true;
    }
    if let Some(details) = usage_obj
        .get("completion_tokens_details")
        .and_then(|v| v.as_object())
        && let Some(v) = details.get("reasoning_tokens")
    {
        let value = to_i64(v);
        m.reasoning_tokens = value;
        m.reasoning_output_tokens = value;
        recognized = true;
    }

    if let Some(details) = usage_obj
        .get("input_tokens_details")
        .or_else(|| usage_obj.get("input_token_details"))
        .and_then(|v| v.as_object())
        && let Some(v) = details.get("cached_tokens")
    {
        m.cached_input_tokens = to_i64(v);
        recognized = true;
    } else if let Some(details) = usage_obj
        .get("prompt_tokens_details")
        .or_else(|| usage_obj.get("prompt_token_details"))
        .and_then(|v| v.as_object())
        && let Some(v) = details.get("cached_tokens")
    {
        m.cached_input_tokens = to_i64(v);
        recognized = true;
    } else if let Some(v) = usage_obj.get("cached_input_tokens") {
        m.cached_input_tokens = to_i64(v);
        recognized = true;
    } else if let Some(v) = usage_obj
        .get("cache_read_input_tokens")
        .or_else(|| usage_obj.get("cache_read_tokens"))
    {
        m.cache_read_input_tokens = to_i64(v);
        recognized = true;
    }
    if let Some(v) = usage_obj
        .get("cache_creation_input_tokens")
        .or_else(|| usage_obj.get("cache_creation_tokens"))
    {
        m.cache_creation_input_tokens = to_i64(v);
        recognized = true;
    }
    if let Some(v) = usage_obj.get("cache_creation_5m_input_tokens") {
        m.cache_creation_5m_input_tokens = to_i64(v);
        recognized = true;
    }
    if let Some(v) = usage_obj.get("cache_creation_1h_input_tokens") {
        m.cache_creation_1h_input_tokens = to_i64(v);
        recognized = true;
    }
    if m.cache_creation_input_tokens == 0 {
        m.cache_creation_input_tokens = m
            .cache_creation_5m_input_tokens
            .saturating_add(m.cache_creation_1h_input_tokens);
    }

    // If total isn't provided, derive it from input/output when possible.
    if !total_provided {
        m.total_tokens = m.derived_total_tokens();
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
                    && let Some(u) = usage_from_value(usage_obj)
                {
                    last = Some(u);
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
            && let Some(u) = usage_from_value(usage_obj)
        {
            *last = Some(u);
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
                total_tokens: 3,
                ..UsageMetrics::default()
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
                reasoning_output_tokens: 5,
                total_tokens: 21,
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn parses_responses_usage_cache_and_reasoning_details() {
        let json = r#"{
          "response":{
            "usage":{
              "input_tokens":100,
              "output_tokens":20,
              "input_tokens_details":{"cached_tokens":40},
              "output_tokens_details":{"reasoning_tokens":7}
            }
          }
        }"#;
        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 100,
                output_tokens: 20,
                reasoning_tokens: 7,
                reasoning_output_tokens: 7,
                cached_input_tokens: 40,
                total_tokens: 120,
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn parses_openai_cached_tokens_before_direct_cache_read_tokens() {
        let json = r#"{
          "usage":{
            "input_tokens":100,
            "output_tokens":20,
            "input_tokens_details":{"cached_tokens":40},
            "cache_read_input_tokens":30
          }
        }"#;
        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 100,
                output_tokens: 20,
                cached_input_tokens: 40,
                total_tokens: 120,
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn parses_anthropic_cache_usage_fields() {
        let json = r#"{
          "usage":{
            "input_tokens":10,
            "output_tokens":5,
            "cache_read_input_tokens":30,
            "cache_creation_5m_input_tokens":20,
            "cache_creation_1h_input_tokens":40
          }
        }"#;
        assert_eq!(
            extract_usage_from_bytes(json.as_bytes()),
            Some(UsageMetrics {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 105,
                cache_read_input_tokens: 30,
                cache_creation_input_tokens: 60,
                cache_creation_5m_input_tokens: 20,
                cache_creation_1h_input_tokens: 40,
                ..UsageMetrics::default()
            })
        );
    }

    #[test]
    fn computes_cache_hit_rate_from_read_and_creation_tokens() {
        let usage = UsageMetrics {
            input_tokens: 100,
            cache_read_input_tokens: 30,
            cache_creation_input_tokens: 20,
            ..UsageMetrics::default()
        };

        let rate = usage.cache_hit_rate().expect("cache hit rate");

        assert_eq!(rate, 0.2);
    }

    #[test]
    fn computes_cache_hit_rate_when_direct_cache_read_is_included_in_input() {
        let usage = UsageMetrics {
            input_tokens: 100,
            cache_read_input_tokens: 30,
            cache_creation_input_tokens: 20,
            ..UsageMetrics::default()
        };

        let rate = usage
            .cache_hit_rate_with_accounting(CacheInputAccounting::DirectReadIncludedInInput)
            .expect("cache hit rate");

        assert_eq!(rate, 0.25);
    }

    #[test]
    fn computes_cache_hit_rate_from_cached_input_tokens() {
        let usage = UsageMetrics {
            input_tokens: 100,
            cached_input_tokens: 40,
            ..UsageMetrics::default()
        };

        let rate = usage.cache_hit_rate().expect("cache hit rate");

        assert_eq!(rate, 0.4);
    }

    #[test]
    fn computes_cache_hit_rate_from_mixed_usage_cache_fields() {
        let usage = UsageMetrics {
            input_tokens: 1_500,
            cached_input_tokens: 50,
            cache_read_input_tokens: 250,
            ..UsageMetrics::default()
        };

        let rate = usage.cache_hit_rate().expect("cache hit rate");

        assert!((rate - (300.0 / 1_750.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn computes_service_specific_cache_hit_rate_from_mixed_usage_cache_fields() {
        let usage = UsageMetrics {
            input_tokens: 1_500,
            cached_input_tokens: 50,
            cache_read_input_tokens: 250,
            ..UsageMetrics::default()
        };

        let rate = usage
            .cache_hit_rate_for_service("codex")
            .expect("cache hit rate");

        assert!((rate - (300.0 / 1_500.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn unknown_usage_schema_returns_none() {
        let json = r#"{"usage":{"foo":123}}"#;
        assert_eq!(extract_usage_from_bytes(json.as_bytes()), None);
    }
}
