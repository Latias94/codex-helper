use super::*;

pub(super) fn station_routes(proxy: ProxyService) -> Router {
    let stations_proxy = proxy.clone();
    let runtime_proxy = proxy.clone();
    let active_proxy = proxy.clone();
    let active_legacy_proxy = proxy.clone();
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
            API_V1_STATIONS_ACTIVE,
            post(move |payload| set_persisted_active_station(active_proxy.clone(), payload)),
        )
        .route(
            API_V1_STATIONS_CONFIG_ACTIVE_LEGACY,
            post(move |payload| set_persisted_active_station(active_legacy_proxy.clone(), payload)),
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
