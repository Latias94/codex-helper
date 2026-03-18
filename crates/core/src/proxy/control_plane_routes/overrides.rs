use super::*;

pub(super) fn override_routes(proxy: ProxyService) -> Router {
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
