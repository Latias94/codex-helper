use serde_json::Value as JsonValue;

use super::*;
use crate::logging::ControlTraceLogEntry;

pub(super) fn control_trace_summary(entry: &ControlTraceLogEntry, lang: Language) -> String {
    match entry.kind.as_str() {
        "request_completed" => control_trace_request_completed_summary(&entry.payload, lang),
        "retry_trace" => control_trace_retry_summary(entry, lang),
        _ => {
            let event = entry
                .event
                .clone()
                .or_else(|| json_field_string(&entry.payload, "event"))
                .unwrap_or_else(|| "-".to_string());
            format!("event={event}")
        }
    }
}

pub(super) fn json_field_string(value: &JsonValue, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn json_field_u64(value: &JsonValue, key: &str) -> Option<u64> {
    value.get(key).and_then(|value| match value {
        JsonValue::Number(number) => number.as_u64(),
        JsonValue::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    })
}

fn json_nested_string(value: &JsonValue, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current.as_str().map(str::to_string)
}

fn json_nested_u64(value: &JsonValue, path: &[&str]) -> Option<u64> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    match current {
        JsonValue::Number(number) => number.as_u64(),
        JsonValue::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn control_trace_request_completed_summary(payload: &JsonValue, lang: Language) -> String {
    let method = json_field_string(payload, "method").unwrap_or_else(|| "-".to_string());
    let path = json_field_string(payload, "path").unwrap_or_else(|| "-".to_string());
    let status = json_field_u64(payload, "status_code")
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let duration = json_field_u64(payload, "duration_ms")
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "-".to_string());
    let station = json_field_string(payload, "station_name").unwrap_or_else(|| "-".to_string());
    let provider = json_field_string(payload, "provider_id").unwrap_or_else(|| "-".to_string());
    let tier = json_nested_string(payload, &["service_tier", "actual"])
        .or_else(|| json_nested_string(payload, &["service_tier", "effective"]))
        .map(|value| super::format_service_tier_display(Some(value.as_str()), lang, "-"))
        .unwrap_or_else(|| "-".to_string());

    format!(
        "{} {}  st={}  dur={}  station={}  provider={}  tier={}",
        method, path, status, duration, station, provider, tier
    )
}

fn control_trace_retry_summary(entry: &ControlTraceLogEntry, lang: Language) -> String {
    let event = entry
        .event
        .clone()
        .or_else(|| json_field_string(&entry.payload, "event"))
        .unwrap_or_else(|| "retry_trace".to_string());
    match event.as_str() {
        "attempt_select" => {
            let station = json_field_string(&entry.payload, "station_name")
                .unwrap_or_else(|| "-".to_string());
            let upstream = json_field_u64(&entry.payload, "upstream_index")
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let provider =
                json_field_string(&entry.payload, "provider_id").unwrap_or_else(|| "-".to_string());
            let model =
                json_field_string(&entry.payload, "model").unwrap_or_else(|| "-".to_string());
            format!(
                "select station={} upstream#{} provider={} model={}",
                station, upstream, provider, model
            )
        }
        "retry_options" => {
            let upstream_max = json_nested_u64(&entry.payload, &["upstream", "max_attempts"])
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let provider_max = json_nested_u64(&entry.payload, &["provider", "max_attempts"])
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let cross = entry
                .payload
                .get("allow_cross_station_before_first_output")
                .and_then(|value| value.as_bool())
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            format!(
                "retry upstream_max={} provider_max={} cross_station={}",
                upstream_max, provider_max, cross
            )
        }
        "lbs_for_request" => {
            let mode = json_field_string(&entry.payload, "mode").unwrap_or_else(|| "-".to_string());
            let pinned = json_field_string(&entry.payload, "pinned_source")
                .unwrap_or_else(|| "-".to_string());
            let selected_station = json_field_string(&entry.payload, "selected_station")
                .or_else(|| json_field_string(&entry.payload, "pinned_name"))
                .unwrap_or_else(|| "-".to_string());
            format!(
                "{} mode={} selected={} pinned={}",
                pick(lang, "路由入口", "LB selection"),
                mode,
                selected_station,
                pinned
            )
        }
        _ => {
            let station = json_field_string(&entry.payload, "station_name")
                .or_else(|| json_field_string(&entry.payload, "selected_station"))
                .unwrap_or_else(|| "-".to_string());
            let base_url = json_field_string(&entry.payload, "upstream_base_url")
                .unwrap_or_else(|| "-".to_string());
            let mode = json_field_string(&entry.payload, "mode").unwrap_or_default();
            let note = json_field_string(&entry.payload, "note").unwrap_or_default();
            format!(
                "event={} station={} upstream={} {} {}",
                event,
                station,
                super::shorten_middle(base_url.as_str(), 48),
                mode,
                note
            )
            .trim()
            .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_trace_request_completed_summary_marks_fast_mode() {
        let summary = control_trace_request_completed_summary(
            &serde_json::json!({
                "method": "POST",
                "path": "/v1/responses",
                "status_code": 200,
                "duration_ms": 512,
                "station_name": "right",
                "provider_id": "right",
                "service_tier": {
                    "effective": "priority"
                }
            }),
            Language::En,
        );

        assert!(summary.contains("POST /v1/responses"));
        assert!(summary.contains("station=right"));
        assert!(summary.contains("priority (fast mode)"));
    }

    #[test]
    fn control_trace_retry_summary_formats_attempt_select() {
        let entry = ControlTraceLogEntry {
            ts_ms: 1,
            kind: "retry_trace".to_string(),
            service: Some("codex".to_string()),
            request_id: Some(7),
            event: Some("attempt_select".to_string()),
            payload: serde_json::json!({
                "event": "attempt_select",
                "station_name": "right",
                "upstream_index": 1,
                "provider_id": "right",
                "model": "gpt-5.4-fast"
            }),
        };

        let summary = control_trace_retry_summary(&entry, Language::En);

        assert!(summary.contains("station=right"));
        assert!(summary.contains("upstream#1"));
        assert!(summary.contains("gpt-5.4-fast"));
    }
}
