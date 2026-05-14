use super::*;
use crate::logging::{ControlTraceDetail, ControlTraceLogEntry};

pub(super) fn control_trace_summary(entry: &ControlTraceLogEntry, lang: Language) -> String {
    match entry.resolved_detail() {
        Some(ControlTraceDetail::RequestCompleted {
            method,
            path,
            status_code,
            duration_ms,
            station_name,
            provider_id,
            service_tier,
            ..
        }) => {
            let method = method.unwrap_or_else(|| "-".to_string());
            let path = path.unwrap_or_else(|| "-".to_string());
            let status = status_code
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let duration = duration_ms
                .map(format_duration_ms)
                .unwrap_or_else(|| "-".to_string());
            let station = station_name.unwrap_or_else(|| "-".to_string());
            let provider = provider_id.unwrap_or_else(|| "-".to_string());
            let tier = service_tier
                .actual
                .or(service_tier.effective)
                .map(|value| super::format_service_tier_display(Some(value.as_str()), lang, "-"))
                .unwrap_or_else(|| "-".to_string());

            format!(
                "{} {}  st={}  dur={}  station={}  provider={}  tier={}",
                method, path, status, duration, station, provider, tier
            )
        }
        Some(ControlTraceDetail::AttemptSelect {
            station_name,
            upstream_index,
            provider_id,
            endpoint_id,
            provider_endpoint_key,
            preference_group,
            model,
            ..
        }) => {
            let station = station_name.unwrap_or_else(|| "-".to_string());
            let upstream = upstream_index
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let model = model.unwrap_or_else(|| "-".to_string());
            let provider_endpoint = format_route_decision_provider_endpoint(
                provider_id.as_deref(),
                endpoint_id.as_deref(),
            );
            let endpoint = provider_endpoint_key
                .or_else(|| provider_endpoint.clone())
                .unwrap_or_else(|| "-".to_string());
            let provider = format_route_decision_provider_endpoint(
                provider_id.as_deref(),
                endpoint_id.as_deref(),
            )
            .or(provider_id)
            .unwrap_or_else(|| "-".to_string());
            let group = preference_group
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            format!(
                "select endpoint={} group={} provider={} model={} compat_station={} upstream#{}",
                endpoint, group, provider, model, station, upstream
            )
        }
        Some(ControlTraceDetail::RetryOptions {
            upstream_max_attempts,
            provider_max_attempts,
            allow_cross_station_before_first_output,
        }) => {
            let upstream_max = upstream_max_attempts
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let provider_max = provider_max_attempts
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let cross = allow_cross_station_before_first_output
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            format!(
                "retry upstream_max={} provider_max={} cross_station={}",
                upstream_max, provider_max, cross
            )
        }
        Some(ControlTraceDetail::LoadBalancerSelection {
            mode,
            pinned_source,
            pinned_name,
            selected_station,
            selected_stations,
            ..
        }) => {
            let selected = selected_station
                .or(pinned_name)
                .or_else(|| selected_stations.first().cloned())
                .unwrap_or_else(|| "-".to_string());
            let mode = mode.unwrap_or_else(|| "-".to_string());
            let pinned = pinned_source.unwrap_or_else(|| "-".to_string());
            format!(
                "{} mode={} selected={} pinned={}",
                pick(lang, "路由入口", "LB selection"),
                mode,
                selected,
                pinned
            )
        }
        Some(ControlTraceDetail::ProviderRuntimeOverride {
            provider_name,
            endpoint_name,
            enabled,
            runtime_state,
            base_urls,
            ..
        }) => {
            let provider = provider_name.unwrap_or_else(|| "-".to_string());
            let endpoint = endpoint_name.unwrap_or_else(|| "*".to_string());
            let enabled = enabled
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let state = runtime_state.unwrap_or_else(|| "-".to_string());
            format!(
                "{} provider={} endpoint={} enabled={} state={} urls={}",
                pick(lang, "运行时覆盖", "Runtime override"),
                provider,
                endpoint,
                enabled,
                state,
                base_urls.len()
            )
        }
        Some(ControlTraceDetail::RouteExecutorShadowMismatch {
            request_model,
            legacy_attempt_count,
            executor_attempt_count,
            first_mismatch_index,
            legacy_station_name,
            executor_station_name,
            ..
        }) => {
            let model = request_model.unwrap_or_else(|| "-".to_string());
            let mismatch = first_mismatch_index
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let legacy_station = legacy_station_name.unwrap_or_else(|| "-".to_string());
            let executor_station = executor_station_name.unwrap_or_else(|| "-".to_string());
            format!(
                "{} model={} mismatch={} legacy_attempts={} executor_attempts={} legacy_station={} executor_station={}",
                pick(lang, "路由执行器影子差异", "Route executor shadow mismatch"),
                model,
                mismatch,
                legacy_attempt_count,
                executor_attempt_count,
                legacy_station,
                executor_station
            )
        }
        Some(ControlTraceDetail::RouteGraphSelectionExplain {
            request_model,
            affinity_policy,
            affinity_provider_endpoint_key,
            selected_matches_affinity,
            selected_provider_id,
            selected_endpoint_id,
            selected_provider_endpoint_key,
            selected_preference_group,
            skipped_higher_priority_groups,
            skipped_higher_priority_candidates,
        }) => {
            let model = request_model.unwrap_or_else(|| "-".to_string());
            let policy = affinity_policy.unwrap_or_else(|| "-".to_string());
            let affinity = affinity_provider_endpoint_key.unwrap_or_else(|| "-".to_string());
            let selected = selected_provider_endpoint_key
                .or_else(|| {
                    format_route_decision_provider_endpoint(
                        selected_provider_id.as_deref(),
                        selected_endpoint_id.as_deref(),
                    )
                })
                .unwrap_or_else(|| "-".to_string());
            let group = selected_preference_group
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let affinity_match = selected_matches_affinity
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            format!(
                "{} model={} selected={} group={} affinity_policy={} affinity={} affinity_match={} skipped_groups={} skipped_candidates={}",
                pick(lang, "路由图选择解释", "Route graph selection explain"),
                model,
                selected,
                group,
                policy,
                affinity,
                affinity_match,
                skipped_higher_priority_groups.len(),
                skipped_higher_priority_candidates.len(),
            )
        }
        Some(ControlTraceDetail::RetryEvent {
            event_name,
            station_name,
            upstream_base_url,
            mode,
            note,
        }) => {
            let station = station_name.unwrap_or_else(|| "-".to_string());
            let base_url = upstream_base_url.unwrap_or_else(|| "-".to_string());
            format!(
                "event={} station={} upstream={} {} {}",
                event_name,
                station,
                super::shorten_middle(base_url.as_str(), 48),
                mode.unwrap_or_default(),
                note.unwrap_or_default()
            )
            .trim()
            .to_string()
        }
        None => {
            let event = entry.event.clone().unwrap_or_else(|| "-".to_string());
            format!("event={event}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_trace_request_completed_summary_marks_fast_mode() {
        let summary = control_trace_summary(
            &ControlTraceLogEntry {
                ts_ms: 1,
                kind: "request_completed".to_string(),
                service: Some("codex".to_string()),
                request_id: Some(1),
                trace_id: Some("codex-1".to_string()),
                event: Some("request_completed".to_string()),
                detail: Some(ControlTraceDetail::RequestCompleted {
                    method: Some("POST".to_string()),
                    path: Some("/v1/responses".to_string()),
                    status_code: Some(200),
                    duration_ms: Some(512),
                    station_name: Some("right".to_string()),
                    provider_id: Some("right".to_string()),
                    upstream_base_url: None,
                    service_tier: crate::logging::ServiceTierLog {
                        requested: None,
                        effective: Some("priority".to_string()),
                        actual: None,
                    },
                }),
                payload: serde_json::json!({}),
            },
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
            trace_id: Some("codex-7".to_string()),
            event: Some("attempt_select".to_string()),
            detail: Some(ControlTraceDetail::AttemptSelect {
                station_name: Some("right".to_string()),
                upstream_index: Some(1),
                upstream_base_url: None,
                provider_id: Some("right".to_string()),
                endpoint_id: Some("default".to_string()),
                provider_endpoint_key: Some("codex/right/default".to_string()),
                preference_group: Some(0),
                model: Some("gpt-5.4-fast".to_string()),
            }),
            payload: serde_json::json!({}),
        };

        let summary = control_trace_summary(&entry, Language::En);

        assert!(summary.contains("endpoint=codex/right/default"));
        assert!(summary.contains("group=0"));
        assert!(summary.contains("provider=right/default"));
        assert!(summary.contains("compat_station=right"));
        assert!(summary.contains("upstream#1"));
        assert!(summary.contains("gpt-5.4-fast"));
    }

    #[test]
    fn control_trace_provider_runtime_override_summary_mentions_endpoint() {
        let entry = ControlTraceLogEntry {
            ts_ms: 1,
            kind: "retry_trace".to_string(),
            service: Some("codex".to_string()),
            request_id: None,
            trace_id: None,
            event: Some("provider_runtime_override".to_string()),
            detail: Some(ControlTraceDetail::ProviderRuntimeOverride {
                provider_name: Some("alpha".to_string()),
                endpoint_name: Some("default".to_string()),
                base_urls: vec!["https://alpha.example/v1".to_string()],
                enabled: Some(false),
                clear_enabled: false,
                runtime_state: Some("breaker_open".to_string()),
                clear_runtime_state: false,
            }),
            payload: serde_json::json!({}),
        };

        let summary = control_trace_summary(&entry, Language::En);

        assert!(summary.contains("provider=alpha"));
        assert!(summary.contains("endpoint=default"));
        assert!(summary.contains("breaker_open"));
    }
}
