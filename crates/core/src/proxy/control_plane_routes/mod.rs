use axum::Router;
use axum::middleware;
use axum::routing::{get, post, put};

use super::ProxyService;
use super::admin::{AdminAccessConfig, require_admin_access};
use super::control_plane::{
    api_capabilities, api_operator_summary, api_v1_snapshot, apply_session_profile,
    get_global_station_override, get_session_identity_card, list_active_requests,
    list_recent_finished, list_session_identity_cards, list_session_stats, set_default_profile,
    set_global_station_override,
};
use super::control_plane_manifest::{
    API_V1_CAPABILITIES, API_V1_CONTROL_TRACE, API_V1_GLOBAL_STATION_OVERRIDE,
    API_V1_HEALTHCHECK_CANCEL, API_V1_HEALTHCHECK_START, API_V1_OPERATOR_SUMMARY,
    API_V1_PRICING_CATALOG, API_V1_PROFILE_BY_NAME, API_V1_PROFILES, API_V1_PROFILES_DEFAULT,
    API_V1_PROFILES_DEFAULT_PERSISTED, API_V1_PROVIDER_SPEC_BY_NAME, API_V1_PROVIDER_SPECS,
    API_V1_PROVIDERS, API_V1_PROVIDERS_BALANCES_REFRESH, API_V1_PROVIDERS_RUNTIME,
    API_V1_REQUEST_LEDGER_RECENT, API_V1_REQUEST_LEDGER_SUMMARY, API_V1_RETRY_CONFIG,
    API_V1_ROUTING, API_V1_ROUTING_EXPLAIN, API_V1_RUNTIME_RELOAD, API_V1_RUNTIME_STATUS,
    API_V1_SESSION_BY_ID, API_V1_SESSION_OVERRIDE_EFFORT, API_V1_SESSION_OVERRIDE_MODEL,
    API_V1_SESSION_OVERRIDE_PROFILE, API_V1_SESSION_OVERRIDE_RESET,
    API_V1_SESSION_OVERRIDE_SERVICE_TIER, API_V1_SESSION_OVERRIDE_STATION,
    API_V1_SESSION_OVERRIDES, API_V1_SESSIONS, API_V1_SNAPSHOT, API_V1_STATION_BY_NAME,
    API_V1_STATION_SPEC_BY_NAME, API_V1_STATION_SPECS, API_V1_STATIONS, API_V1_STATIONS_ACTIVE,
    API_V1_STATIONS_PROBE, API_V1_STATIONS_RUNTIME, API_V1_STATUS_ACTIVE,
    API_V1_STATUS_HEALTH_CHECKS, API_V1_STATUS_RECENT, API_V1_STATUS_SESSION_STATS,
    API_V1_STATUS_STATION_HEALTH,
};
use super::healthcheck_api::{
    cancel_health_checks, list_health_checks, list_station_health, probe_station,
    start_health_checks,
};
use super::persisted_registry_api::{
    delete_persisted_profile, delete_persisted_provider_spec, delete_persisted_station_spec,
    list_persisted_provider_specs, list_persisted_routing_spec, list_persisted_station_specs,
    set_persisted_active_station, set_persisted_default_profile, update_persisted_station,
    upsert_persisted_profile, upsert_persisted_provider_spec, upsert_persisted_routing_spec,
    upsert_persisted_station_spec,
};
use super::providers_api::{
    apply_provider_runtime_meta, list_providers, refresh_provider_balances,
};
use super::runtime_admin_api::{
    get_control_trace, get_pricing_catalog, get_request_ledger_recent, get_request_ledger_summary,
    get_retry_config, get_routing_explain, list_profiles, reload_runtime_config, runtime_status,
    set_retry_config,
};
use super::session_overrides::{
    apply_session_manual_overrides, list_session_manual_overrides, list_session_model_overrides,
    list_session_reasoning_effort_overrides, list_session_service_tier_overrides,
    list_session_station_overrides, reset_session_manual_overrides, set_session_model_override,
    set_session_reasoning_effort_override, set_session_service_tier_override,
    set_session_station_override,
};
use super::stations_api::{apply_station_runtime_meta, list_stations};

mod capability_session;
mod healthchecks;
mod overrides;
mod profiles;
mod providers;
mod routing;
mod stations;
mod status_runtime;

pub(super) fn control_plane_routes(proxy: ProxyService) -> Router {
    let admin_access = AdminAccessConfig::from_env();

    Router::new()
        .merge(capability_session::capability_and_session_routes(
            proxy.clone(),
        ))
        .merge(status_runtime::status_and_runtime_routes(proxy.clone()))
        .merge(stations::station_routes(proxy.clone()))
        .merge(providers::provider_routes(proxy.clone()))
        .merge(routing::routing_routes(proxy.clone()))
        .merge(profiles::profile_routes(proxy.clone()))
        .merge(overrides::override_routes(proxy.clone()))
        .merge(healthchecks::healthcheck_routes(proxy))
        .layer(middleware::from_fn_with_state(
            admin_access,
            require_admin_access,
        ))
}
