use axum::Router;
use axum::middleware;
use axum::routing::{get, post, put};

use super::ProxyService;
use super::admin::{AdminAccessConfig, require_admin_access};
use super::control_plane::{
    api_capabilities, api_v1_snapshot, apply_session_profile, get_global_station_override,
    get_session_identity_card, list_active_requests, list_recent_finished,
    list_session_identity_cards, list_session_stats, set_default_profile,
    set_global_station_override,
};
use super::control_plane_manifest::{
    API_V1_CAPABILITIES, API_V1_CONTROL_TRACE, API_V1_GLOBAL_STATION_OVERRIDE,
    API_V1_HEALTHCHECK_CANCEL, API_V1_HEALTHCHECK_START, API_V1_PROFILE_BY_NAME, API_V1_PROFILES,
    API_V1_PROFILES_DEFAULT, API_V1_PROFILES_DEFAULT_PERSISTED, API_V1_PROVIDER_SPEC_BY_NAME,
    API_V1_PROVIDER_SPECS, API_V1_PROVIDERS, API_V1_PROVIDERS_RUNTIME, API_V1_RETRY_CONFIG,
    API_V1_RUNTIME_RELOAD, API_V1_RUNTIME_STATUS, API_V1_SESSION_BY_ID,
    API_V1_SESSION_OVERRIDE_EFFORT, API_V1_SESSION_OVERRIDE_MODEL, API_V1_SESSION_OVERRIDE_PROFILE,
    API_V1_SESSION_OVERRIDE_RESET, API_V1_SESSION_OVERRIDE_SERVICE_TIER,
    API_V1_SESSION_OVERRIDE_STATION, API_V1_SESSION_OVERRIDES, API_V1_SESSIONS, API_V1_SNAPSHOT,
    API_V1_STATION_BY_NAME, API_V1_STATION_SPEC_BY_NAME, API_V1_STATION_SPECS, API_V1_STATIONS,
    API_V1_STATIONS_CONFIG_ACTIVE, API_V1_STATIONS_PROBE, API_V1_STATIONS_RUNTIME,
    API_V1_STATUS_ACTIVE, API_V1_STATUS_HEALTH_CHECKS, API_V1_STATUS_RECENT,
    API_V1_STATUS_SESSION_STATS, API_V1_STATUS_STATION_HEALTH,
};
use super::healthcheck_api::{
    cancel_health_checks, list_health_checks, list_station_health, probe_station,
    start_health_checks,
};
use super::persisted_config_api::{
    delete_persisted_profile, delete_persisted_provider_spec, delete_persisted_station_spec,
    list_persisted_provider_specs, list_persisted_station_specs, set_persisted_active_station,
    set_persisted_default_profile, update_persisted_station, upsert_persisted_profile,
    upsert_persisted_provider_spec, upsert_persisted_station_spec,
};
use super::providers_api::{apply_provider_runtime_meta, list_providers};
use super::runtime_admin_api::{
    get_control_trace, get_retry_config, list_profiles, reload_runtime_config,
    runtime_config_status, set_retry_config,
};
use super::session_overrides::{
    apply_session_manual_overrides, list_session_manual_overrides, list_session_model_overrides,
    list_session_reasoning_effort_overrides, list_session_service_tier_overrides,
    list_session_station_overrides, reset_session_manual_overrides, set_session_model_override,
    set_session_reasoning_effort_override, set_session_service_tier_override,
    set_session_station_override,
};
use super::stations_api::{apply_station_runtime_meta, list_stations};

pub(super) fn control_plane_routes(proxy: ProxyService) -> Router {
    let admin_access = AdminAccessConfig::from_env();

    Router::new()
        .merge(capability_and_session_routes(proxy.clone()))
        .merge(status_and_runtime_routes(proxy.clone()))
        .merge(station_routes(proxy.clone()))
        .merge(provider_routes(proxy.clone()))
        .merge(profile_routes(proxy.clone()))
        .merge(override_routes(proxy.clone()))
        .merge(healthcheck_routes(proxy))
        .layer(middleware::from_fn_with_state(
            admin_access,
            require_admin_access,
        ))
}

