use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value as JsonValue;

pub use crate::logging::request_log_path;
use crate::pricing::{CostAdjustments, estimate_request_cost_from_operator_catalog};
use crate::usage::UsageMetrics;

#[derive(Debug, Clone, PartialEq)]
pub struct RequestLogLine {
    raw: String,
    value: Option<JsonValue>,
}

impl RequestLogLine {
    pub fn from_raw(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let value = serde_json::from_str::<JsonValue>(&raw).ok();
        Self { raw, value }
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn value(&self) -> Option<&JsonValue> {
        self.value.as_ref()
    }

    pub fn is_valid_json(&self) -> bool {
        self.value.is_some()
    }

    pub fn display_lines(&self) -> Vec<String> {
        self.value
            .as_ref()
            .map(format_request_log_record_lines)
            .unwrap_or_default()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RequestLogFilters {
    pub session: Option<String>,
    pub model: Option<String>,
    pub station: Option<String>,
    pub provider: Option<String>,
    pub status_min: Option<u64>,
    pub status_max: Option<u64>,
    pub fast: bool,
    pub retried: bool,
}

impl RequestLogFilters {
    pub fn matches(&self, record: &JsonValue) -> bool {
        if let Some(expected) = self.session.as_deref()
            && !field_contains(str_field(record, "session_id"), expected)
        {
            return false;
        }
        if let Some(expected) = self.model.as_deref()
            && !field_contains(request_model(record).as_deref(), expected)
        {
            return false;
        }
        if let Some(expected) = self.station.as_deref()
            && !field_contains(Some(station_name(record)), expected)
        {
            return false;
        }
        if let Some(expected) = self.provider.as_deref()
            && !field_contains(str_field(record, "provider_id"), expected)
        {
            return false;
        }
        if let Some(min) = self.status_min
            && u64_field(record, "status_code").unwrap_or(0) < min
        {
            return false;
        }
        if let Some(max) = self.status_max
            && u64_field(record, "status_code").unwrap_or(0) > max
        {
            return false;
        }
        if self.fast && !request_is_fast(record) {
            return false;
        }
        if self.retried && !request_was_retried(record) {
            return false;
        }
        true
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RequestUsageAggregate {
    pub requests: u64,
    pub duration_ms_total: u64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub total_tokens: i64,
}

impl RequestUsageAggregate {
    pub fn record(&mut self, duration_ms: u64, usage: Option<&UsageMetrics>) {
        self.requests = self.requests.saturating_add(1);
        self.duration_ms_total = self.duration_ms_total.saturating_add(duration_ms);
        let Some(usage) = usage else {
            return;
        };

        self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens.max(0));
        self.output_tokens = self
            .output_tokens
            .saturating_add(usage.output_tokens.max(0));
        self.reasoning_tokens = self
            .reasoning_tokens
            .saturating_add(reasoning_tokens(usage));
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(usage.cache_read_input_tokens.max(0));
        self.cache_creation_input_tokens = self
            .cache_creation_input_tokens
            .saturating_add(cache_creation_tokens(usage));
        self.total_tokens = self.total_tokens.saturating_add(total_tokens(usage));
    }

    pub fn average_duration_ms(&self) -> u64 {
        if self.requests == 0 {
            0
        } else {
            self.duration_ms_total / self.requests
        }
    }

    pub fn summary_line(&self, station_name: &str) -> String {
        format!(
            "{} | {} | {} | {} | {} | {} | {} | {} | {}",
            station_name,
            self.requests,
            self.input_tokens,
            self.output_tokens,
            self.cache_read_input_tokens,
            self.cache_creation_input_tokens,
            self.reasoning_tokens,
            self.total_tokens,
            self.average_duration_ms()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestUsageSummaryRow {
    pub station_name: String,
    pub aggregate: RequestUsageAggregate,
}

pub fn read_request_log_lines(path: &Path) -> std::io::Result<Vec<RequestLogLine>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    Ok(reader
        .lines()
        .map_while(Result::ok)
        .map(RequestLogLine::from_raw)
        .collect())
}

pub fn tail_request_log(path: &Path, limit: usize) -> std::io::Result<Vec<RequestLogLine>> {
    let lines = read_request_log_lines(path)?;
    let total = lines.len();
    let start = total.saturating_sub(limit);
    Ok(lines[start..].to_vec())
}

pub fn summarize_request_log(
    path: &Path,
    limit: usize,
) -> std::io::Result<Vec<RequestUsageSummaryRow>> {
    let lines = read_request_log_lines(path)?;
    Ok(summarize_request_log_lines(lines.iter(), limit))
}

pub fn find_request_log(
    path: &Path,
    filters: &RequestLogFilters,
    limit: usize,
) -> std::io::Result<Vec<RequestLogLine>> {
    let lines = read_request_log_lines(path)?;
    Ok(lines
        .iter()
        .rev()
        .filter(|line| line.value().is_some_and(|record| filters.matches(record)))
        .take(limit)
        .cloned()
        .collect())
}

fn summarize_request_log_lines<'a>(
    lines: impl IntoIterator<Item = &'a RequestLogLine>,
    limit: usize,
) -> Vec<RequestUsageSummaryRow> {
    let mut aggregate: HashMap<String, RequestUsageAggregate> = HashMap::new();
    for line in lines {
        let Some(record) = line.value() else {
            continue;
        };
        let station_name = station_name(record).to_string();
        let duration_ms = u64_field(record, "duration_ms").unwrap_or(0);
        let entry = aggregate.entry(station_name).or_default();
        entry.record(duration_ms, usage_metrics(record).as_ref());
    }

    let mut items: Vec<RequestUsageSummaryRow> = aggregate
        .into_iter()
        .map(|(station_name, aggregate)| RequestUsageSummaryRow {
            station_name,
            aggregate,
        })
        .collect();
    items.sort_by(|a, b| b.aggregate.total_tokens.cmp(&a.aggregate.total_tokens));
    items.into_iter().take(limit).collect()
}

pub fn format_request_log_record_lines(record: &JsonValue) -> Vec<String> {
    let ts = i64_field(record, "timestamp_ms").unwrap_or(0);
    let service = str_field(record, "service").unwrap_or("-");
    let method = str_field(record, "method").unwrap_or("-");
    let path = str_field(record, "path").unwrap_or("-");
    let status = u64_field(record, "status_code").unwrap_or(0);
    let station = station_name(record);
    let provider = str_field(record, "provider_id").unwrap_or("-");
    let model = request_model(record).unwrap_or_else(|| "-".to_string());
    let tier = service_tier_display(record);

    let mut lines = vec![format!(
        "[{}] {} {} {} status={} station={} provider={} model={} tier={}",
        ts, service, method, path, status, station, provider, model, tier
    )];

    let duration_ms = u64_field(record, "duration_ms").unwrap_or(0);
    let ttfb_ms = u64_field(record, "ttfb_ms");
    let usage = usage_metrics(record);
    let speed = usage
        .as_ref()
        .and_then(|usage| output_tokens_per_second(usage, duration_ms, ttfb_ms));
    let cost = request_cost_display(model.as_str(), usage.as_ref());

    lines.push(format!(
        "    timing duration={} ttfb={} output_speed={} cost={}",
        format_ms(duration_ms),
        format_optional_ms(ttfb_ms),
        format_optional_speed(speed),
        cost
    ));

    if let Some(usage) = usage.as_ref() {
        lines.push(format!(
            "    tokens input={} output={} cache_read={} cache_create={} reasoning={} total={}",
            usage.input_tokens.max(0),
            usage.output_tokens.max(0),
            usage.cache_read_input_tokens.max(0),
            cache_creation_tokens(usage),
            reasoning_tokens(usage),
            total_tokens(usage)
        ));
    } else {
        lines.push("    tokens -".to_string());
    }

    lines
}

pub fn request_log_record_model(record: &JsonValue) -> Option<String> {
    request_model(record)
}

pub fn request_log_record_station(record: &JsonValue) -> &str {
    station_name(record)
}

pub fn request_log_record_is_fast(record: &JsonValue) -> bool {
    request_is_fast(record)
}

pub fn request_log_record_was_retried(record: &JsonValue) -> bool {
    request_was_retried(record)
}

fn field_contains(value: Option<&str>, expected: &str) -> bool {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.is_empty() {
        return true;
    }
    value
        .map(|value| value.to_ascii_lowercase().contains(&expected))
        .unwrap_or(false)
}

fn request_is_fast(record: &JsonValue) -> bool {
    record
        .get("observability")
        .and_then(|observability| observability.get("fast_mode"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
        || service_tier_display(record).eq_ignore_ascii_case("priority(fast)")
}

fn request_was_retried(record: &JsonValue) -> bool {
    if record
        .get("observability")
        .and_then(|observability| observability.get("retried"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    if record
        .get("observability")
        .and_then(|observability| observability.get("attempt_count"))
        .and_then(|value| value.as_u64())
        .is_some_and(|attempts| attempts > 1)
    {
        return true;
    }
    record
        .get("retry")
        .and_then(|retry| retry.get("route_attempts"))
        .and_then(|attempts| attempts.as_array())
        .is_some_and(|attempts| attempts.len() > 1)
}

fn usage_metrics(record: &JsonValue) -> Option<UsageMetrics> {
    record
        .get("usage")
        .and_then(|usage| serde_json::from_value::<UsageMetrics>(usage.clone()).ok())
}

fn request_cost_display(model: &str, usage: Option<&UsageMetrics>) -> String {
    let model = model.trim();
    if model.is_empty() || model == "-" {
        return "-".to_string();
    }
    let cost =
        estimate_request_cost_from_operator_catalog(Some(model), usage, CostAdjustments::default());
    cost.display_total_with_confidence()
}

fn output_tokens_per_second(
    usage: &UsageMetrics,
    duration_ms: u64,
    ttfb_ms: Option<u64>,
) -> Option<f64> {
    let output_tokens = usage.output_tokens.max(0);
    if output_tokens == 0 || duration_ms == 0 {
        return None;
    }
    let generation_ms = match ttfb_ms {
        Some(ttfb) if ttfb > 0 && ttfb < duration_ms => duration_ms.saturating_sub(ttfb),
        _ => duration_ms,
    };
    if generation_ms == 0 {
        return None;
    }
    Some(output_tokens as f64 / (generation_ms as f64 / 1000.0))
}

fn request_model(record: &JsonValue) -> Option<String> {
    str_field(record, "model")
        .map(ToOwned::to_owned)
        .or_else(|| nested_str(record, &["route_decision", "effective_model", "value"]))
        .or_else(|| model_from_route_attempts(record))
        .or_else(|| model_from_legacy_retry_chain(record))
}

fn model_from_route_attempts(record: &JsonValue) -> Option<String> {
    record
        .get("retry")
        .and_then(|retry| retry.get("route_attempts"))
        .and_then(|attempts| attempts.as_array())
        .and_then(|attempts| {
            attempts
                .iter()
                .rev()
                .filter(|attempt| {
                    !attempt
                        .get("skipped")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false)
                })
                .find_map(|attempt| str_field(attempt, "model").map(ToOwned::to_owned))
        })
}

fn model_from_legacy_retry_chain(record: &JsonValue) -> Option<String> {
    record
        .get("retry")
        .and_then(|retry| retry.get("upstream_chain"))
        .and_then(|chain| chain.as_array())
        .and_then(|chain| {
            chain
                .iter()
                .rev()
                .filter_map(|entry| entry.as_str())
                .find_map(|entry| raw_kv_field(entry, "model"))
        })
}

fn raw_kv_field(raw: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    raw.split_whitespace()
        .find_map(|part| part.strip_prefix(&prefix))
        .map(|value| value.trim().trim_matches(',').to_string())
        .filter(|value| !value.is_empty() && value != "-")
}

fn service_tier_display(record: &JsonValue) -> String {
    let tier = record
        .get("service_tier")
        .and_then(|tier| {
            tier.as_str()
                .map(ToOwned::to_owned)
                .or_else(|| service_tier_log_value(tier))
        })
        .unwrap_or_else(|| "-".to_string());
    if tier.eq_ignore_ascii_case("priority") {
        "priority(fast)".to_string()
    } else {
        tier
    }
}

fn service_tier_log_value(tier: &JsonValue) -> Option<String> {
    ["actual", "effective", "requested"]
        .iter()
        .find_map(|key| str_field(tier, key).map(ToOwned::to_owned))
}

fn station_name(record: &JsonValue) -> &str {
    str_field(record, "station_name")
        .or_else(|| str_field(record, "config_name"))
        .unwrap_or("-")
}

fn str_field<'a>(record: &'a JsonValue, key: &str) -> Option<&'a str> {
    record
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn nested_str(record: &JsonValue, path: &[&str]) -> Option<String> {
    let mut current = record;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn i64_field(record: &JsonValue, key: &str) -> Option<i64> {
    record.get(key).and_then(|value| value.as_i64())
}

fn u64_field(record: &JsonValue, key: &str) -> Option<u64> {
    record.get(key).and_then(|value| value.as_u64())
}

fn reasoning_tokens(usage: &UsageMetrics) -> i64 {
    usage.reasoning_output_tokens_total().max(0)
}

fn cache_creation_tokens(usage: &UsageMetrics) -> i64 {
    usage.cache_creation_tokens_total().max(0)
}

fn total_tokens(usage: &UsageMetrics) -> i64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage
            .input_tokens
            .max(0)
            .saturating_add(usage.output_tokens.max(0))
            .saturating_add(usage.cache_read_input_tokens.max(0))
            .saturating_add(cache_creation_tokens(usage))
    }
}

fn format_ms(value: u64) -> String {
    format!("{value}ms")
}

fn format_optional_ms(value: Option<u64>) -> String {
    value.map(format_ms).unwrap_or_else(|| "-".to_string())
}

fn format_optional_speed(value: Option<f64>) -> String {
    value
        .map(|speed| format!("{speed:.2} tok/s"))
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn display_lines_include_route_model_fast_cache_and_speed() {
        let record = json!({
            "timestamp_ms": 123,
            "service": "codex",
            "method": "POST",
            "path": "/v1/responses",
            "status_code": 200,
            "duration_ms": 2000,
            "ttfb_ms": 500,
            "station_name": "main",
            "provider_id": "relay",
            "service_tier": { "effective": "priority" },
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 30,
                "cache_read_input_tokens": 10,
                "cache_creation_5m_input_tokens": 5,
                "reasoning_output_tokens": 7,
                "total_tokens": 1045
            },
            "retry": {
                "route_attempts": [
                    { "decision": "completed", "model": "gpt-5" }
                ]
            }
        });

        let lines = format_request_log_record_lines(&record);

        assert!(lines[0].contains("model=gpt-5"));
        assert!(lines[0].contains("tier=priority(fast)"));
        assert!(lines[1].contains("output_speed=20.00 tok/s"));
        assert!(lines[1].contains("cost="));
        assert!(lines[2].contains("cache_read=10"));
        assert!(lines[2].contains("cache_create=5"));
        assert!(lines[2].contains("reasoning=7"));
    }

    #[test]
    fn request_model_reads_legacy_retry_chain_model() {
        let record = json!({
            "retry": {
                "upstream_chain": [
                    "main:https://relay.example/v1 (idx=0) status=200 class=- model=gpt-5.4-mini"
                ]
            }
        });

        assert_eq!(request_model(&record).as_deref(), Some("gpt-5.4-mini"));
    }

    #[test]
    fn usage_aggregate_summary_includes_cache_and_average_duration() {
        let mut aggregate = RequestUsageAggregate::default();
        aggregate.record(
            200,
            Some(&UsageMetrics {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_input_tokens: 3,
                cache_creation_1h_input_tokens: 2,
                reasoning_output_tokens: 4,
                ..UsageMetrics::default()
            }),
        );
        aggregate.record(400, None);

        assert_eq!(
            aggregate.summary_line("main"),
            "main | 2 | 10 | 5 | 3 | 2 | 4 | 20 | 300"
        );
    }

    #[test]
    fn filters_match_route_model_fast_retry_and_status() {
        let record = json!({
            "session_id": "sid-abc",
            "station_name": "main-station",
            "provider_id": "relay-one",
            "status_code": 429,
            "service_tier": { "actual": "priority" },
            "observability": {
                "attempt_count": 2,
                "retried": true
            },
            "retry": {
                "route_attempts": [
                    { "decision": "failed_status", "model": "gpt-5.4-high" },
                    { "decision": "completed", "model": "gpt-5.4-high" }
                ]
            }
        });

        let filters = RequestLogFilters {
            session: Some("abc".to_string()),
            model: Some("5.4".to_string()),
            station: Some("main".to_string()),
            provider: Some("relay".to_string()),
            status_min: Some(400),
            status_max: Some(499),
            fast: true,
            retried: true,
        };

        assert!(filters.matches(&record));
    }

    #[test]
    fn filters_reject_nonmatching_model() {
        let record = json!({
            "model": "gpt-5.4-high",
            "status_code": 200
        });
        let filters = RequestLogFilters {
            model: Some("mini".to_string()),
            ..RequestLogFilters::default()
        };

        assert!(!filters.matches(&record));
    }

    #[test]
    fn tail_keeps_invalid_raw_lines_but_summary_ignores_them() {
        let lines = [
            RequestLogLine::from_raw(
                r#"{"station_name":"a","duration_ms":100,"usage":{"total_tokens":7}}"#,
            ),
            RequestLogLine::from_raw("not-json"),
            RequestLogLine::from_raw(
                r#"{"station_name":"b","duration_ms":200,"usage":{"total_tokens":3}}"#,
            ),
        ];

        assert!(!lines[1].is_valid_json());
        let rows = summarize_request_log_lines(lines.iter(), 10);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].station_name, "a");
        assert_eq!(rows[0].aggregate.total_tokens, 7);
        assert_eq!(rows[1].station_name, "b");
        assert_eq!(rows[1].aggregate.total_tokens, 3);
    }
}
