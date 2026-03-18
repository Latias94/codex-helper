use super::*;

pub(super) fn healthcheck_routes(proxy: ProxyService) -> Router {
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
