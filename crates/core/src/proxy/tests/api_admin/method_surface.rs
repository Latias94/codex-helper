use axum::http::Method;

use super::*;

#[tokio::test]
async fn proxy_control_plane_exposes_only_get_and_head_routes() {
    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(make_helper_config(Vec::new(), RetryConfig::default())),
        "codex",
    );
    let app = crate::proxy::router(proxy);
    let loopback = std::net::SocketAddr::from(([127, 0, 0, 1], 42_111));
    let control_plane_paths = [
        "/__codex_helper/api/v1/capabilities",
        "/__codex_helper/api/v1/snapshot",
        "/__codex_helper/api/v1/fleet/snapshot",
        "/__codex_helper/api/v1/operator/read-model",
        "/__codex_helper/api/v1/operator/summary",
        "/__codex_helper/api/v1/sessions",
        "/__codex_helper/api/v1/sessions/test",
        "/__codex_helper/api/v1/status/active",
        "/__codex_helper/api/v1/status/recent",
        "/__codex_helper/api/v1/status/session-stats",
        "/__codex_helper/api/v1/status/health-checks",
        "/__codex_helper/api/v1/status/station-health",
        "/__codex_helper/api/v1/runtime/status",
        "/__codex_helper/api/v1/request-ledger/recent",
        "/__codex_helper/api/v1/request-ledger/summary",
        "/__codex_helper/api/v1/request-ledger/chain",
        "/__codex_helper/api/v1/control-trace",
        "/__codex_helper/api/v1/retry/config",
        "/__codex_helper/api/v1/pricing/catalog",
        "/__codex_helper/api/v1/routing",
        "/__codex_helper/api/v1/routing/explain",
        "/__codex_helper/api/v1/stations",
        "/__codex_helper/api/v1/stations/specs",
        "/__codex_helper/api/v1/providers",
        "/__codex_helper/api/v1/providers/specs",
        "/__codex_helper/api/v1/profiles",
        "/__codex_helper/api/v1/overrides/session",
        "/__codex_helper/api/v1/overrides/session/model",
        "/__codex_helper/api/v1/overrides/session/effort",
        "/__codex_helper/api/v1/overrides/session/station",
        "/__codex_helper/api/v1/overrides/session/route",
        "/__codex_helper/api/v1/overrides/session/service-tier",
        "/__codex_helper/api/v1/overrides/global-station",
        "/__codex_helper/api/v1/overrides/global-route",
        "/__codex_helper/api/v1/codex/relay-capabilities",
        "/__codex_helper/api/v1/codex/relay-live-smoke",
        "/__codex_helper/api/v1/runtime/reload",
        "/__codex_helper/api/v1/runtime/shutdown",
        "/__codex_helper/api/v1/stations/runtime",
        "/__codex_helper/api/v1/stations/active",
        "/__codex_helper/api/v1/stations/probe",
        "/__codex_helper/api/v1/stations/test",
        "/__codex_helper/api/v1/stations/specs/test",
        "/__codex_helper/api/v1/providers/runtime",
        "/__codex_helper/api/v1/providers/balances/refresh",
        "/__codex_helper/api/v1/providers/specs/test",
        "/__codex_helper/api/v1/profiles/default",
        "/__codex_helper/api/v1/profiles/default/persisted",
        "/__codex_helper/api/v1/profiles/test",
        "/__codex_helper/api/v1/overrides/session/profile",
        "/__codex_helper/api/v1/overrides/session/reset",
        "/__codex_helper/api/v1/healthcheck/start",
        "/__codex_helper/api/v1/healthcheck/cancel",
        "/__codex_helper/local/v1/operator/credentials/refresh",
    ];

    let non_read_methods = [
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
        Method::CONNECT,
        Method::TRACE,
        Method::from_bytes(b"PROPFIND").expect("custom method"),
    ];

    for path in control_plane_paths {
        for method in &non_read_methods {
            let mut request = Request::builder()
                .method(method.clone())
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .expect("build mutation request");
            request.extensions_mut().insert(ConnectInfo(loopback));

            let response = app
                .clone()
                .oneshot(request)
                .await
                .expect("mutation response");

            assert!(
                matches!(
                    response.status(),
                    StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
                ),
                "{method} {path} remained routable with status {}",
                response.status()
            );
        }
    }
}

#[tokio::test]
async fn control_plane_read_surface_matches_canonical_allowlist() {
    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(make_helper_config(Vec::new(), RetryConfig::default())),
        "codex",
    );
    let app = crate::proxy::router(proxy);
    let loopback = std::net::SocketAddr::from(([127, 0, 0, 1], 42_111));
    let canonical_paths = [
        "/__codex_helper/api/v1/operator/read-model",
        "/__codex_helper/api/v1/request-ledger/chain",
    ];
    let removed_paths = [
        "/__codex_helper/api/v1/capabilities",
        "/__codex_helper/api/v1/snapshot",
        "/__codex_helper/api/v1/fleet/snapshot",
        "/__codex_helper/api/v1/operator/summary",
        "/__codex_helper/api/v1/sessions",
        "/__codex_helper/api/v1/sessions/test",
        "/__codex_helper/api/v1/status/active",
        "/__codex_helper/api/v1/status/recent",
        "/__codex_helper/api/v1/status/session-stats",
        "/__codex_helper/api/v1/status/health-checks",
        "/__codex_helper/api/v1/status/station-health",
        "/__codex_helper/api/v1/runtime/status",
        "/__codex_helper/api/v1/request-ledger/recent",
        "/__codex_helper/api/v1/request-ledger/summary",
        "/__codex_helper/api/v1/control-trace",
        "/__codex_helper/api/v1/retry/config",
        "/__codex_helper/api/v1/pricing/catalog",
        "/__codex_helper/api/v1/routing",
        "/__codex_helper/api/v1/routing/explain",
        "/__codex_helper/api/v1/stations",
        "/__codex_helper/api/v1/providers",
        "/__codex_helper/api/v1/providers/specs",
        "/__codex_helper/api/v1/profiles",
        "/__codex_helper/api/v1/overrides/session",
        "/__codex_helper/api/v1/overrides/session/model",
        "/__codex_helper/api/v1/overrides/session/effort",
        "/__codex_helper/api/v1/overrides/session/station",
        "/__codex_helper/api/v1/overrides/session/route",
        "/__codex_helper/api/v1/overrides/session/service-tier",
        "/__codex_helper/api/v1/overrides/global-station",
        "/__codex_helper/api/v1/overrides/global-route",
        "/__codex_helper/local/v1/operator/credentials/refresh",
    ];

    for path in canonical_paths {
        for method in [Method::GET, Method::HEAD] {
            let mut request = Request::builder()
                .method(method.clone())
                .uri(path)
                .body(Body::empty())
                .expect("build canonical read request");
            request.extensions_mut().insert(ConnectInfo(loopback));

            let response = app
                .clone()
                .oneshot(request)
                .await
                .expect("canonical read response");

            assert!(
                !matches!(
                    response.status(),
                    StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
                ),
                "{method} {path} is not available: {}",
                response.status()
            );
        }
    }

    for path in removed_paths {
        for method in [Method::GET, Method::HEAD] {
            let mut request = Request::builder()
                .method(method.clone())
                .uri(path)
                .body(Body::empty())
                .expect("build raw read request");
            request.extensions_mut().insert(ConnectInfo(loopback));

            let response = app
                .clone()
                .oneshot(request)
                .await
                .expect("raw read response");

            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{method} {path} remained routable"
            );
        }
    }
}
