use crate::dashboard_core::{ControlPlaneSurfaceCapabilities, OperatorSummaryLinks};

pub(super) const API_V1_CAPABILITIES: &str = "/__codex_helper/api/v1/capabilities";
pub(super) const API_V1_SNAPSHOT: &str = "/__codex_helper/api/v1/snapshot";
pub(super) const API_V1_OPERATOR_SUMMARY: &str = "/__codex_helper/api/v1/operator/summary";
pub(super) const API_V1_SESSIONS: &str = "/__codex_helper/api/v1/sessions";
pub(super) const API_V1_SESSION_BY_ID: &str = "/__codex_helper/api/v1/sessions/{session_id}";
pub(super) const API_V1_STATUS_ACTIVE: &str = "/__codex_helper/api/v1/status/active";
pub(super) const API_V1_STATUS_RECENT: &str = "/__codex_helper/api/v1/status/recent";
pub(super) const API_V1_STATUS_SESSION_STATS: &str = "/__codex_helper/api/v1/status/session-stats";
pub(super) const API_V1_STATUS_HEALTH_CHECKS: &str = "/__codex_helper/api/v1/status/health-checks";
pub(super) const API_V1_STATUS_STATION_HEALTH: &str =
    "/__codex_helper/api/v1/status/station-health";
pub(super) const API_V1_RUNTIME_STATUS: &str = "/__codex_helper/api/v1/runtime/status";
pub(super) const API_V1_RUNTIME_RELOAD: &str = "/__codex_helper/api/v1/runtime/reload";
pub(super) const API_V1_REQUEST_LEDGER_RECENT: &str =
    "/__codex_helper/api/v1/request-ledger/recent";
pub(super) const API_V1_REQUEST_LEDGER_SUMMARY: &str =
    "/__codex_helper/api/v1/request-ledger/summary";
pub(super) const API_V1_CONTROL_TRACE: &str = "/__codex_helper/api/v1/control-trace";
pub(super) const API_V1_RETRY_CONFIG: &str = "/__codex_helper/api/v1/retry/config";
pub(super) const API_V1_PRICING_CATALOG: &str = "/__codex_helper/api/v1/pricing/catalog";
pub(super) const API_V1_ROUTING: &str = "/__codex_helper/api/v1/routing";
pub(super) const API_V1_ROUTING_EXPLAIN: &str = "/__codex_helper/api/v1/routing/explain";
pub(super) const API_V1_STATIONS: &str = "/__codex_helper/api/v1/stations";
pub(super) const API_V1_STATIONS_RUNTIME: &str = "/__codex_helper/api/v1/stations/runtime";
pub(super) const API_V1_STATIONS_ACTIVE: &str = "/__codex_helper/api/v1/stations/active";
pub(super) const API_V1_STATIONS_PROBE: &str = "/__codex_helper/api/v1/stations/probe";
pub(super) const API_V1_STATION_BY_NAME: &str = "/__codex_helper/api/v1/stations/{name}";
pub(super) const API_V1_STATION_SPECS: &str = "/__codex_helper/api/v1/stations/specs";
pub(super) const API_V1_STATION_SPEC_BY_NAME: &str = "/__codex_helper/api/v1/stations/specs/{name}";
pub(super) const API_V1_PROVIDERS: &str = "/__codex_helper/api/v1/providers";
pub(super) const API_V1_PROVIDERS_RUNTIME: &str = "/__codex_helper/api/v1/providers/runtime";
pub(super) const API_V1_PROVIDERS_BALANCES_REFRESH: &str =
    "/__codex_helper/api/v1/providers/balances/refresh";
pub(super) const API_V1_PROVIDER_SPECS: &str = "/__codex_helper/api/v1/providers/specs";
pub(super) const API_V1_PROVIDER_SPEC_BY_NAME: &str =
    "/__codex_helper/api/v1/providers/specs/{name}";
pub(super) const API_V1_PROFILES: &str = "/__codex_helper/api/v1/profiles";
pub(super) const API_V1_PROFILES_DEFAULT: &str = "/__codex_helper/api/v1/profiles/default";
pub(super) const API_V1_PROFILES_DEFAULT_PERSISTED: &str =
    "/__codex_helper/api/v1/profiles/default/persisted";
pub(super) const API_V1_PROFILE_BY_NAME: &str = "/__codex_helper/api/v1/profiles/{name}";
pub(super) const API_V1_SESSION_OVERRIDES: &str = "/__codex_helper/api/v1/overrides/session";
pub(super) const API_V1_SESSION_OVERRIDE_PROFILE: &str =
    "/__codex_helper/api/v1/overrides/session/profile";
