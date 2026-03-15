use axum::Json;
use axum::Router;
use axum::middleware;
use axum::routing::{any, get, post, put};

use super::ProxyService;
use super::admin::{
    AdminAccessConfig, ProxyAdminDiscovery, reject_admin_paths_from_proxy, require_admin_access,
    require_admin_path_only,
};
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
use super::handle_proxy;
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

pub fn router(proxy: ProxyService) -> Router {
    // In axum 0.8, wildcard segments use `/{*path}` (equivalent to `/*path` from axum 0.7).
    let admin_access = AdminAccessConfig::from_env();

    let p2 = proxy.clone();
    let p8 = proxy.clone();
    let p9 = proxy.clone();
    let p10 = proxy.clone();
    let p11 = proxy.clone();
    let p12 = proxy.clone();
    let p13 = proxy.clone();
    let p14 = proxy.clone();
    let p15 = proxy.clone();
    let p16 = proxy.clone();
    let p17 = proxy.clone();
    let p18 = proxy.clone();
    let p19 = proxy.clone();
    let p20 = proxy.clone();
    let p21 = proxy.clone();
    let p22 = proxy.clone();
    let p23 = proxy.clone();
    let p24 = proxy.clone();
    let p25 = proxy.clone();
    let p26 = proxy.clone();
    let p27 = proxy.clone();
    let p28 = proxy.clone();
    let p29 = proxy.clone();
    let p30 = proxy.clone();
    let p31 = proxy.clone();
    let p32 = proxy.clone();
    let p33 = proxy.clone();
    let p35 = proxy.clone();
    let p36 = proxy.clone();
    let p37 = proxy.clone();
    let p38 = proxy.clone();
    let p39 = proxy.clone();
    let p40 = proxy.clone();
    let p41 = proxy.clone();
    let p42 = proxy.clone();
    let p43 = proxy.clone();
    let p44 = proxy.clone();
    let p45 = proxy.clone();
    let p46 = proxy.clone();
    let p47 = proxy.clone();
    let p48 = proxy.clone();
    let p49 = proxy.clone();
    let p50 = proxy.clone();
    let p51 = proxy.clone();
    let p52 = proxy.clone();
    let p53 = proxy.clone();
    let p54 = proxy.clone();
    let p55 = proxy.clone();
    let p56 = proxy.clone();

    let admin_routes = Router::new()
        // Versioned API (v1): attach-friendly, safe-by-default (no secrets).
        .route(
            API_V1_CAPABILITIES,
            get(move || api_capabilities(p8.clone())),
        )
        .route(
            API_V1_SNAPSHOT,
            get(move |q| api_v1_snapshot(p25.clone(), q)),
        )
        .route(
            API_V1_SESSIONS,
            get(move || list_session_identity_cards(p26.clone())),
        )
        .route(
            API_V1_SESSION_BY_ID,
            get(move |session_id| get_session_identity_card(p56.clone(), session_id)),
        )
        .route(
            API_V1_STATUS_ACTIVE,
            get(move || list_active_requests(p9.clone())),
        )
        .route(
            API_V1_STATUS_RECENT,
            get(move |q| list_recent_finished(p10.clone(), q)),
        )
        .route(
            API_V1_STATUS_SESSION_STATS,
            get(move || list_session_stats(p11.clone())),
        )
        .route(
            API_V1_STATUS_HEALTH_CHECKS,
            get(move || list_health_checks(p21.clone())),
        )
        .route(
            API_V1_STATUS_STATION_HEALTH,
            get(move || list_station_health(p22.clone())),
        )
        .route(
            API_V1_RUNTIME_STATUS,
            get(move || runtime_config_status(p12.clone())),
        )
        .route(
            API_V1_RUNTIME_RELOAD,
            post(move || reload_runtime_config(p13.clone())),
        )
        .route(
            API_V1_CONTROL_TRACE,
            get(move |q| get_control_trace(p14.clone(), q)),
        )
        .route(
            API_V1_RETRY_CONFIG,
            get(move || get_retry_config(p43.clone()))
                .post(move |payload| set_retry_config(p44.clone(), payload)),
        )
        .route(API_V1_STATIONS, get(move || list_stations(p35.clone())))
        .route(
            API_V1_STATIONS_RUNTIME,
            post(move |payload| apply_station_runtime_meta(p36.clone(), payload)),
        )
        .route(
            API_V1_STATIONS_CONFIG_ACTIVE,
            post(move |payload| set_persisted_active_station(p41.clone(), payload)),
        )
        .route(
            API_V1_STATIONS_PROBE,
            post(move |payload| probe_station(p51.clone(), payload)),
        )
        .route(
            API_V1_STATION_BY_NAME,
            put(move |name, payload| update_persisted_station(p42.clone(), name, payload)),
        )
        .route(
            API_V1_STATION_SPECS,
            get(move || list_persisted_station_specs(p37.clone())),
        )
        .route(
            API_V1_STATION_SPEC_BY_NAME,
            put(move |name, payload| upsert_persisted_station_spec(p45.clone(), name, payload))
                .delete(move |name| delete_persisted_station_spec(p46.clone(), name)),
        )
        .route(
            API_V1_PROVIDER_SPECS,
            get(move || list_persisted_provider_specs(p47.clone())),
        )
        .route(API_V1_PROVIDERS, get(move || list_providers(p54.clone())))
        .route(
            API_V1_PROVIDERS_RUNTIME,
            post(move |payload| apply_provider_runtime_meta(p55.clone(), payload)),
        )
        .route(
            API_V1_PROVIDER_SPEC_BY_NAME,
            put(move |name, payload| upsert_persisted_provider_spec(p48.clone(), name, payload))
                .delete(move |name| delete_persisted_provider_spec(p49.clone(), name)),
        )
        .route(API_V1_PROFILES, get(move || list_profiles(p31.clone())))
        .route(
            API_V1_PROFILES_DEFAULT,
            post(move |payload| set_default_profile(p33.clone(), payload)),
        )
        .route(
            API_V1_PROFILES_DEFAULT_PERSISTED,
            post(move |payload| set_persisted_default_profile(p38.clone(), payload)),
        )
        .route(
            API_V1_PROFILE_BY_NAME,
            put(move |name, payload| upsert_persisted_profile(p39.clone(), name, payload))
                .delete(move |name| delete_persisted_profile(p40.clone(), name)),
        )
        .route(
            API_V1_SESSION_OVERRIDES,
            get(move || list_session_manual_overrides(p52.clone()))
                .post(move |payload| apply_session_manual_overrides(p53.clone(), payload)),
        )
        .route(
            API_V1_SESSION_OVERRIDE_PROFILE,
            post(move |payload| apply_session_profile(p32.clone(), payload)),
        )
        .route(
            API_V1_SESSION_OVERRIDE_MODEL,
            get(move || list_session_model_overrides(p15.clone()))
                .post(move |payload| set_session_model_override(p16.clone(), payload)),
        )
        .route(
            API_V1_SESSION_OVERRIDE_EFFORT,
            get(move || list_session_reasoning_effort_overrides(p17.clone()))
                .post(move |payload| set_session_reasoning_effort_override(p18.clone(), payload)),
        )
        .route(
            API_V1_SESSION_OVERRIDE_STATION,
            get(move || list_session_station_overrides(p19.clone()))
                .post(move |payload| set_session_station_override(p20.clone(), payload)),
        )
        .route(
            API_V1_SESSION_OVERRIDE_SERVICE_TIER,
            get(move || list_session_service_tier_overrides(p23.clone()))
                .post(move |payload| set_session_service_tier_override(p24.clone(), payload)),
        )
        .route(
            API_V1_SESSION_OVERRIDE_RESET,
            post(move |payload| reset_session_manual_overrides(p50.clone(), payload)),
        )
        .route(
            API_V1_GLOBAL_STATION_OVERRIDE,
            get(move || get_global_station_override(p27.clone()))
                .post(move |payload| set_global_station_override(p28.clone(), payload)),
        )
        .route(
            API_V1_HEALTHCHECK_START,
            post(move |payload| start_health_checks(p29.clone(), payload)),
        )
        .route(
            API_V1_HEALTHCHECK_CANCEL,
            post(move |payload| cancel_health_checks(p30.clone(), payload)),
        )
        .layer(middleware::from_fn_with_state(
            admin_access,
            require_admin_access,
        ));

    Router::new()
        .merge(admin_routes)
        .merge(proxy_only_router(p2))
}

pub fn proxy_only_router(proxy: ProxyService) -> Router {
    proxy_only_router_with_admin_base_url(proxy, None)
}

pub fn proxy_only_router_with_admin_base_url(
    proxy: ProxyService,
    admin_base_url: Option<String>,
) -> Router {
    let service_name = proxy.service_name;
    let discovery = admin_base_url.map(|admin_base_url| {
        Json(ProxyAdminDiscovery {
            api_version: 1,
            service_name,
            admin_base_url,
        })
    });

    let mut router = Router::new();
    if let Some(discovery) = discovery {
        router = router.route(
            "/.well-known/codex-helper-admin",
            get(move || {
                let discovery = discovery.clone();
                async move { discovery }
            }),
        );
    }

    router
        .route("/{*path}", any(move |req| handle_proxy(proxy.clone(), req)))
        .layer(middleware::from_fn(reject_admin_paths_from_proxy))
}

pub fn admin_listener_router(proxy: ProxyService) -> Router {
    router(proxy).layer(middleware::from_fn(require_admin_path_only))
}
