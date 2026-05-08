use super::*;

pub(super) fn provider_routes(proxy: ProxyService) -> Router {
    let specs_proxy = proxy.clone();
    let providers_proxy = proxy.clone();
    let runtime_proxy = proxy.clone();
    let balance_refresh_proxy = proxy.clone();
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
            API_V1_PROVIDERS_BALANCES_REFRESH,
            post(move |query| refresh_provider_balances(balance_refresh_proxy.clone(), query)),
        )
        .route(
            API_V1_PROVIDER_SPEC_BY_NAME,
            put(move |name, payload| {
                upsert_persisted_provider_spec(upsert_proxy.clone(), name, payload)
            })
            .delete(move |name| delete_persisted_provider_spec(proxy.clone(), name)),
        )
}
