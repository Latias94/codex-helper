use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlTraceDetail {
    RequestCompleted {
        method: Option<String>,
        path: Option<String>,
        status_code: Option<u16>,
        duration_ms: Option<u64>,
        station_name: Option<String>,
        provider_id: Option<String>,
        upstream_base_url: Option<String>,
        service_tier: ServiceTierLog,
    },
    RetryOptions {
        upstream_max_attempts: Option<u32>,
        provider_max_attempts: Option<u32>,
        allow_cross_station_before_first_output: Option<bool>,
    },
    AttemptSelect {
        station_name: Option<String>,
        upstream_index: Option<u64>,
        upstream_base_url: Option<String>,
        provider_id: Option<String>,
        endpoint_id: Option<String>,
        provider_endpoint_key: Option<String>,
        preference_group: Option<u64>,
        model: Option<String>,
    },
    LoadBalancerSelection {
        mode: Option<String>,
        pinned_source: Option<String>,
        pinned_name: Option<String>,
        selected_station: Option<String>,
        selected_stations: Vec<String>,
        active_station: Option<String>,
        note: Option<String>,
    },
    ProviderRuntimeOverride {
        provider_name: Option<String>,
        endpoint_name: Option<String>,
        base_urls: Vec<String>,
        enabled: Option<bool>,
        clear_enabled: bool,
        runtime_state: Option<String>,
        clear_runtime_state: bool,
    },
    RouteExecutorShadowMismatch {
        request_model: Option<String>,
        legacy_attempt_count: usize,
        executor_attempt_count: usize,
        first_mismatch_index: Option<usize>,
        legacy_station_name: Option<String>,
        legacy_upstream_index: Option<u64>,
        legacy_provider_id: Option<String>,
        executor_station_name: Option<String>,
        executor_upstream_index: Option<u64>,
        executor_provider_id: Option<String>,
    },
    RouteGraphSelectionExplain {
        request_model: Option<String>,
        affinity_policy: Option<String>,
        affinity_provider_endpoint_key: Option<String>,
        selected_matches_affinity: Option<bool>,
        selected_provider_id: Option<String>,
        selected_endpoint_id: Option<String>,
        selected_provider_endpoint_key: Option<String>,
        selected_preference_group: Option<u64>,
        skipped_higher_priority_groups: Vec<u64>,
        skipped_higher_priority_candidates: Vec<ControlTraceRouteGraphSkippedCandidate>,
    },
    RetryEvent {
        event_name: String,
        station_name: Option<String>,
        upstream_base_url: Option<String>,
        mode: Option<String>,
        note: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlTraceRouteGraphSkippedCandidate {
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub provider_endpoint_key: Option<String>,
    pub preference_group: Option<u64>,
    pub route_path: Vec<String>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlTraceLogEntry {
    pub ts_ms: u64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<ControlTraceDetail>,
    #[serde(default)]
    pub payload: JsonValue,
}

impl ControlTraceLogEntry {
    pub fn resolved_trace_id(&self) -> Option<String> {
        derive_control_trace_id(
            self.trace_id.as_deref(),
            self.service.as_deref(),
            self.request_id,
            &self.payload,
        )
    }

    pub fn resolved_detail(&self) -> Option<ControlTraceDetail> {
        self.detail.clone().or_else(|| {
            infer_control_trace_detail(self.kind.as_str(), self.event.as_deref(), &self.payload)
        })
    }
}

pub fn control_trace_path() -> PathBuf {
    std::env::var("CODEX_HELPER_CONTROL_TRACE_PATH")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| proxy_home_dir().join("logs").join("control_trace.jsonl"))
}

fn control_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_bool_default("CODEX_HELPER_CONTROL_TRACE", true))
}

fn retry_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_bool("CODEX_HELPER_RETRY_TRACE"))
}

fn retry_trace_path() -> PathBuf {
    std::env::var("CODEX_HELPER_RETRY_TRACE_PATH")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| proxy_home_dir().join("logs").join("retry_trace.jsonl"))
}

