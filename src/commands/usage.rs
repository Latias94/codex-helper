use crate::config::proxy_home_dir;
use crate::pricing::{CostAdjustments, estimate_request_cost_from_operator_catalog};
use crate::usage::UsageMetrics;
use crate::{CliError, CliResult, UsageCommand};
use owo_colors::OwoColorize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

pub async fn handle_usage_cmd(cmd: UsageCommand) -> CliResult<()> {
    let log_path: PathBuf = proxy_home_dir().join("logs").join("requests.jsonl");
    if !log_path.exists() {
        println!("No request logs found at {:?}", log_path);
        return Ok(());
    }

    match cmd {
        UsageCommand::Tail { limit, raw } => {
            let file = File::open(&log_path)
                .map_err(|e| CliError::Usage(format!("无法打开请求日志 {:?}: {}", log_path, e)))?;
            let reader = BufReader::new(file);
            let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
            let total = lines.len();
            let start = total.saturating_sub(limit);
            for line in &lines[start..] {
                if raw {
                    // 原样输出 JSON 行，方便 jq/脚本进一步处理
                    println!("{line}");
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<JsonValue>(line) {
                    for out in tail_record_lines(&v) {
                        println!("{out}");
                    }
                }
            }
        }
        UsageCommand::Summary { limit } => {
            let file = File::open(&log_path)
                .map_err(|e| CliError::Usage(format!("无法打开请求日志 {:?}: {}", log_path, e)))?;
            let reader = BufReader::new(file);
            let mut aggregate: HashMap<String, UsageAggregate> = HashMap::new();

            for line in reader.lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<JsonValue>(&line) {
                    let station_name = station_name(&v).to_string();
                    let duration_ms = u64_field(&v, "duration_ms").unwrap_or(0);
                    let entry = aggregate.entry(station_name).or_default();
                    entry.record(duration_ms, usage_metrics(&v).as_ref());
                }
            }

            let mut items: Vec<(String, UsageAggregate)> = aggregate.into_iter().collect();
            items.sort_by(|a, b| b.1.total_tokens.cmp(&a.1.total_tokens));

            println!(
                "{}",
                format!("Usage summary by station (from {:?})", log_path).bold()
            );
            println!(
                "{}",
                "station_name | requests | input | output | cache_read | cache_create | reasoning | total | avg_duration_ms"
                    .bold()
            );
            for (name, aggregate) in items.into_iter().take(limit) {
                println!("{}", aggregate.summary_line(&name));
            }
        }
        UsageCommand::Find {
            limit,
            session,
            model,
            station,
            provider,
            status_min,
            status_max,
            errors,
            fast,
            retried,
            raw,
        } => {
            let file = File::open(&log_path)
                .map_err(|e| CliError::Usage(format!("无法打开请求日志 {:?}: {}", log_path, e)))?;
            let reader = BufReader::new(file);
            let lines = reader.lines().map_while(Result::ok).collect::<Vec<_>>();
            let filters = UsageFindFilters {
                session: session.as_deref(),
                model: model.as_deref(),
                station: station.as_deref(),
                provider: provider.as_deref(),
                status_min: status_min.or(errors.then_some(400)),
                status_max,
                fast,
                retried,
            };
            let mut printed = 0usize;
            for line in lines.iter().rev() {
                let Ok(v) = serde_json::from_str::<JsonValue>(line) else {
                    continue;
                };
                if !request_matches_filters(&v, &filters) {
                    continue;
                }

                if raw {
                    println!("{line}");
                } else {
                    for out in tail_record_lines(&v) {
                        println!("{out}");
                    }
                }
                printed += 1;
                if printed >= limit {
                    break;
                }
            }
            if printed == 0 && !raw {
                println!("No request records matched the filters in {:?}.", log_path);
            }
        }
    }

    Ok(())
}

#[derive(Debug, Default)]
struct UsageFindFilters<'a> {
    session: Option<&'a str>,
    model: Option<&'a str>,
    station: Option<&'a str>,
    provider: Option<&'a str>,
    status_min: Option<u64>,
    status_max: Option<u64>,
    fast: bool,
    retried: bool,
}