fn capability_and_session_routes(proxy: ProxyService) -> Router {
    let capabilities_proxy = proxy.clone();
    let snapshot_proxy = proxy.clone();
    let sessions_proxy = proxy.clone();

    Router::new()
        .route(
            API_V1_CAPABILITIES,
            get(move || api_capabilities(capabilities_proxy.clone())),
        )
        .route(
            API_V1_SNAPSHOT,
            get(move |query| api_v1_snapshot(snapshot_proxy.clone(), query)),
        )
        .route(
            API_V1_SESSIONS,
            get(move || list_session_identity_cards(sessions_proxy.clone())),
        )
        .route(
            API_V1_SESSION_BY_ID,
            get(move |session_id| get_session_identity_card(proxy.clone(), session_id)),
        )
}

fn status_and_runtime_routes(proxy: ProxyService) -> Router {
    let active_proxy = proxy.clone();
    let recent_proxy = proxy.clone();
    let stats_proxy = proxy.clone();
    let health_proxy = proxy.clone();
    let station_health_proxy = proxy.clone();
    let runtime_status_proxy = proxy.clone();
    let runtime_reload_proxy = proxy.clone();
    let control_trace_proxy = proxy.clone();
    let retry_get_proxy = proxy.clone();

    Router::new()
        .route(
            API_V1_STATUS_ACTIVE,
            get(move || list_active_requests(active_proxy.clone())),
        )
        .route(
            API_V1_STATUS_RECENT,
            get(move |query| list_recent_finished(recent_proxy.clone(), query)),
        )
        .route(
            API_V1_STATUS_SESSION_STATS,
            get(move || list_session_stats(stats_proxy.clone())),
        )
        .route(
            API_V1_STATUS_HEALTH_CHECKS,
            get(move || list_health_checks(health_proxy.clone())),
        )
        .route(
            API_V1_STATUS_STATION_HEALTH,
            get(move || list_station_health(station_health_proxy.clone())),
        )
        .route(
            API_V1_RUNTIME_STATUS,
            get(move || runtime_config_status(runtime_status_proxy.clone())),
        )
        .route(
            API_V1_RUNTIME_RELOAD,
            post(move || reload_runtime_config(runtime_reload_proxy.clone())),
        )
        .route(
            API_V1_CONTROL_TRACE,
            get(move |query| get_control_trace(control_trace_proxy.clone(), query)),
        )
        .route(
            API_V1_RETRY_CONFIG,
            get(move || get_retry_config(retry_get_proxy.clone()))
                .post(move |payload| set_retry_config(proxy.clone(), payload)),
        )
}

fn station_routes(proxy: ProxyService) -> Router {
    let stations_proxy = proxy.clone();
    let runtime_proxy = proxy.clone();
    let active_proxy = proxy.clone();
    let probe_proxy = proxy.clone();
    let update_proxy = proxy.clone();
    let specs_proxy = proxy.clone();
    let upsert_proxy = proxy.clone();

    Router::new()
        .route(
            API_V1_STATIONS,
            get(move || list_stations(stations_proxy.clone())),
        )
        .route(
            API_V1_STATIONS_RUNTIME,
            post(move |payload| apply_station_runtime_meta(runtime_proxy.clone(), payload)),
        )
        .route(
            API_V1_STATIONS_CONFIG_ACTIVE,
            post(move |payload| set_persisted_active_station(active_proxy.clone(), payload)),
        )
        .route(
            API_V1_STATIONS_PROBE,
            post(move |payload| probe_station(probe_proxy.clone(), payload)),
        )
        .route(
            API_V1_STATION_BY_NAME,
            put(move |name, payload| update_persisted_station(update_proxy.clone(), name, payload)),
        )
        .route(
            API_V1_STATION_SPECS,
            get(move || list_persisted_station_specs(specs_proxy.clone())),
        )
        .route(
            API_V1_STATION_SPEC_BY_NAME,
            put(move |name, payload| {
                upsert_persisted_station_spec(upsert_proxy.clone(), name, payload)
            })
            .delete(move |name| delete_persisted_station_spec(proxy.clone(), name)),
        )
}

fn provider_routes(proxy: ProxyService) -> Router {
    let specs_proxy = proxy.clone();
    let providers_proxy = proxy.clone();
    let runtime_proxy = proxy.clone();
    let upsert_proxy = proxy.clone();

    Router::new()
        .route(
            API_V1_PROVIDER_SPECS,
            get(move || list_persisted_provider_specs(specs_proxy.clone())),
        )
        .route(
            API_V1_PROVIDERS,
            get(move || list_providers(providers_proxy.clone())),
        )
        .route(
            API_V1_PROVIDERS_RUNTIME,
            post(move |payload| apply_provider_runtime_meta(runtime_proxy.clone(), payload)),
        )
        .route(
            API_V1_PROVIDER_SPEC_BY_NAME,
            put(move |name, payload| {
                upsert_persisted_provider_spec(upsert_proxy.clone(), name, payload)
            })
            .delete(move |name| delete_persisted_provider_spec(proxy.clone(), name)),
        )
}