fn control_trace_read_window(limit: usize) -> usize {
    limit.saturating_mul(4).clamp(80, 400)
}

pub fn read_recent_control_trace_entries(
    limit: usize,
) -> anyhow::Result<Vec<ControlTraceLogEntry>> {
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader};

    let path = control_trace_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let mut ring = VecDeque::with_capacity(control_trace_read_window(limit));
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if ring.len() == ring.capacity() {
            ring.pop_front();
        }
        ring.push_back(line);
    }

    let mut out = Vec::new();
    for line in ring {
        let Ok(entry) = serde_json::from_str::<ControlTraceLogEntry>(&line) else {
            continue;
        };
        out.push(hydrate_control_trace_entry(entry));
    }
    out.sort_by_key(|entry| std::cmp::Reverse(entry.ts_ms));
    Ok(out)
}

fn hydrate_control_trace_entry(mut entry: ControlTraceLogEntry) -> ControlTraceLogEntry {
    if entry.trace_id.is_none() {
        entry.trace_id = entry.resolved_trace_id();
    }
    if entry.detail.is_none() {
        entry.detail =
            infer_control_trace_detail(entry.kind.as_str(), entry.event.as_deref(), &entry.payload);
    }
    entry
}

fn json_u64_field(value: &JsonValue, key: &str) -> Option<u64> {
    value.get(key).and_then(|value| match value {
        JsonValue::Number(number) => number.as_u64(),
        JsonValue::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    })
}

fn json_u16_field(value: &JsonValue, key: &str) -> Option<u16> {
    json_u64_field(value, key).and_then(|value| u16::try_from(value).ok())
}

