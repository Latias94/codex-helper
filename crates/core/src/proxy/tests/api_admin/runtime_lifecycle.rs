use super::*;
use crate::proxy::tests::harness::upstream_config;

#[tokio::test]
async fn runtime_shutdown_mutation_route_is_absent() {
    let cfg = make_helper_config(
        vec![upstream_config("http://127.0.0.1:9/v1")],
        RetryConfig::default(),
    );
    let proxy = ProxyService::new(Client::new(), Arc::new(cfg), "codex");
    let app = crate::proxy::router(proxy);
    let mut request = Request::builder()
        .method("POST")
        .uri("/__codex_helper/api/v1/runtime/shutdown")
        .body(Body::empty())
        .expect("build shutdown request");
    request
        .extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            42_114,
        ))));

    let response = app.oneshot(request).await.expect("shutdown response");

    assert!(matches!(
        response.status(),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ));
}