fn profile_routes(proxy: ProxyService) -> Router {
    let list_proxy = proxy.clone();
    let default_proxy = proxy.clone();
    let persisted_default_proxy = proxy.clone();
    let upsert_proxy = proxy.clone();

    Router::new()
        .route(
            API_V1_PROFILES,
            get(move || list_profiles(list_proxy.clone())),
        )
        .route(
            API_V1_PROFILES_DEFAULT,
            post(move |payload| set_default_profile(default_proxy.clone(), payload)),
        )
        .route(
            API_V1_PROFILES_DEFAULT_PERSISTED,
            post(move |payload| {
                set_persisted_default_profile(persisted_default_proxy.clone(), payload)
            }),
        )
        .route(
            API_V1_PROFILE_BY_NAME,
            put(move |name, payload| upsert_persisted_profile(upsert_proxy.clone(), name, payload))
                .delete(move |name| delete_persisted_profile(proxy.clone(), name)),
        )
}

fn override_routes(proxy: ProxyService) -> Router {
    let session_overrides_apply_proxy = proxy.clone();
    let session_overrides_proxy = proxy.clone();
    let session_profile_proxy = proxy.clone();
    let session_model_list_proxy = proxy.clone();
    let session_model_set_proxy = proxy.clone();
    let session_effort_list_proxy = proxy.clone();
    let session_effort_set_proxy = proxy.clone();
    let session_station_list_proxy = proxy.clone();
    let session_station_set_proxy = proxy.clone();
    let session_service_tier_list_proxy = proxy.clone();
    let session_service_tier_set_proxy = proxy.clone();
    let session_reset_proxy = proxy.clone();
    let global_station_get_proxy = proxy.clone();
    let global_station_set_proxy = proxy.clone();

    Router::new()
        .route(
            API_V1_SESSION_OVERRIDES,
            get(move || list_session_manual_overrides(session_overrides_proxy.clone())).post(
                move |payload| {
                    apply_session_manual_overrides(session_overrides_apply_proxy.clone(), payload)
                },
            ),
        )
        .route(
            API_V1_SESSION_OVERRIDE_PROFILE,
            post(move |payload| apply_session_profile(session_profile_proxy.clone(), payload)),
        )
        .route(
            API_V1_SESSION_OVERRIDE_MODEL,
            get(move || list_session_model_overrides(session_model_list_proxy.clone())).post(
                move |payload| set_session_model_override(session_model_set_proxy.clone(), payload),
            ),
        )
        .route(
            API_V1_SESSION_OVERRIDE_EFFORT,
            get(move || list_session_reasoning_effort_overrides(session_effort_list_proxy.clone()))
                .post(move |payload| {
                    set_session_reasoning_effort_override(session_effort_set_proxy.clone(), payload)
                }),
        )
        .route(
            API_V1_SESSION_OVERRIDE_STATION,
            get(move || list_session_station_overrides(session_station_list_proxy.clone())).post(
                move |payload| {
                    set_session_station_override(session_station_set_proxy.clone(), payload)
                },
            ),
        )
        .route(
            API_V1_SESSION_OVERRIDE_SERVICE_TIER,
            get(move || {
                list_session_service_tier_overrides(session_service_tier_list_proxy.clone())
            })
            .post(move |payload| {
                set_session_service_tier_override(session_service_tier_set_proxy.clone(), payload)
            }),
        )
        .route(
            API_V1_SESSION_OVERRIDE_RESET,
            post(move |payload| {
                reset_session_manual_overrides(session_reset_proxy.clone(), payload)
            }),
        )
        .route(
            API_V1_GLOBAL_STATION_OVERRIDE,
            get(move || get_global_station_override(global_station_get_proxy.clone())).post(
                move |payload| {
                    set_global_station_override(global_station_set_proxy.clone(), payload)
                },
            ),
        )
}

fn healthcheck_routes(proxy: ProxyService) -> Router {
    let start_proxy = proxy.clone();

    Router::new()
        .route(
            API_V1_HEALTHCHECK_START,
            post(move |payload| start_health_checks(start_proxy.clone(), payload)),
        )
        .route(
            API_V1_HEALTHCHECK_CANCEL,
            post(move |payload| cancel_health_checks(proxy.clone(), payload)),
        )
}