fn request_matches_filters(v: &JsonValue, filters: &UsageFindFilters<'_>) -> bool {
    if let Some(expected) = filters.session
        && !field_contains(str_field(v, "session_id"), expected)
    {
        return false;
    }
    if let Some(expected) = filters.model
        && !field_contains(request_model(v).as_deref(), expected)
    {
        return false;
    }
    if let Some(expected) = filters.station
        && !field_contains(Some(station_name(v)), expected)
    {
        return false;
    }
    if let Some(expected) = filters.provider
        && !field_contains(str_field(v, "provider_id"), expected)
    {
        return false;
    }
    if let Some(min) = filters.status_min
        && u64_field(v, "status_code").unwrap_or(0) < min
    {
        return false;
    }
    if let Some(max) = filters.status_max
        && u64_field(v, "status_code").unwrap_or(0) > max
    {
        return false;
    }
    if filters.fast && !request_is_fast(v) {
        return false;
    }
    if filters.retried && !request_was_retried(v) {
        return false;
    }
    true
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

fn request_is_fast(v: &JsonValue) -> bool {
    v.get("observability")
        .and_then(|observability| observability.get("fast_mode"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
        || service_tier_display(v).eq_ignore_ascii_case("priority(fast)")
}

fn request_was_retried(v: &JsonValue) -> bool {
    if v.get("observability")
        .and_then(|observability| observability.get("retried"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return true;
    }
    if v.get("observability")
        .and_then(|observability| observability.get("attempt_count"))
        .and_then(|value| value.as_u64())
        .is_some_and(|attempts| attempts > 1)
    {
        return true;
    }
    v.get("retry")
        .and_then(|retry| retry.get("route_attempts"))
        .and_then(|attempts| attempts.as_array())
        .is_some_and(|attempts| attempts.len() > 1)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct UsageAggregate {
    requests: u64,
    duration_ms_total: u64,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    total_tokens: i64,
}

impl UsageAggregate {
    fn record(&mut self, duration_ms: u64, usage: Option<&UsageMetrics>) {
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

    fn summary_line(&self, station_name: &str) -> String {
        let avg_duration = if self.requests == 0 {
            0
        } else {
            self.duration_ms_total / self.requests
        };
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
            avg_duration
        )
    }
}

fn tail_record_lines(v: &JsonValue) -> Vec<String> {
    let ts = i64_field(v, "timestamp_ms").unwrap_or(0);
    let service = str_field(v, "service").unwrap_or("-");
    let method = str_field(v, "method").unwrap_or("-");
    let path = str_field(v, "path").unwrap_or("-");
    let status = u64_field(v, "status_code").unwrap_or(0);
    let station = station_name(v);
    let provider = str_field(v, "provider_id").unwrap_or("-");
    let model = request_model(v).unwrap_or_else(|| "-".to_string());
    let tier = service_tier_display(v);

    let mut lines = vec![format!(
        "[{}] {} {} {} status={} station={} provider={} model={} tier={}",
        ts, service, method, path, status, station, provider, model, tier
    )];

    let duration_ms = u64_field(v, "duration_ms").unwrap_or(0);
    let ttfb_ms = u64_field(v, "ttfb_ms");
    let usage = usage_metrics(v);
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

fn usage_metrics(v: &JsonValue) -> Option<UsageMetrics> {
    v.get("usage")
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

fn request_model(v: &JsonValue) -> Option<String> {
    str_field(v, "model")
        .map(ToOwned::to_owned)
        .or_else(|| nested_str(v, &["route_decision", "effective_model", "value"]))
        .or_else(|| model_from_route_attempts(v))
        .or_else(|| model_from_legacy_retry_chain(v))
}

fn model_from_route_attempts(v: &JsonValue) -> Option<String> {
    v.get("retry")
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

fn model_from_legacy_retry_chain(v: &JsonValue) -> Option<String> {
    v.get("retry")
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

fn service_tier_display(v: &JsonValue) -> String {
    let tier = v
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

fn station_name(v: &JsonValue) -> &str {
    str_field(v, "station_name")
        .or_else(|| str_field(v, "config_name"))
        .unwrap_or("-")
}

fn str_field<'a>(v: &'a JsonValue, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn nested_str(v: &JsonValue, path: &[&str]) -> Option<String> {
    let mut current = v;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn i64_field(v: &JsonValue, key: &str) -> Option<i64> {
    v.get(key).and_then(|value| value.as_i64())
}

fn u64_field(v: &JsonValue, key: &str) -> Option<u64> {
    v.get(key).and_then(|value| value.as_u64())
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
    fn tail_record_lines_include_route_model_fast_cache_and_speed() {
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

        let lines = tail_record_lines(&record);

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
        let mut aggregate = UsageAggregate::default();
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
    fn request_matches_filters_uses_route_model_fast_retry_and_status() {
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

        let filters = UsageFindFilters {
            session: Some("abc"),
            model: Some("5.4"),
            station: Some("main"),
            provider: Some("relay"),
            status_min: Some(400),
            status_max: Some(499),
            fast: true,
            retried: true,
        };

        assert!(request_matches_filters(&record, &filters));
    }

    #[test]
    fn request_matches_filters_rejects_nonmatching_model() {
        let record = json!({
            "model": "gpt-5.4-high",
            "status_code": 200
        });
        let filters = UsageFindFilters {
            model: Some("mini"),
            ..UsageFindFilters::default()
        };

        assert!(!request_matches_filters(&record, &filters));
    }
}
