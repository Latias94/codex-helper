use super::*;
use crate::dashboard_core::{OperatorReadModel, OperatorReadStatus};
use crate::state::SessionObservationScope;

#[tokio::test]
async fn operator_read_model_reports_redacted_session_from_request_context() {
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

    let cfg = make_helper_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
            tags: HashMap::from([("provider_id".to_string(), "u1".to_string())]),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    let proxy = ProxyService::new(Client::new(), Arc::new(cfg), "codex");
    let state = proxy.state.clone();
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
    response.bytes().await.expect("responses body");

    let identity_cards = state.list_session_identity_cards(10).await;
    assert_eq!(identity_cards.len(), 1);
    assert_eq!(identity_cards[0].session_id.as_deref(), Some("sid-client"));
    assert_eq!(
        identity_cards[0].last_client_name.as_deref(),
        Some("Frank-Desk")
    );
    assert_eq!(
        identity_cards[0].last_client_addr.as_deref(),
        Some("127.0.0.1")
    );
    assert_eq!(
        identity_cards[0].observation_scope,
        SessionObservationScope::ObservedOnly
    );

    let model = client
        .get(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/operator/read-model"
        ))
        .send()
        .await
        .expect("operator read model send")
        .error_for_status()
        .expect("operator read model status")
        .json::<OperatorReadModel>()
        .await
        .expect("operator read model json");
    assert_eq!(model.status, OperatorReadStatus::Ready);
    assert!(model.validate().is_ok());
    let sessions = &model
        .data
        .as_ref()
        .expect("ready operator data")
        .summary
        .sessions;
    assert_eq!(sessions.len(), 1);
    let expected_session_key =
        crate::dashboard_core::operator_summary::operator_session_key("sid-client");
    assert_eq!(
        sessions[0].session_key, expected_session_key,
        "wire session identity must use the canonical opaque key"
    );
    assert_eq!(sessions[0].last_status, Some(200));
    let serialized_session =
        serde_json::to_string(&sessions[0]).expect("serialize operator session summary");
    assert!(!serialized_session.contains("sid-client"));
    assert!(!serialized_session.contains("Frank-Desk"));
    assert!(!serialized_session.contains("last_client_name"));
    assert!(!serialized_session.contains("last_client_addr"));
    assert!(!serialized_session.contains("observation_scope"));

    proxy_handle.abort();
    upstream_handle.abort();
}