pub(super) const API_V1_SESSION_OVERRIDE_MODEL: &str =
    "/__codex_helper/api/v1/overrides/session/model";
pub(super) const API_V1_SESSION_OVERRIDE_EFFORT: &str =
    "/__codex_helper/api/v1/overrides/session/effort";
pub(super) const API_V1_SESSION_OVERRIDE_STATION: &str =
    "/__codex_helper/api/v1/overrides/session/station";
pub(super) const API_V1_SESSION_OVERRIDE_ROUTE: &str =
    "/__codex_helper/api/v1/overrides/session/route";
pub(super) const API_V1_SESSION_OVERRIDE_SERVICE_TIER: &str =
    "/__codex_helper/api/v1/overrides/session/service-tier";
pub(super) const API_V1_SESSION_OVERRIDE_RESET: &str =
    "/__codex_helper/api/v1/overrides/session/reset";
pub(super) const API_V1_GLOBAL_STATION_OVERRIDE: &str =
    "/__codex_helper/api/v1/overrides/global-station";
pub(super) const API_V1_GLOBAL_ROUTE_OVERRIDE: &str =
    "/__codex_helper/api/v1/overrides/global-route";
pub(super) const API_V1_HEALTHCHECK_START: &str = "/__codex_helper/api/v1/healthcheck/start";
pub(super) const API_V1_HEALTHCHECK_CANCEL: &str = "/__codex_helper/api/v1/healthcheck/cancel";

const API_V1_ENDPOINT_PATHS: &[&str] = &[
    API_V1_CAPABILITIES,
    API_V1_SNAPSHOT,
    API_V1_OPERATOR_SUMMARY,
    API_V1_SESSIONS,
    API_V1_SESSION_BY_ID,
    API_V1_STATUS_ACTIVE,
    API_V1_STATUS_RECENT,
    API_V1_STATUS_SESSION_STATS,
    API_V1_STATUS_HEALTH_CHECKS,
    API_V1_STATUS_STATION_HEALTH,
    API_V1_RUNTIME_STATUS,
    API_V1_RUNTIME_RELOAD,
    API_V1_REQUEST_LEDGER_RECENT,
    API_V1_REQUEST_LEDGER_SUMMARY,
    API_V1_CONTROL_TRACE,
    API_V1_RETRY_CONFIG,
    API_V1_PRICING_CATALOG,
    API_V1_ROUTING,
    API_V1_ROUTING_EXPLAIN,
    API_V1_STATIONS,
    API_V1_STATIONS_RUNTIME,
    API_V1_STATIONS_ACTIVE,
    API_V1_STATIONS_PROBE,
    API_V1_STATION_BY_NAME,
    API_V1_STATION_SPECS,
    API_V1_STATION_SPEC_BY_NAME,
    API_V1_PROVIDERS,
    API_V1_PROVIDERS_RUNTIME,
    API_V1_PROVIDER_SPECS,
    API_V1_PROVIDER_SPEC_BY_NAME,
    API_V1_PROVIDERS_BALANCES_REFRESH,
    API_V1_PROFILES,
    API_V1_PROFILES_DEFAULT,
    API_V1_PROFILES_DEFAULT_PERSISTED,
    API_V1_PROFILE_BY_NAME,
    API_V1_SESSION_OVERRIDES,
    API_V1_SESSION_OVERRIDE_PROFILE,
    API_V1_SESSION_OVERRIDE_MODEL,
    API_V1_SESSION_OVERRIDE_EFFORT,
    API_V1_SESSION_OVERRIDE_STATION,
    API_V1_SESSION_OVERRIDE_ROUTE,
    API_V1_SESSION_OVERRIDE_SERVICE_TIER,
    API_V1_SESSION_OVERRIDE_RESET,
    API_V1_GLOBAL_STATION_OVERRIDE,
    API_V1_GLOBAL_ROUTE_OVERRIDE,
    API_V1_HEALTHCHECK_START,
    API_V1_HEALTHCHECK_CANCEL,
];

pub(super) fn api_v1_endpoint_paths() -> Vec<String> {
    API_V1_ENDPOINT_PATHS
        .iter()
        .copied()
        .map(str::to_string)
        .collect()
}

