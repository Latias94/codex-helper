use axum::Json;
use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::http::{HeaderMap, Uri};
use axum::middleware;
use axum::routing::MethodFilter;
use axum::routing::{any, get, on};

use super::ProxyService;
use super::admin::{ProxyAdminDiscovery, reject_admin_paths_from_proxy, require_admin_path_only};
use super::control_plane_routes::control_plane_routes;
use super::handle_proxy;
use super::openai_images::{handle_openai_images_edits, handle_openai_images_generations};
use super::responses_websocket::handle_responses_websocket;

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

    let proxy_for_fallback = proxy.clone();
    router
        .route(
            "/images/generations",
            on(MethodFilter::POST, {
                let proxy = proxy.clone();
                move |req| handle_openai_images_generations(proxy.clone(), req)
            }),
        )
        .route(
            "/v1/images/generations",
            on(MethodFilter::POST, {
                let proxy = proxy.clone();
                move |req| handle_openai_images_generations(proxy.clone(), req)
            }),
        )
        .route(
            "/images/edits",
            on(MethodFilter::POST, {
                let proxy = proxy.clone();
                move |req| handle_openai_images_edits(proxy.clone(), req)
            }),
        )
        .route(
            "/v1/images/edits",
            on(MethodFilter::POST, {
                let proxy = proxy.clone();
                move |req| handle_openai_images_edits(proxy.clone(), req)
            }),
        )
        .route("/responses", responses_websocket_route(proxy.clone()))
        .route("/v1/responses", responses_websocket_route(proxy.clone()))
        .route(
            "/backend-api/codex/responses",
            responses_websocket_route(proxy),
        )
        .route(
            "/{*path}",
            any(move |req| handle_proxy(proxy_for_fallback.clone(), req)),
        )
        .layer(middleware::from_fn(reject_admin_paths_from_proxy))
}

fn responses_websocket_route(proxy: ProxyService) -> axum::routing::MethodRouter {
    let proxy_for_ws = proxy.clone();
    let proxy_for_fallback = proxy;
    on(
        MethodFilter::GET,
        move |ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
              headers: HeaderMap,
              uri: Uri| {
            handle_responses_websocket(proxy_for_ws.clone(), ws, headers, uri)
        },
    )
    .fallback(move |req| handle_proxy(proxy_for_fallback.clone(), req))
}

pub fn admin_listener_router(proxy: ProxyService) -> Router {
    router(proxy).layer(middleware::from_fn(require_admin_path_only))
}
