use super::*;

#[tokio::test]
async fn proxy_api_v1_sessions_report_client_identity_from_request_context() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": "resp_test",
                    "output": [],
                })),
            )
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
            tags: HashMap::from([("provider_id".to_string(), "u1".to_string())]),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = Client::new();
    let response = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session_id", "sid-client")
        .header(super::CLIENT_NAME_HEADER, "Frank-Desk")
        .header("user-agent", "Codex CLI/0.1")
        .body(r#"{"input":"hi"}"#)
        .send()
        .await
        .expect("responses send");
    assert_eq!(response.status(), StatusCode::OK);

    let sessions = client
        .get(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/sessions"
        ))
        .send()
        .await
        .expect("sessions send")
        .error_for_status()
        .expect("sessions status")
        .json::<serde_json::Value>()
        .await
        .expect("sessions json");
    let sessions = sessions.as_array().expect("sessions array");
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0]
            .get("session_id")
            .and_then(|value| value.as_str()),
        Some("sid-client")
    );
    assert_eq!(
        sessions[0]
            .get("last_client_name")
            .and_then(|value| value.as_str()),
        Some("Frank-Desk")
    );
    assert_eq!(
        sessions[0]
            .get("last_client_addr")
            .and_then(|value| value.as_str()),
        Some("127.0.0.1")
    );
    assert_eq!(
        sessions[0]
            .get("observation_scope")
            .and_then(|value| value.as_str()),
        Some("observed_only")
    );

    proxy_handle.abort();
    upstream_handle.abort();
}
