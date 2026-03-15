pub(super) const API_V1_CAPABILITIES: &str = "/__codex_helper/api/v1/capabilities";
pub(super) const API_V1_SNAPSHOT: &str = "/__codex_helper/api/v1/snapshot";
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
pub(super) const API_V1_CONTROL_TRACE: &str = "/__codex_helper/api/v1/control-trace";
pub(super) const API_V1_RETRY_CONFIG: &str = "/__codex_helper/api/v1/retry/config";
pub(super) const API_V1_STATIONS: &str = "/__codex_helper/api/v1/stations";
pub(super) const API_V1_STATIONS_RUNTIME: &str = "/__codex_helper/api/v1/stations/runtime";
pub(super) const API_V1_STATIONS_CONFIG_ACTIVE: &str =
    "/__codex_helper/api/v1/stations/config-active";
pub(super) const API_V1_STATIONS_PROBE: &str = "/__codex_helper/api/v1/stations/probe";
pub(super) const API_V1_STATION_BY_NAME: &str = "/__codex_helper/api/v1/stations/{name}";
pub(super) const API_V1_STATION_SPECS: &str = "/__codex_helper/api/v1/stations/specs";
pub(super) const API_V1_STATION_SPEC_BY_NAME: &str = "/__codex_helper/api/v1/stations/specs/{name}";
pub(super) const API_V1_PROVIDERS: &str = "/__codex_helper/api/v1/providers";
pub(super) const API_V1_PROVIDERS_RUNTIME: &str = "/__codex_helper/api/v1/providers/runtime";
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
pub(super) const API_V1_SESSION_OVERRIDE_SERVICE_TIER: &str =
    "/__codex_helper/api/v1/overrides/session/service-tier";
pub(super) const API_V1_SESSION_OVERRIDE_RESET: &str =
    "/__codex_helper/api/v1/overrides/session/reset";
pub(super) const API_V1_GLOBAL_STATION_OVERRIDE: &str =
    "/__codex_helper/api/v1/overrides/global-station";
pub(super) const API_V1_HEALTHCHECK_START: &str = "/__codex_helper/api/v1/healthcheck/start";
pub(super) const API_V1_HEALTHCHECK_CANCEL: &str = "/__codex_helper/api/v1/healthcheck/cancel";

const API_V1_ENDPOINT_PATHS: &[&str] = &[
    API_V1_CAPABILITIES,
    API_V1_SNAPSHOT,
    API_V1_SESSIONS,
    API_V1_SESSION_BY_ID,
    API_V1_STATUS_ACTIVE,
    API_V1_STATUS_RECENT,
    API_V1_STATUS_SESSION_STATS,
    API_V1_STATUS_HEALTH_CHECKS,
    API_V1_STATUS_STATION_HEALTH,
    API_V1_RUNTIME_STATUS,
    API_V1_RUNTIME_RELOAD,
    API_V1_CONTROL_TRACE,
    API_V1_RETRY_CONFIG,
    API_V1_STATIONS,
    API_V1_STATIONS_RUNTIME,
    API_V1_STATIONS_CONFIG_ACTIVE,
    API_V1_STATIONS_PROBE,
    API_V1_STATION_BY_NAME,
    API_V1_STATION_SPECS,
    API_V1_STATION_SPEC_BY_NAME,
    API_V1_PROVIDERS,
    API_V1_PROVIDERS_RUNTIME,
    API_V1_PROVIDER_SPECS,
    API_V1_PROVIDER_SPEC_BY_NAME,
    API_V1_PROFILES,
    API_V1_PROFILES_DEFAULT,
    API_V1_PROFILES_DEFAULT_PERSISTED,
    API_V1_PROFILE_BY_NAME,
    API_V1_SESSION_OVERRIDES,
    API_V1_SESSION_OVERRIDE_PROFILE,
    API_V1_SESSION_OVERRIDE_MODEL,
    API_V1_SESSION_OVERRIDE_EFFORT,
    API_V1_SESSION_OVERRIDE_STATION,
    API_V1_SESSION_OVERRIDE_SERVICE_TIER,
    API_V1_SESSION_OVERRIDE_RESET,
    API_V1_GLOBAL_STATION_OVERRIDE,
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
}
