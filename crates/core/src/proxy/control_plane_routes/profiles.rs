use super::*;

pub(super) fn profile_routes(proxy: ProxyService) -> Router {
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
