use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::config::RetryProfileName;
use crate::state::{
    FinishedRequest, HealthCheckStatus, LbConfigView, PassiveHealthState, RuntimeConfigState,
    SessionIdentityCard, StationHealth,
};

use super::types::{
    ControlPlaneSurfaceCapabilities, ControlProfileOption, HostLocalControlPlaneCapabilities,
    ProviderOption, RemoteAdminAccessCapabilities, SharedControlPlaneCapabilities, StationOption,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiV1OperatorSummary {
    pub api_version: u32,
    pub service_name: String,
    pub runtime: OperatorRuntimeSummary,
    pub counts: OperatorSummaryCounts,
    pub retry: OperatorRetrySummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<OperatorHealthSummary>,
    #[serde(default)]
    pub session_cards: Vec<SessionIdentityCard>,
    #[serde(default)]
    pub stations: Vec<StationOption>,
    #[serde(default)]
    pub profiles: Vec<ControlProfileOption>,
    #[serde(default)]
    pub providers: Vec<ProviderOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<OperatorSummaryLinks>,
    pub surface_capabilities: ControlPlaneSurfaceCapabilities,
    pub shared_capabilities: SharedControlPlaneCapabilities,
    pub host_local_capabilities: HostLocalControlPlaneCapabilities,
    pub remote_admin_access: RemoteAdminAccessCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorRuntimeSummary {
    #[serde(default)]
    pub runtime_loaded_at_ms: Option<u64>,
    #[serde(default)]
    pub runtime_source_mtime_ms: Option<u64>,
    #[serde(default)]
    pub configured_active_station: Option<String>,
    #[serde(default)]
    pub effective_active_station: Option<String>,
    #[serde(default)]
    pub global_station_override: Option<String>,
    #[serde(default)]
    pub configured_default_profile: Option<String>,
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub default_profile_summary: Option<OperatorProfileSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorProfileSummary {
    pub name: String,
    #[serde(default)]
    pub station: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub fast_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorRetrySummary {
    #[serde(default)]
    pub configured_profile: Option<RetryProfileName>,
    #[serde(default)]
    pub supports_write: bool,
    pub upstream_max_attempts: u32,
    pub provider_max_attempts: u32,
    #[serde(default)]
    pub allow_cross_station_before_first_output: bool,
    #[serde(default)]
    pub recent_retried_requests: usize,
    #[serde(default)]
    pub recent_cross_station_failovers: usize,
    #[serde(default)]
    pub recent_fast_mode_requests: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OperatorRetryObservations {
    pub recent_retried_requests: usize,
    pub recent_cross_station_failovers: usize,
    pub recent_fast_mode_requests: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorSummaryCounts {
    #[serde(default)]
    pub active_requests: usize,
    #[serde(default)]
    pub recent_requests: usize,
    #[serde(default)]
    pub sessions: usize,
    #[serde(default)]
    pub stations: usize,
    #[serde(default)]
    pub profiles: usize,
    #[serde(default)]
    pub providers: usize,
}

pub fn summarize_recent_retry_observations(
    recent: &[FinishedRequest],
) -> OperatorRetryObservations {
    let mut observations = OperatorRetryObservations::default();

    for request in recent {
        if request
            .service_tier
            .as_deref()
            .is_some_and(|tier| tier.eq_ignore_ascii_case("priority"))
        {
            observations.recent_fast_mode_requests += 1;
        }

        let Some(retry) = request.retry.as_ref() else {
            continue;
        };
        if retry.attempts <= 1 {
            continue;
        }

        observations.recent_retried_requests += 1;
        if retry.touched_other_station(request.station_name.as_deref()) {
            observations.recent_cross_station_failovers += 1;
        }
    }

    observations
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorHealthSummary {
    #[serde(default)]
    pub stations_draining: usize,
    #[serde(default)]
    pub stations_breaker_open: usize,
    #[serde(default)]
    pub stations_half_open: usize,
    #[serde(default)]
    pub stations_with_active_health_checks: usize,
    #[serde(default)]
    pub stations_with_probe_failures: usize,
    #[serde(default)]
    pub stations_with_degraded_passive_health: usize,
    #[serde(default)]
    pub stations_with_failing_passive_health: usize,
    #[serde(default)]
    pub stations_with_cooldown: usize,
    #[serde(default)]
    pub stations_with_usage_exhaustion: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OperatorSummaryLinks {
    pub snapshot: String,
    pub status_active: String,
    pub runtime_status: String,
    pub runtime_reload: String,
    pub status_recent: String,
    pub status_session_stats: String,
    pub status_health_checks: String,
    pub status_station_health: String,
    pub control_trace: String,
    pub retry_config: String,
    pub sessions: String,
    pub session_by_id_template: String,
    pub session_overrides: String,
    pub global_station_override: String,
    pub stations: String,
    pub station_by_name_template: String,
    pub station_specs: String,
    pub station_spec_by_name_template: String,
    pub station_probe: String,
    #[serde(default)]
    pub healthcheck_start: String,
    #[serde(default)]
    pub healthcheck_cancel: String,
    pub providers: String,
    pub provider_specs: String,
    pub provider_spec_by_name_template: String,
    pub profiles: String,
    pub profile_by_name_template: String,
    pub default_profile: String,
    pub persisted_default_profile: String,
}

pub fn build_operator_health_summary(
    stations: &[StationOption],
    station_health: &HashMap<String, StationHealth>,
    health_checks: &HashMap<String, HealthCheckStatus>,
    lb_view: &HashMap<String, LbConfigView>,
) -> OperatorHealthSummary {
    let mut summary = OperatorHealthSummary::default();

    for station in stations {
        match station.runtime_state {
            RuntimeConfigState::Draining => summary.stations_draining += 1,
            RuntimeConfigState::BreakerOpen => summary.stations_breaker_open += 1,
            RuntimeConfigState::HalfOpen => summary.stations_half_open += 1,
            RuntimeConfigState::Normal => {}
        }
    }

    let station_names = stations
        .iter()
        .map(|station| station.name.as_str())
        .chain(station_health.keys().map(String::as_str))
        .chain(health_checks.keys().map(String::as_str))
        .chain(lb_view.keys().map(String::as_str))
        .collect::<BTreeSet<_>>();

    for station_name in station_names {
        let health = station_health.get(station_name);
        let check_status = health_checks.get(station_name);
        let lb = lb_view.get(station_name);

        if check_status.is_some_and(|status| !status.done && !status.canceled) {
            summary.stations_with_active_health_checks += 1;
        }

        if station_has_probe_failures(health) {
            summary.stations_with_probe_failures += 1;
        }

        match strongest_passive_health_state(health) {
            Some(PassiveHealthState::Failing) => summary.stations_with_failing_passive_health += 1,
            Some(PassiveHealthState::Degraded) => {
                summary.stations_with_degraded_passive_health += 1;
            }
            _ => {}
        }

        if lb.is_some_and(|view| {
            view.upstreams
                .iter()
                .any(|upstream| upstream.cooldown_remaining_secs.is_some())
        }) {
            summary.stations_with_cooldown += 1;
        }

        if lb.is_some_and(|view| {
            view.upstreams
                .iter()
                .any(|upstream| upstream.usage_exhausted)
        }) {
            summary.stations_with_usage_exhaustion += 1;
        }
    }

    summary
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    fn finished_request(
        station_name: Option<&str>,
        service_tier: Option<&str>,
        retry: Option<crate::logging::RetryInfo>,
    ) -> FinishedRequest {
        FinishedRequest {
            id: 1,
            trace_id: Some("codex-1".to_string()),
            session_id: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: service_tier.map(str::to_string),
            station_name: station_name.map(str::to_string),
            provider_id: None,
            upstream_base_url: None,
            route_decision: None,
            usage: None,
            retry,
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 100,
            ttfb_ms: None,
            ended_at_ms: 1,
        }
    }

    #[test]
    fn summarize_recent_retry_observations_reports_retry_failover_and_fast_mode() {
        let recent = vec![
            finished_request(
                Some("alpha"),
                Some("PRIORITY"),
                Some(crate::logging::RetryInfo {
                    attempts: 2,
                    upstream_chain: vec!["alpha:http://alpha.example/v1".to_string()],
                    route_attempts: Vec::new(),
                }),
            ),
            finished_request(
                Some("alpha"),
                Some("default"),
                Some(crate::logging::RetryInfo {
                    attempts: 3,
                    upstream_chain: vec![
                        "beta:http://beta.example/v1".to_string(),
                        "alpha:http://alpha.example/v1".to_string(),
                    ],
                    route_attempts: Vec::new(),
                }),
            ),
            finished_request(Some("alpha"), Some("priority"), None),
        ];

        let summary = summarize_recent_retry_observations(&recent);

        assert_eq!(summary.recent_retried_requests, 2);
        assert_eq!(summary.recent_cross_station_failovers, 1);
        assert_eq!(summary.recent_fast_mode_requests, 2);
    }
}

fn station_has_probe_failures(health: Option<&StationHealth>) -> bool {
    let Some(health) = health else {
        return false;
    };
    if health.upstreams.is_empty() {
        return false;
    }

    let has_ok = health
        .upstreams
        .iter()
        .any(|upstream| upstream.ok == Some(true));
    if has_ok {
        return false;
    }

    health.upstreams.iter().any(|upstream| {
        upstream.ok == Some(false) || upstream.status_code.is_some() || upstream.error.is_some()
    })
}

fn strongest_passive_health_state(health: Option<&StationHealth>) -> Option<PassiveHealthState> {
    let health = health?;
    let mut has_degraded = false;

    for passive in health
        .upstreams
        .iter()
        .filter_map(|upstream| upstream.passive.as_ref())
    {
        match passive.state {
            PassiveHealthState::Failing => return Some(PassiveHealthState::Failing),
            PassiveHealthState::Degraded => has_degraded = true,
            PassiveHealthState::Healthy | PassiveHealthState::Unknown => {}
        }
    }

    if has_degraded {
        Some(PassiveHealthState::Degraded)
    } else {
        None
    }
}
