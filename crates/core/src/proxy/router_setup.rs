use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::http::{HeaderMap, Uri};
use axum::middleware;
use axum::routing::MethodFilter;
use axum::routing::{any, on};

use super::ProxyService;
use super::admin::{reject_admin_paths_from_proxy, require_admin_path_only};
use super::control_plane_routes::control_plane_routes;
use super::handle_proxy;
use super::local_operator_routes::local_operator_routes;
use super::openai_images::{handle_openai_images_edits, handle_openai_images_generations};
use super::responses_websocket::handle_responses_websocket;

pub(crate) fn router(proxy: ProxyService) -> Router {
    // In axum 0.8, wildcard segments use `/{*path}` (equivalent to `/*path` from axum 0.7).
    let proxy_routes = proxy_only_router(proxy.clone());

    Router::new()
        .merge(control_plane_routes(proxy))
        .merge(proxy_routes)
}

pub(crate) fn proxy_only_router(proxy: ProxyService) -> Router {
    let proxy_for_fallback = proxy.clone();
    Router::new()
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

pub(crate) fn admin_listener_router(proxy: ProxyService) -> Router {
    router(proxy.clone())
        .merge(local_operator_routes(proxy))
        .layer(middleware::from_fn(require_admin_path_only))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Json;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use reqwest::Client;
    use tower::ServiceExt;

    use super::*;
    use crate::config::{HelperConfig, ProviderConfig, RouteGraphConfig, ServiceRouteConfig};
    use crate::proxy::{
        LOCAL_V1_BALANCE_REFRESH, LOCAL_V1_CREDENTIAL_REFRESH, LOCAL_V1_DEFAULT_PROFILE_MUTATION,
        LOCAL_V1_OPERATOR_SESSION, LOCAL_V1_RELAY_CAPABILITIES, LOCAL_V1_RELAY_LIVE_SMOKE,
        LOCAL_V1_ROUTING_MUTATION, LOCAL_V1_RUNTIME_RELOAD, LOCAL_V1_SESSION_AFFINITY_MUTATION,
        LOCAL_V1_SESSION_BINDING_MUTATION,
    };

    fn proxy_with_upstream(base_url: String) -> ProxyService {
        let config = HelperConfig {
            codex: ServiceRouteConfig {
                providers: std::collections::BTreeMap::from([(
                    "test".to_string(),
                    ProviderConfig {
                        base_url: Some(base_url),
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::ordered_failover(vec!["test".to_string()])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        ProxyService::new(
            Client::builder()
                .no_proxy()
                .build()
                .expect("build upstream client"),
            Arc::new(config),
            "codex",
        )
    }

    #[tokio::test]
    async fn missing_admin_discovery_is_not_forwarded_upstream() {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let hits = upstream_hits.clone();
        let upstream = Router::new().fallback(move || {
            let hits = hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({
                    "api_version": 1,
                    "service_name": "codex",
                    "admin_base_url": "http://127.0.0.1:65535"
                }))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind hostile upstream");
        let upstream_addr = listener.local_addr().expect("hostile upstream address");
        let upstream_handle = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve hostile upstream");
        });
        let app = proxy_only_router(proxy_with_upstream(format!("http://{upstream_addr}")));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/codex-helper-admin")
                    .body(Body::empty())
                    .expect("build discovery request"),
            )
            .await
            .expect("discovery response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
        upstream_handle.abort();
    }

    #[tokio::test]
    async fn public_proxy_rejects_local_operator_paths_without_forwarding() {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let hits = upstream_hits.clone();
        let upstream = Router::new().fallback(move || {
            let hits = hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind hostile upstream");
        let upstream_addr = listener.local_addr().expect("hostile upstream address");
        let upstream_handle = tokio::spawn(async move {
            axum::serve(listener, upstream)
                .await
                .expect("serve hostile upstream");
        });
        let app = proxy_only_router(proxy_with_upstream(format!("http://{upstream_addr}")));

        for path in [
            LOCAL_V1_OPERATOR_SESSION,
            LOCAL_V1_BALANCE_REFRESH,
            LOCAL_V1_CREDENTIAL_REFRESH,
            LOCAL_V1_ROUTING_MUTATION,
            LOCAL_V1_SESSION_AFFINITY_MUTATION,
            LOCAL_V1_SESSION_BINDING_MUTATION,
            LOCAL_V1_DEFAULT_PROFILE_MUTATION,
            LOCAL_V1_RUNTIME_RELOAD,
            LOCAL_V1_RELAY_CAPABILITIES,
            LOCAL_V1_RELAY_LIVE_SMOKE,
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .body(Body::empty())
                        .expect("build local operator request"),
                )
                .await
                .expect("local operator rejection");
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "path={path}");
        }
        assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
        upstream_handle.abort();
    }

    #[tokio::test]
    async fn admin_listener_rejects_unsigned_local_operator_actions() {
        let app = admin_listener_router(proxy_with_upstream("http://127.0.0.1:1".to_string()));
        for path in [
            LOCAL_V1_BALANCE_REFRESH,
            LOCAL_V1_CREDENTIAL_REFRESH,
            LOCAL_V1_DEFAULT_PROFILE_MUTATION,
            LOCAL_V1_RUNTIME_RELOAD,
            LOCAL_V1_RELAY_CAPABILITIES,
            LOCAL_V1_RELAY_LIVE_SMOKE,
        ] {
            let mut request = Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"force":true}"#))
                .expect("build local operator request");
            request.extensions_mut().insert(axum::extract::ConnectInfo(
                "127.0.0.1:32100"
                    .parse::<std::net::SocketAddr>()
                    .expect("loopback address"),
            ));
            let response = app
                .clone()
                .oneshot(request)
                .await
                .expect("unsigned response");

            assert_eq!(response.status(), StatusCode::FORBIDDEN, "path={path}");
        }
    }
}