pub(super) fn api_v1_surface_capabilities() -> ControlPlaneSurfaceCapabilities {
    ControlPlaneSurfaceCapabilities {
        snapshot: true,
        operator_summary: true,
        status_active: true,
        status_recent: true,
        status_session_stats: true,
        status_health_checks: true,
        status_station_health: true,
        runtime_status: true,
        runtime_reload: true,
        request_ledger_recent: true,
        request_ledger_summary: true,
        control_trace: true,
        retry_config: true,
        pricing_catalog: true,
        routing: true,
        routing_explain: true,
        stations: true,
        station_runtime: true,
        station_persisted_settings: true,
        station_specs: true,
        station_probe: true,
        providers: true,
        provider_runtime: true,
        provider_balance_refresh: true,
        provider_specs: true,
        profiles: true,
        default_profile_override: true,
        persisted_default_profile: true,
        profile_mutation: true,
        session_overrides: true,
        session_profile_override: true,
        session_model_override: true,
        session_reasoning_effort_override: true,
        session_station_override: true,
        session_route_override: true,
        session_service_tier_override: true,
        session_override_reset: true,
        global_station_override: true,
        global_route_override: true,
        healthcheck_start: true,
        healthcheck_cancel: true,
    }
}

pub(super) fn api_v1_operator_summary_links() -> OperatorSummaryLinks {
    OperatorSummaryLinks {
        snapshot: API_V1_SNAPSHOT.to_string(),
        status_active: API_V1_STATUS_ACTIVE.to_string(),
        runtime_status: API_V1_RUNTIME_STATUS.to_string(),
        runtime_reload: API_V1_RUNTIME_RELOAD.to_string(),
        status_recent: API_V1_STATUS_RECENT.to_string(),
        status_session_stats: API_V1_STATUS_SESSION_STATS.to_string(),
        status_health_checks: API_V1_STATUS_HEALTH_CHECKS.to_string(),
        status_station_health: API_V1_STATUS_STATION_HEALTH.to_string(),
        request_ledger_recent: API_V1_REQUEST_LEDGER_RECENT.to_string(),
        request_ledger_summary: API_V1_REQUEST_LEDGER_SUMMARY.to_string(),
        control_trace: API_V1_CONTROL_TRACE.to_string(),
        retry_config: API_V1_RETRY_CONFIG.to_string(),
        pricing_catalog: API_V1_PRICING_CATALOG.to_string(),
        routing: API_V1_ROUTING.to_string(),
        routing_explain: API_V1_ROUTING_EXPLAIN.to_string(),
        sessions: API_V1_SESSIONS.to_string(),
        session_by_id_template: API_V1_SESSION_BY_ID.to_string(),
        session_overrides: API_V1_SESSION_OVERRIDES.to_string(),
        global_station_override: API_V1_GLOBAL_STATION_OVERRIDE.to_string(),
        global_route_override: API_V1_GLOBAL_ROUTE_OVERRIDE.to_string(),
        stations: API_V1_STATIONS.to_string(),
        station_by_name_template: API_V1_STATION_BY_NAME.to_string(),
        station_specs: API_V1_STATION_SPECS.to_string(),
        station_spec_by_name_template: API_V1_STATION_SPEC_BY_NAME.to_string(),
        station_probe: API_V1_STATIONS_PROBE.to_string(),
        healthcheck_start: API_V1_HEALTHCHECK_START.to_string(),
        healthcheck_cancel: API_V1_HEALTHCHECK_CANCEL.to_string(),
        providers: API_V1_PROVIDERS.to_string(),
        provider_balance_refresh: API_V1_PROVIDERS_BALANCES_REFRESH.to_string(),
        provider_specs: API_V1_PROVIDER_SPECS.to_string(),
        provider_spec_by_name_template: API_V1_PROVIDER_SPEC_BY_NAME.to_string(),
        profiles: API_V1_PROFILES.to_string(),
        profile_by_name_template: API_V1_PROFILE_BY_NAME.to_string(),
        default_profile: API_V1_PROFILES_DEFAULT.to_string(),
        persisted_default_profile: API_V1_PROFILES_DEFAULT_PERSISTED.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::API_V1_ENDPOINT_PATHS;

    #[test]
    fn api_v1_endpoint_paths_are_unique() {
        let mut seen = BTreeSet::new();
        for path in API_V1_ENDPOINT_PATHS {
            assert!(seen.insert(*path), "duplicate endpoint path: {path}");
        }
    }

    #[test]
    fn api_v1_endpoint_paths_include_routing_surface() {
        assert!(API_V1_ENDPOINT_PATHS.contains(&super::API_V1_ROUTING));
        assert!(API_V1_ENDPOINT_PATHS.contains(&super::API_V1_ROUTING_EXPLAIN));
    }
}
