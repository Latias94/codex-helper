use super::*;

pub(super) fn status_and_runtime_routes(proxy: ProxyService) -> Router {
    let active_proxy = proxy.clone();
    let recent_proxy = proxy.clone();
    let stats_proxy = proxy.clone();
    let health_proxy = proxy.clone();
    let station_health_proxy = proxy.clone();
    let runtime_status_proxy = proxy.clone();
    let runtime_reload_proxy = proxy.clone();
    let control_trace_proxy = proxy.clone();
    let retry_get_proxy = proxy.clone();
    let pricing_proxy = proxy.clone();

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
            get(move || runtime_status(runtime_status_proxy.clone())),
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
        .route(
            API_V1_PRICING_CATALOG,
            get(move || get_pricing_catalog(pricing_proxy.clone())),
        )
}
