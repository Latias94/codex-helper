use axum::Router;
use axum::middleware;
use axum::routing::get;

use super::ProxyService;
use super::admin::{AdminAccessConfig, require_admin_access};
use super::control_plane::api_operator_read_model;
use super::control_plane_manifest::{API_V1_OPERATOR_READ_MODEL, API_V1_REQUEST_LEDGER_CHAIN};
use super::runtime_admin_api::get_request_ledger_chain;

pub(super) fn control_plane_routes(proxy: ProxyService) -> Router {
    let admin_access = AdminAccessConfig::from_env();
    let read_model_proxy = proxy.clone();

    Router::new()
        .route(
            API_V1_OPERATOR_READ_MODEL,
            get(move || api_operator_read_model(read_model_proxy.clone())),
        )
        .route(
            API_V1_REQUEST_LEDGER_CHAIN,
            get(move |query| get_request_ledger_chain(proxy.clone(), query)),
        )
        .layer(middleware::from_fn_with_state(
            admin_access,
            require_admin_access,
        ))
}
