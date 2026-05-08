use super::*;

pub(super) fn routing_routes(proxy: ProxyService) -> Router {
    let list_proxy = proxy.clone();

    Router::new().route(
        API_V1_ROUTING,
        get(move || list_persisted_routing_spec(list_proxy.clone()))
            .put(move |payload| upsert_persisted_routing_spec(proxy.clone(), payload)),
    )
}