fn json_string_field(value: &JsonValue, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn json_bool_field(value: &JsonValue, key: &str) -> Option<bool> {
    value.get(key).and_then(|value| value.as_bool())
}

fn json_nested_string_field(value: &JsonValue, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn json_nested_u64_field(value: &JsonValue, path: &[&str]) -> Option<u64> {
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

fn json_string_vec_field(value: &JsonValue, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn json_u64_vec_field(value: &JsonValue, key: &str) -> Vec<u64> {
    value
        .get(key)
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| match item {
                    JsonValue::Number(number) => number.as_u64(),
                    JsonValue::String(text) => text.trim().parse::<u64>().ok(),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn json_array_field<'a>(value: &'a JsonValue, key: &str) -> &'a [JsonValue] {
    value
        .get(key)
        .and_then(|value| value.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn derive_control_trace_id(
    trace_id: Option<&str>,
    service: Option<&str>,
    request_id: Option<u64>,
    payload: &JsonValue,
) -> Option<String> {
    trace_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| json_string_field(payload, "trace_id"))
        .or_else(|| {
            let request_id = request_id.or_else(|| json_u64_field(payload, "request_id"))?;
            let service = service
                .map(str::to_string)
                .or_else(|| json_string_field(payload, "service"))
                .unwrap_or_default();
            Some(request_trace_id(service.as_str(), request_id))
        })
}

fn json_service_tier_field(value: &JsonValue) -> ServiceTierLog {
    value
        .get("service_tier")
        .cloned()
        .and_then(|value| serde_json::from_value::<ServiceTierLog>(value).ok())
        .unwrap_or_default()
}

fn infer_control_trace_detail(
    kind: &str,
    event: Option<&str>,
    payload: &JsonValue,
) -> Option<ControlTraceDetail> {
    match kind {
        "request_completed" => Some(ControlTraceDetail::RequestCompleted {
            method: json_string_field(payload, "method"),
            path: json_string_field(payload, "path"),
            status_code: json_u16_field(payload, "status_code"),
            duration_ms: json_u64_field(payload, "duration_ms"),
            station_name: json_string_field(payload, "station_name"),
            provider_id: json_string_field(payload, "provider_id"),
            upstream_base_url: json_string_field(payload, "upstream_base_url"),
            service_tier: json_service_tier_field(payload),
        }),
        "retry_trace" => infer_retry_control_trace_detail(event, payload),
        _ => None,
    }
}

fn infer_retry_control_trace_detail(
    event: Option<&str>,
    payload: &JsonValue,
) -> Option<ControlTraceDetail> {
    let event_name = event
        .map(str::to_string)
        .or_else(|| json_string_field(payload, "event"))
        .unwrap_or_else(|| "retry_trace".to_string());

    match event_name.as_str() {
        "attempt_select" => Some(ControlTraceDetail::AttemptSelect {
            station_name: json_nested_string_field(payload, &["compatibility", "station_name"])
                .or_else(|| json_string_field(payload, "station_name")),
            upstream_index: json_nested_u64_field(payload, &["compatibility", "upstream_index"])
                .or_else(|| json_u64_field(payload, "upstream_index")),
            upstream_base_url: json_string_field(payload, "upstream_base_url"),
            provider_id: json_string_field(payload, "provider_id"),
            endpoint_id: json_string_field(payload, "endpoint_id"),
            provider_endpoint_key: json_string_field(payload, "provider_endpoint_key"),
            preference_group: json_u64_field(payload, "preference_group"),
            model: json_string_field(payload, "model"),
        }),
        "retry_options" => Some(ControlTraceDetail::RetryOptions {
            upstream_max_attempts: json_nested_u64_field(payload, &["upstream", "max_attempts"])
                .and_then(|value| u32::try_from(value).ok()),
            provider_max_attempts: json_nested_u64_field(payload, &["provider", "max_attempts"])
                .and_then(|value| u32::try_from(value).ok()),
            allow_cross_station_before_first_output: json_bool_field(
                payload,
                "allow_cross_station_before_first_output",
            ),
        }),
        "lbs_for_request" => Some(ControlTraceDetail::LoadBalancerSelection {
            mode: json_string_field(payload, "mode"),
            pinned_source: json_string_field(payload, "pinned_source"),
            pinned_name: json_string_field(payload, "pinned_name"),
            selected_station: json_string_field(payload, "selected_station"),
            selected_stations: json_string_vec_field(payload, "selected_stations"),
            active_station: json_string_field(payload, "active_station"),
            note: json_string_field(payload, "note"),
        }),
        "provider_runtime_override" => Some(ControlTraceDetail::ProviderRuntimeOverride {
            provider_name: json_string_field(payload, "provider_name"),
            endpoint_name: json_string_field(payload, "endpoint_name"),
            base_urls: json_string_vec_field(payload, "base_urls"),
            enabled: json_bool_field(payload, "enabled"),
            clear_enabled: json_bool_field(payload, "clear_enabled").unwrap_or(false),
            runtime_state: json_string_field(payload, "runtime_state"),
            clear_runtime_state: json_bool_field(payload, "clear_runtime_state").unwrap_or(false),
        }),
        "route_executor_shadow_mismatch" => Some(route_executor_shadow_mismatch_detail(payload)),
        "route_graph_selection_explain" => Some(route_graph_selection_explain_detail(payload)),
        _ => Some(ControlTraceDetail::RetryEvent {
            event_name,
            station_name: json_string_field(payload, "station_name")
                .or_else(|| json_string_field(payload, "selected_station")),
            upstream_base_url: json_string_field(payload, "upstream_base_url"),
            mode: json_string_field(payload, "mode"),
            note: json_string_field(payload, "note"),
        }),
    }
}

fn route_graph_selection_explain_detail(payload: &JsonValue) -> ControlTraceDetail {
    ControlTraceDetail::RouteGraphSelectionExplain {
        request_model: json_string_field(payload, "request_model"),
        affinity_policy: json_nested_string_field(payload, &["affinity", "policy"]),
        affinity_provider_endpoint_key: json_nested_string_field(
            payload,
            &["affinity", "provider_endpoint_key"],
        ),
        selected_matches_affinity: payload
            .get("affinity")
            .and_then(|affinity| json_bool_field(affinity, "selected_matches_affinity")),
        selected_provider_id: json_nested_string_field(payload, &["selected", "provider_id"]),
        selected_endpoint_id: json_nested_string_field(payload, &["selected", "endpoint_id"]),
        selected_provider_endpoint_key: json_nested_string_field(
            payload,
            &["selected", "provider_endpoint_key"],
        ),
        selected_preference_group: json_nested_u64_field(
            payload,
            &["selected", "preference_group"],
        ),
        skipped_higher_priority_groups: json_u64_vec_field(
            payload,
            "skipped_higher_priority_groups",
        ),
        skipped_higher_priority_candidates: json_array_field(
            payload,
            "skipped_higher_priority_candidates",
        )
        .iter()
        .map(|candidate| ControlTraceRouteGraphSkippedCandidate {
            provider_id: json_string_field(candidate, "provider_id"),
            endpoint_id: json_string_field(candidate, "endpoint_id"),
            provider_endpoint_key: json_string_field(candidate, "provider_endpoint_key"),
            preference_group: json_u64_field(candidate, "preference_group"),
            route_path: json_string_vec_field(candidate, "route_path"),
            reasons: json_string_vec_field(candidate, "reasons"),
        })
        .collect(),
    }
}

fn route_executor_shadow_mismatch_detail(payload: &JsonValue) -> ControlTraceDetail {
    let legacy_attempts = json_array_field(payload, "legacy_attempts");
    let executor_attempts = json_array_field(payload, "executor_attempts");
    let first_mismatch_index = first_mismatch_index(legacy_attempts, executor_attempts);
    let legacy_attempt = first_mismatch_index.and_then(|idx| legacy_attempts.get(idx));
    let executor_attempt = first_mismatch_index.and_then(|idx| executor_attempts.get(idx));

    ControlTraceDetail::RouteExecutorShadowMismatch {
        request_model: json_string_field(payload, "request_model"),
        legacy_attempt_count: legacy_attempts.len(),
        executor_attempt_count: executor_attempts.len(),
        first_mismatch_index,
        legacy_station_name: legacy_attempt
            .and_then(|attempt| json_string_field(attempt, "station_name")),
        legacy_upstream_index: legacy_attempt
            .and_then(|attempt| json_u64_field(attempt, "upstream_index")),
        legacy_provider_id: legacy_attempt
            .and_then(|attempt| json_string_field(attempt, "provider_id")),
        executor_station_name: executor_attempt
            .and_then(|attempt| json_string_field(attempt, "station_name")),
        executor_upstream_index: executor_attempt
            .and_then(|attempt| json_u64_field(attempt, "upstream_index")),
        executor_provider_id: executor_attempt
            .and_then(|attempt| json_string_field(attempt, "provider_id")),
    }
}

fn first_mismatch_index(left: &[JsonValue], right: &[JsonValue]) -> Option<usize> {
    let shared_len = left.len().min(right.len());
    for idx in 0..shared_len {
        if left[idx] != right[idx] {
            return Some(idx);
        }
    }
    (left.len() != right.len()).then_some(shared_len)
}

pub(super) fn append_control_trace_payload(
    opt: RequestLogOptions,
    kind: &'static str,
    service: Option<&str>,
    request_id: Option<u64>,
    event: Option<&str>,
    ts_ms: u64,
    payload: JsonValue,
) {
    if !control_trace_enabled() {
        return;
    }
    let path = control_trace_path();
    ensure_log_parent(&path);
    let entry = make_control_trace_entry(kind, service, request_id, event, ts_ms, payload);
    if let Ok(line) = serde_json::to_string(&entry) {
        let _ = append_json_line(&path, opt, &line);
    }
}

fn make_control_trace_entry(
    kind: &'static str,
    service: Option<&str>,
    request_id: Option<u64>,
    event: Option<&str>,
    ts_ms: u64,
    payload: JsonValue,
) -> ControlTraceLogEntry {
    let detail = infer_control_trace_detail(kind, event, &payload);
    let trace_id = derive_control_trace_id(None, service, request_id, &payload);
    ControlTraceLogEntry {
        ts_ms,
        kind: kind.to_string(),
        service: service.map(str::to_string),
        request_id,
        trace_id,
        event: event.map(str::to_string),
        detail,
        payload,
    }
}

pub fn log_retry_trace(mut event: JsonValue) {
    let legacy_enabled = retry_trace_enabled();
    let unified_enabled = control_trace_enabled();
    if !legacy_enabled && !unified_enabled {
        return;
    }

    if let JsonValue::Object(ref mut obj) = event {
        obj.entry("ts_ms".to_string())
            .or_insert_with(|| JsonValue::Number(serde_json::Number::from(now_ms())));
    }

    let ts_ms = json_u64_field(&event, "ts_ms").unwrap_or_else(now_ms);
    let service = json_string_field(&event, "service");
    let request_id = json_u64_field(&event, "request_id");
    let event_name = json_string_field(&event, "event");
    let opt = request_log_options();
    let _guard = match log_lock().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };

    if legacy_enabled {
        let path = retry_trace_path();
        ensure_log_parent(&path);
        if let Ok(line) = serde_json::to_string(&event) {
            let _ = append_json_line(&path, opt, &line);
        }
    }

    append_control_trace_payload(
        opt,
        "retry_trace",
        service.as_deref(),
        request_id,
        event_name.as_deref(),
        ts_ms,
        event,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_field_helpers_extract_string_and_numeric_values() {
        let event = serde_json::json!({
            "service": "codex",
            "request_id": "42",
            "ts_ms": 99,
        });

        assert_eq!(
            json_string_field(&event, "service").as_deref(),
            Some("codex")
        );
        assert_eq!(json_u64_field(&event, "request_id"), Some(42));
        assert_eq!(json_u64_field(&event, "ts_ms"), Some(99));
    }

    #[test]
    fn make_control_trace_entry_keeps_kind_event_and_request_id() {
        let entry = make_control_trace_entry(
            "retry_trace",
            Some("codex"),
            Some(7),
            Some("attempt_select"),
            123,
            serde_json::json!({
                "event": "attempt_select",
                "service": "codex",
                "request_id": 7,
                "provider_id": "monthly",
                "endpoint_id": "default",
                "provider_endpoint_key": "codex/monthly/default",
                "preference_group": 0,
                "compatibility": {
                    "station_name": "routing",
                    "upstream_index": 0
                },
            }),
        );

        let value = serde_json::to_value(entry).expect("serialize control trace entry");
        assert_eq!(value["kind"].as_str(), Some("retry_trace"));
        assert_eq!(value["event"].as_str(), Some("attempt_select"));
        assert_eq!(value["request_id"].as_u64(), Some(7));
        assert_eq!(value["trace_id"].as_str(), Some("codex-7"));
        assert_eq!(value["service"].as_str(), Some("codex"));
        assert_eq!(value["payload"]["event"].as_str(), Some("attempt_select"));
        assert_eq!(value["detail"]["type"].as_str(), Some("attempt_select"));
        assert_eq!(
            value["detail"]["provider_endpoint_key"].as_str(),
            Some("codex/monthly/default")
        );
        assert_eq!(value["detail"]["preference_group"].as_u64(), Some(0));
        assert_eq!(value["detail"]["station_name"].as_str(), Some("routing"));
        assert_eq!(value["detail"]["upstream_index"].as_u64(), Some(0));
    }

    #[test]
    fn hydrate_control_trace_entry_adds_trace_id_to_legacy_event() {
        let entry: ControlTraceLogEntry = serde_json::from_value(serde_json::json!({
            "ts_ms": 1,
            "kind": "retry_trace",
            "service": "codex",
            "request_id": 7,
            "event": "attempt_select",
            "payload": {
                "event": "attempt_select",
                "station_name": "right"
            }
        }))
        .expect("deserialize legacy control trace");

        let entry = hydrate_control_trace_entry(entry);

        assert_eq!(entry.trace_id.as_deref(), Some("codex-7"));
        assert_eq!(
            entry.detail,
            Some(ControlTraceDetail::AttemptSelect {
                station_name: Some("right".to_string()),
                upstream_index: None,
                upstream_base_url: None,
                provider_id: None,
                endpoint_id: None,
                provider_endpoint_key: None,
                preference_group: None,
                model: None,
            })
        );
    }

    #[test]
    fn resolved_trace_id_prefers_payload_trace_id() {
        let entry: ControlTraceLogEntry = serde_json::from_value(serde_json::json!({
            "ts_ms": 1,
            "kind": "retry_trace",
            "service": "codex",
            "request_id": 7,
            "payload": {
                "trace_id": "external-123",
                "service": "codex",
                "request_id": 8
            }
        }))
        .expect("deserialize control trace");

        assert_eq!(entry.resolved_trace_id().as_deref(), Some("external-123"));
    }

    #[test]
    fn control_trace_entry_resolved_detail_infers_request_completed_from_legacy_payload() {
        let entry: ControlTraceLogEntry = serde_json::from_value(serde_json::json!({
            "ts_ms": 1,
            "kind": "request_completed",
            "service": "codex",
            "request_id": 11,
            "event": "request_completed",
            "payload": {
                "method": "POST",
                "path": "/v1/responses",
                "status_code": 200,
                "duration_ms": 321,
                "station_name": "right",
                "provider_id": "right",
                "service_tier": {
                    "effective": "priority"
                }
            }
        }))
        .expect("deserialize control trace");

        assert_eq!(
            entry.resolved_detail(),
            Some(ControlTraceDetail::RequestCompleted {
                method: Some("POST".to_string()),
                path: Some("/v1/responses".to_string()),
                status_code: Some(200),
                duration_ms: Some(321),
                station_name: Some("right".to_string()),
                provider_id: Some("right".to_string()),
                upstream_base_url: None,
                service_tier: ServiceTierLog {
                    requested: None,
                    effective: Some("priority".to_string()),
                    actual: None,
                },
            })
        );
    }

    #[test]
    fn control_trace_entry_resolved_detail_infers_provider_runtime_override() {
        let entry: ControlTraceLogEntry = serde_json::from_value(serde_json::json!({
            "ts_ms": 1,
            "kind": "retry_trace",
            "service": "codex",
            "event": "provider_runtime_override",
            "payload": {
                "event": "provider_runtime_override",
                "provider_name": "alpha",
                "endpoint_name": "default",
                "base_urls": ["https://alpha.example/v1"],
                "enabled": false,
                "clear_enabled": false,
                "runtime_state": "breaker_open",
                "clear_runtime_state": false
            }
        }))
        .expect("deserialize provider runtime trace");

        assert_eq!(
            entry.resolved_detail(),
            Some(ControlTraceDetail::ProviderRuntimeOverride {
                provider_name: Some("alpha".to_string()),
                endpoint_name: Some("default".to_string()),
                base_urls: vec!["https://alpha.example/v1".to_string()],
                enabled: Some(false),
                clear_enabled: false,
                runtime_state: Some("breaker_open".to_string()),
                clear_runtime_state: false,
            })
        );
    }

    #[test]
    fn control_trace_entry_resolved_detail_infers_route_graph_selection_explain() {
        let entry: ControlTraceLogEntry = serde_json::from_value(serde_json::json!({
            "ts_ms": 1,
            "kind": "retry_trace",
            "service": "codex",
            "request_id": 42,
            "event": "route_graph_selection_explain",
            "payload": {
                "event": "route_graph_selection_explain",
                "service": "codex",
                "request_id": 42,
                "request_model": "gpt-5.4",
                "affinity": {
                    "policy": "preferred_group",
                    "provider_endpoint_key": "codex/chili/default",
                    "selected_matches_affinity": false
                },
                "selected": {
                    "provider_id": "chili",
                    "endpoint_id": "default",
                    "provider_endpoint_key": "codex/chili/default",
                    "preference_group": 1,
                    "route_path": ["entry", "fallback"]
                },
                "skipped_higher_priority_groups": [0],
                "skipped_higher_priority_candidates": [
                    {
                        "provider_id": "monthly",
                        "endpoint_id": "default",
                        "provider_endpoint_key": "codex/monthly/default",
                        "preference_group": 0,
                        "route_path": ["entry", "monthly"],
                        "reasons": ["usage_exhausted"]
                    }
                ]
            }
        }))
        .expect("deserialize route graph selection explain trace");

        assert_eq!(
            entry.resolved_detail(),
            Some(ControlTraceDetail::RouteGraphSelectionExplain {
                request_model: Some("gpt-5.4".to_string()),
                affinity_policy: Some("preferred_group".to_string()),
                affinity_provider_endpoint_key: Some("codex/chili/default".to_string()),
                selected_matches_affinity: Some(false),
                selected_provider_id: Some("chili".to_string()),
                selected_endpoint_id: Some("default".to_string()),
                selected_provider_endpoint_key: Some("codex/chili/default".to_string()),
                selected_preference_group: Some(1),
                skipped_higher_priority_groups: vec![0],
                skipped_higher_priority_candidates: vec![ControlTraceRouteGraphSkippedCandidate {
                    provider_id: Some("monthly".to_string()),
                    endpoint_id: Some("default".to_string()),
                    provider_endpoint_key: Some("codex/monthly/default".to_string()),
                    preference_group: Some(0),
                    route_path: vec!["entry".to_string(), "monthly".to_string()],
                    reasons: vec!["usage_exhausted".to_string()],
                }],
            })
        );
    }

    #[test]
    fn control_trace_entry_resolved_detail_infers_route_executor_shadow_mismatch() {
        let entry: ControlTraceLogEntry = serde_json::from_value(serde_json::json!({
            "ts_ms": 1,
            "kind": "retry_trace",
            "service": "codex",
            "request_id": 19,
            "event": "route_executor_shadow_mismatch",
            "payload": {
                "event": "route_executor_shadow_mismatch",
                "service": "codex",
                "request_id": 19,
                "request_model": "gpt-5",
                "legacy_attempts": [
                    {
                        "decision": "selected",
                        "station_name": "routing",
                        "upstream_index": 1,
                        "upstream_base_url": "https://legacy.example/v1",
                        "provider_id": "legacy"
                    }
                ],
                "executor_attempts": [
                    {
                        "decision": "selected",
                        "station_name": "routing",
                        "upstream_index": 0,
                        "upstream_base_url": "https://executor.example/v1",
                        "provider_id": "executor"
                    },
                    {
                        "decision": "selected",
                        "station_name": "routing",
                        "upstream_index": 1,
                        "upstream_base_url": "https://legacy.example/v1",
                        "provider_id": "legacy"
                    }
                ]
            }
        }))
        .expect("deserialize shadow mismatch trace");

        let entry = hydrate_control_trace_entry(entry);
        assert_eq!(
            entry.resolved_detail(),
            Some(ControlTraceDetail::RouteExecutorShadowMismatch {
                request_model: Some("gpt-5".to_string()),
                legacy_attempt_count: 1,
                executor_attempt_count: 2,
                first_mismatch_index: Some(0),
                legacy_station_name: Some("routing".to_string()),
                legacy_upstream_index: Some(1),
                legacy_provider_id: Some("legacy".to_string()),
                executor_station_name: Some("routing".to_string()),
                executor_upstream_index: Some(0),
                executor_provider_id: Some("executor".to_string()),
            })
        );

        let value = serde_json::to_value(entry).expect("serialize shadow mismatch trace");
        assert_eq!(
            value["detail"]["type"].as_str(),
            Some("route_executor_shadow_mismatch")
        );
    }

    #[test]
    fn first_mismatch_index_reports_length_mismatch_after_shared_prefix() {
        let left = vec![serde_json::json!({"provider_id": "a"})];
        let right = vec![
            serde_json::json!({"provider_id": "a"}),
            serde_json::json!({"provider_id": "b"}),
        ];

        assert_eq!(first_mismatch_index(&left, &right), Some(1));
        assert_eq!(first_mismatch_index(&left, &left), None);
    }
}
