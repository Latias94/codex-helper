use axum::Json;
use axum::Router;
use axum::middleware;
use axum::routing::{any, get};

use super::ProxyService;
use super::admin::{ProxyAdminDiscovery, reject_admin_paths_from_proxy, require_admin_path_only};
use super::control_plane_routes::control_plane_routes;
use super::handle_proxy;

pub fn router(proxy: ProxyService) -> Router {
    // In axum 0.8, wildcard segments use `/{*path}` (equivalent to `/*path` from axum 0.7).
    let proxy_routes = proxy_only_router(proxy.clone());

    Router::new()
        .merge(control_plane_routes(proxy))
        .merge(proxy_routes)
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
