use super::*;
use crate::proxy::tests::harness::{spawn_proxy_service, upstream_config};

#[tokio::test]
async fn runtime_shutdown_endpoint_requests_shutdown_when_available() {
    let cfg = make_proxy_config(
        vec![upstream_config("http://127.0.0.1:9/v1")],
        RetryConfig::default(),
    );
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let proxy = ProxyService::new_with_v4_source_and_shutdown(
        Client::new(),
        Arc::new(cfg),
        None,
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
        Some(shutdown_tx),
    );
    let server = spawn_proxy_service(proxy);
    let client = reqwest::Client::new();

    let response = client
        .post(server.url("/__codex_helper/api/v1/runtime/shutdown"))
        .send()
        .await
        .expect("send shutdown request")
        .error_for_status()
        .expect("shutdown status")
        .json::<serde_json::Value>()
        .await
        .expect("shutdown json");

    assert_eq!(response["accepted"].as_bool(), Some(true));
    shutdown_rx
        .changed()
        .await
        .expect("shutdown signal should be delivered");
    assert!(*shutdown_rx.borrow());
}

#[tokio::test]
async fn runtime_shutdown_endpoint_returns_unavailable_without_runtime_owner() {
    let cfg = make_proxy_config(
        vec![upstream_config("http://127.0.0.1:9/v1")],
        RetryConfig::default(),
    );
    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let server = spawn_proxy_service(proxy);
    let client = reqwest::Client::new();

    let response = client
        .post(server.url("/__codex_helper/api/v1/runtime/shutdown"))
        .send()
        .await
        .expect("send shutdown request");

    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    let body = response
        .json::<serde_json::Value>()
        .await
        .expect("shutdown unavailable body");
    assert_eq!(
        body["code"].as_str(),
        Some("admin_runtime_shutdown_unavailable")
    );
    assert_eq!(
        body["message"].as_str(),
        Some("runtime shutdown is not available for this proxy instance")
    );
    assert_eq!(body["retryable"].as_bool(), Some(true));
}
