use super::*;

#[path = "response_semantics_compact.rs"]
mod response_semantics_compact;
#[path = "response_semantics_websocket.rs"]
mod response_semantics_websocket;
use crate::logging::RouteAttemptLog;
use crate::proxy::tests::harness::{
    find_finished_request, post_compact_json, post_responses_json, proxy_service,
    spawn_proxy_service, spawn_test_proxy, spawn_test_upstream,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::{SessionIdentitySource, SessionRouteAffinityTarget};
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest as _, Sha256};
use std::io::Cursor;
use std::io::Write;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

fn reasoning_guard_retry_config(max_attempts: u32) -> RetryConfig {
    let mut retry = retry_config(max_attempts, "502", Vec::new(), RetryStrategy::SameUpstream);
    retry.reasoning_guard = Some(crate::config::ReasoningGuardConfig {
        enabled: Some(true),
        ..crate::config::ReasoningGuardConfig::default()
    });
    retry
}

fn local_http_test_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()
        .expect("build local HTTP test client")
}

fn request_local_models_retry_config(on_status: &str, on_class: Vec<String>) -> RetryConfig {
    RetryConfig {
        upstream: Some(retry_layer_config(
            1,
            on_status,
            on_class.clone(),
            RetryStrategy::SameUpstream,
        )),
        provider: Some(retry_layer_config(
            2,
            on_status,
            on_class,
            RetryStrategy::Failover,
        )),
        cloudflare_challenge_cooldown_secs: Some(30),
        cloudflare_timeout_cooldown_secs: Some(30),
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    }
}

fn request_local_models_config(upstreams: Vec<UpstreamConfig>, retry: RetryConfig) -> HelperConfig {
    let mut config = make_helper_config(upstreams, retry);
    config
        .codex
        .routing
        .as_mut()
        .expect("route graph")
        .affinity_policy = RouteAffinityPolicy::FallbackSticky;
    config
}

fn legacy_raw_bearer_account_fingerprint(token: &str) -> String {
    let value = format!("Bearer {token}");
    let mut digest = Sha256::new();
    digest.update(b"codex-helper:provider-account:v1\0");
    digest.update(b"authorization");
    digest.update([0]);
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
    format!("sha256:{:x}", digest.finalize())
}

fn official_openai_test_proxy_service(
    upstream: &crate::proxy::tests::harness::TestUpstreamServer,
    mut upstream_config: UpstreamConfig,
) -> ProxyService {
    upstream_config.base_url = format!("http://api.openai.com:{}/v1", upstream.addr.port());
    upstream_config.auth.allow_anonymous = Some(true);
    let client = crate::proxy::upstream_http_client_builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .resolve("api.openai.com", upstream.addr)
        .build()
        .expect("build official OpenAI test client");
    let config = make_helper_config(
        vec![upstream_config],
        retry_config(1, "", Vec::new(), RetryStrategy::Failover),
    );
    ProxyService::new(client, Arc::new(config), "codex")
}

#[tokio::test]
async fn proxy_official_sol_maps_ultra_to_max_from_captured_contract() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let seen_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let hits_for_route = upstream_hits.clone();
    let body_for_route = seen_body.clone();
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move |body: String| {
            let hits = hits_for_route.clone();
            let seen_body = body_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                *seen_body.lock().expect("seen body lock") =
                    Some(serde_json::from_str(&body).expect("upstream request JSON"));
                Json(serde_json::json!({
                    "id": "resp-official-sol",
                    "object": "response",
                    "model": "gpt-5.6-sol"
                }))
            }
        }),
    ));
    let proxy_service = official_openai_test_proxy_service(&upstream, upstream.upstream_config());
    let state = proxy_service.state.clone();
    let proxy = spawn_proxy_service(proxy_service);

    let response = post_responses_json(
        &local_http_test_client(),
        &proxy,
        r#"{"model":"gpt-5.6-sol","reasoning":{"effort":"ultra","mode":"pro","future_mode":"deliberate"},"future_request_field":true,"input":"hi"}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.expect("read proxy response");

    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);
    let body = seen_body
        .lock()
        .expect("seen body lock")
        .clone()
        .expect("captured upstream body");
    assert_eq!(body["model"].as_str(), Some("gpt-5.6-sol"));
    assert_eq!(body["reasoning"]["effort"].as_str(), Some("max"));
    assert_eq!(body["reasoning"]["mode"].as_str(), Some("pro"));
    assert_eq!(
        body["reasoning"]["future_mode"].as_str(),
        Some("deliberate")
    );
    assert_eq!(body["future_request_field"].as_bool(), Some(true));
    let finished = find_finished_request(&state, 10, |request| {
        request.model.as_deref() == Some("gpt-5.6-sol")
    })
    .await
    .expect("finished official Sol request");
    assert_eq!(finished.reasoning_effort.as_deref(), Some("max"));
}

#[tokio::test]
async fn official_passthrough_persists_keyed_account_identity_not_raw_header_digest() {
    const PASSTHROUGH_AUTH_CANARY: &str =
        "official-passthrough-fingerprint-canary-01f732-never-persist";
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            Json(serde_json::json!({
                "id": "resp-passthrough-account",
                "object": "response",
                "model": "gpt-5.6-sol"
            }))
        }),
    ));
    let service = official_openai_test_proxy_service(&upstream, upstream.upstream_config());
    let state = Arc::clone(&service.state);
    let runtime_store = state.runtime_store_handle();
    let proxy = spawn_proxy_service(service);

    let response = local_http_test_client()
        .post(proxy.responses_url())
        .header("authorization", format!("Bearer {PASSTHROUGH_AUTH_CANARY}"))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5.6-sol","input":"hi"}"#)
        .send()
        .await
        .expect("send official passthrough request");
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.expect("read passthrough response");

    let logical_request = runtime_store
        .read_recent_logical_requests(1)
        .expect("read durable passthrough request")
        .into_iter()
        .next()
        .expect("durable passthrough request");
    let attempts = runtime_store
        .read_attempts_for_logical_request(
            runtime_store.logical_request_handle(logical_request.request.id),
        )
        .expect("read durable passthrough attempt");
    let attempt = attempts.first().expect("durable passthrough attempt");
    let provider_epoch = attempt
        .attempt
        .evidence
        .provider_epoch
        .as_ref()
        .expect("passthrough provider epoch");
    let legacy_raw_fingerprint = legacy_raw_bearer_account_fingerprint(PASSTHROUGH_AUTH_CANARY);
    assert_ne!(
        provider_epoch.scope.account_fingerprint,
        legacy_raw_fingerprint
    );
    let pending_evidence = runtime_store
        .raw_attempt_pending_evidence_json_for_test(attempt.attempt.id)
        .expect("read raw SQLite pending evidence");
    let pending_evidence_value: serde_json::Value =
        serde_json::from_str(&pending_evidence).expect("parse raw SQLite pending evidence");
    assert_eq!(
        pending_evidence_value
            .pointer("/provider_epoch/scope/account_fingerprint")
            .and_then(serde_json::Value::as_str),
        Some(provider_epoch.scope.account_fingerprint.as_str())
    );
    assert!(!pending_evidence.contains(PASSTHROUGH_AUTH_CANARY));
    assert!(!pending_evidence.contains(&legacy_raw_fingerprint));
    assert_eq!(
        logical_request
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.terminal.payload.as_ref())
            .and_then(|payload| payload.provider_epoch.as_ref()),
        Some(provider_epoch)
    );
}

#[tokio::test]
async fn proxy_mapped_official_luna_rejects_ultra_before_upstream_write() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = upstream_hits.clone();
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        axum::routing::any(move || {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({ "unexpected": true }))
            }
        }),
    ));
    let mut upstream_config = upstream.upstream_config();
    upstream_config
        .model_mapping
        .insert("gpt-5.6-fast".to_string(), "gpt-5.6-luna".to_string());
    let proxy = spawn_proxy_service(official_openai_test_proxy_service(
        &upstream,
        upstream_config,
    ));

    let response = post_responses_json(
        &local_http_test_client(),
        &proxy,
        r#"{"model":"gpt-5.6-fast","reasoning":{"effort":"ultra"},"input":"hi"}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        response
            .text()
            .await
            .expect("read mapped Luna rejection")
            .contains("selected provider request contract does not support the ultra intent")
    );
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn proxy_compatible_sol_rejects_ultra_before_upstream_write() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = upstream_hits.clone();
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        axum::routing::any(move || {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({ "unexpected": true }))
            }
        }),
    ));
    let proxy = spawn_test_proxy(make_helper_config(
        vec![upstream.upstream_config()],
        retry_config(1, "", Vec::new(), RetryStrategy::Failover),
    ));

    let response = post_responses_json(
        &local_http_test_client(),
        &proxy,
        r#"{"model":"gpt-5.6-sol","reasoning":{"effort":"ultra"},"input":"hi"}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        response
            .text()
            .await
            .expect("read compatible Sol rejection")
            .contains("reasoning intent requires a captured provider request contract")
    );
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn proxy_withholds_success_when_logical_terminal_commit_fails() {
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": "resp-terminal-failure",
                    "object": "response"
                })),
            )
        }),
    ));
    let service = proxy_service(make_helper_config(
        vec![upstream.upstream_config()],
        retry_config(1, "", Vec::new(), RetryStrategy::Failover),
    ));
    let state = service.state.clone();
    state
        .runtime_store_handle()
        .fail_next_logical_terminal_commit_for_test();
    let proxy = spawn_proxy_service(service);

    let response = reqwest::Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("session-id", "sid-terminal-failure")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses request");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(state.list_recent_finished(10).await.is_empty());
    assert_eq!(state.list_active_requests().await.len(), 1);
    assert!(
        state
            .peek_session_route_affinity("sid-terminal-failure")
            .await
            .is_none()
    );

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_streaming_logical_terminal_failure_withholds_success_side_effects() {
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            let mut response = Response::new(Body::from(
                "event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-stream-terminal-failure\"}}\n\n\
data: [DONE]\n\n",
            ));
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            response
        }),
    ));
    let routing = RouteGraphConfig::ordered_failover(vec!["stream-provider".to_string()]);
    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([(
                "stream-provider".to_string(),
                ProviderConfig {
                    base_url: Some(upstream.upstream_config().base_url),
                    inline_auth: UpstreamAuth::default(),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let service = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = service.state.clone();
    let runtime_store = state.runtime_store_handle();
    runtime_store.fail_next_logical_terminal_commit_for_test();
    let proxy = spawn_proxy_service(service);

    let response = Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("session-id", "sid-stream-terminal-failure")
        .body(r#"{"model":"gpt-5","input":"hi","stream":true}"#)
        .send()
        .await
        .expect("send streaming response request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("drain streaming response");
    assert!(!body.contains("response.completed"), "{body}");
    assert!(!body.contains("[DONE]"), "{body}");
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(
        body.contains(r#""codex_helper_error":"terminal_commit_failed""#),
        "{body}"
    );

    let mut attempt_committed = false;
    for _ in 0..100 {
        let logical_requests = runtime_store
            .read_recent_logical_requests(10)
            .expect("read durable logical request");
        if let Some(logical_request) = logical_requests.first() {
            let logical_handle = runtime_store.logical_request_handle(logical_request.request.id);
            let attempts = runtime_store
                .read_attempts_for_logical_request(logical_handle)
                .expect("read durable streaming attempt");
            if attempts
                .first()
                .is_some_and(|attempt| attempt.terminal.is_some())
            {
                attempt_committed = true;
                break;
            }
        }
        sleep(Duration::from_millis(20)).await;
    }

    assert!(
        attempt_committed,
        "streaming attempt terminal should commit"
    );
    let logical_requests = runtime_store
        .read_recent_logical_requests(10)
        .expect("read durable logical request after failed terminal commit");
    assert_eq!(logical_requests.len(), 1);
    assert!(logical_requests[0].terminal.is_none());
    assert!(state.list_recent_finished(10).await.is_empty());
    assert!(
        state
            .peek_session_route_affinity("sid-stream-terminal-failure")
            .await
            .is_none()
    );

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_streaming_auth_failures_refresh_once_but_cloudflare_challenge_does_not() {
    for (status, cloudflare_challenge, expected_upstream_hits, expected_native_reads) in [
        (StatusCode::UNAUTHORIZED, false, 1, 2),
        (StatusCode::FORBIDDEN, false, 1, 2),
        (StatusCode::FORBIDDEN, true, 2, 1),
    ] {
        let upstream_hits = Arc::new(AtomicUsize::new(0));
        let hits_for_route = Arc::clone(&upstream_hits);
        let upstream = axum::Router::new().route(
            "/v1/responses",
            post(move || {
                let hits = Arc::clone(&hits_for_route);
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    if cloudflare_challenge {
                        axum::response::IntoResponse::into_response((
                            status,
                            [
                                (axum::http::header::CONTENT_TYPE, "text/html"),
                                (axum::http::header::SERVER, "cloudflare"),
                            ],
                            "<html><script src=\"/cdn-cgi/challenge-platform/x.js\"></script></html>",
                        ))
                    } else {
                        axum::response::IntoResponse::into_response((
                            status,
                            Json(serde_json::json!({ "error": "auth failed" })),
                        ))
                    }
                }
            }),
        );
        let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
        let source = HelperConfig {
            codex: ServiceRouteConfig {
                providers: std::collections::BTreeMap::from([(
                    "primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{upstream_addr}/v1")),
                        auth: UpstreamAuth {
                            auth_token_ref: Some(crate::config::CredentialRef::Native {
                                name: "relay.primary".to_string(),
                            }),
                            ..UpstreamAuth::default()
                        },
                        ..ProviderConfig::default()
                    },
                )]),
                routing: Some(RouteGraphConfig::ordered_failover(vec![
                    "primary".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        };
        let (credential_sources, credential_control) =
            crate::credentials::CredentialSourceCapabilities::test_native(
                crate::credentials::SecretValue::new(b"generation-a".to_vec())
                    .expect("valid initial credential"),
            );
        let runtime_store = Arc::new(
            crate::runtime_store::RuntimeStore::open_in_memory().expect("open runtime store"),
        );
        let proxy = ProxyService::new_with_runtime_store_and_credential_sources(
            Client::new(),
            Arc::new(source),
            "codex",
            runtime_store,
            credential_sources,
        )
        .expect("build credential-backed proxy");
        assert_eq!(credential_control.read_count(), 1);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let refresh_driver = proxy.spawn_credential_refresh_driver(shutdown_rx);
        let (proxy_addr, proxy_handle) = spawn_axum_server(crate::proxy::router(proxy));

        let response = local_http_test_client()
            .post(format!("http://{proxy_addr}/v1/responses"))
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(r#"{"model":"gpt-5","input":"hi","stream":true}"#)
            .send()
            .await
            .expect("send streaming auth response request");
        if !cloudflare_challenge {
            assert_eq!(response.status(), status);
        }
        let _ = response.bytes().await.expect("drain streaming auth body");

        if expected_native_reads == 2 {
            tokio::time::timeout(Duration::from_secs(2), async {
                while credential_control.read_count() < 2 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("authentication failure should schedule one credential refresh");
        } else {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(upstream_hits.load(Ordering::SeqCst), expected_upstream_hits);
        assert_eq!(
            credential_control.read_count(),
            expected_native_reads,
            "Cloudflare challenge pages are transport failures, not credential evidence"
        );

        shutdown_tx.send(true).expect("signal refresh shutdown");
        refresh_driver.await.expect("join refresh driver");
        proxy_handle.abort();
        upstream_handle.abort();
    }
}

#[tokio::test]
async fn proxy_sse_failed_terminal_commits_failure_before_forwarding() {
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            let mut response = Response::new(Body::from(
                "event: response.created\n\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-failed\"}}\n\n\
event: response.failed\n\
data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp-failed\",\"error\":{\"message\":\"upstream rejected\"}}}\n\n",
            ));
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            response
        }),
    ));
    let service = proxy_service(make_helper_config(
        vec![upstream.upstream_config()],
        retry_config(1, "", Vec::new(), RetryStrategy::Failover),
    ));
    let state = service.state.clone();
    let runtime_store = state.runtime_store_handle();
    let proxy = spawn_proxy_service(service);

    let response = Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("session-id", "sid-upstream-failed")
        .body(r#"{"model":"gpt-5","input":"hi","stream":true}"#)
        .send()
        .await
        .expect("send streaming failed response request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("drain failed terminal");
    assert!(body.contains("response.created"), "{body}");
    assert!(body.contains("response.failed"), "{body}");

    let logical_requests = runtime_store
        .read_recent_logical_requests(10)
        .expect("read durable failed logical request");
    assert_eq!(logical_requests.len(), 1);
    assert_eq!(
        logical_requests[0]
            .terminal
            .as_ref()
            .map(|terminal| terminal.terminal.outcome),
        Some(crate::runtime_store::LogicalRequestOutcome::Failed)
    );
    let attempts = runtime_store
        .read_attempts_for_logical_request(
            runtime_store.logical_request_handle(logical_requests[0].request.id),
        )
        .expect("read durable failed attempt");
    assert_eq!(
        attempts[0]
            .terminal
            .as_ref()
            .map(|terminal| terminal.terminal.outcome),
        Some(crate::runtime_store::AttemptOutcome::Failed)
    );
    let finished = state.list_recent_finished(10).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].status_code, StatusCode::BAD_GATEWAY.as_u16());
    assert!(
        state
            .peek_session_route_affinity("sid-upstream-failed")
            .await
            .is_none()
    );

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_sse_logical_failure_terminals_are_durable_but_health_neutral() {
    for event_type in ["response.incomplete", "response.cancelled"] {
        let event_body = format!(
            "event: response.created\ndata: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp-logical-failure\"}}}}\n\nevent: {event_type}\ndata: {{\"type\":\"{event_type}\"}}\n\n"
        );
        let upstream = spawn_test_upstream(axum::Router::new().route(
            "/v1/responses",
            post(move || {
                let event_body = event_body.clone();
                async move {
                    let mut response = Response::new(Body::from(event_body));
                    *response.status_mut() = StatusCode::OK;
                    response.headers_mut().insert(
                        axum::http::header::CONTENT_TYPE,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    response
                }
            }),
        ));
        let service = proxy_service(make_helper_config(
            vec![upstream.upstream_config()],
            retry_config(1, "", Vec::new(), RetryStrategy::Failover),
        ));
        let state = service.state.clone();
        let runtime_store = state.runtime_store_handle();
        let proxy = spawn_proxy_service(service);

        let response = Client::new()
            .post(proxy.responses_url())
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("session-id", format!("sid-{event_type}"))
            .body(r#"{"model":"gpt-5","input":"hi","stream":true}"#)
            .send()
            .await
            .expect("send streaming logical failure request");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .text()
            .await
            .expect("drain logical failure terminal");
        assert!(body.contains(event_type), "{body}");

        let logical_requests = runtime_store
            .read_recent_logical_requests(10)
            .expect("read durable logical failure request");
        assert_eq!(logical_requests.len(), 1);
        assert_eq!(
            logical_requests[0]
                .terminal
                .as_ref()
                .map(|terminal| terminal.terminal.outcome),
            Some(crate::runtime_store::LogicalRequestOutcome::Failed)
        );
        let attempts = runtime_store
            .read_attempts_for_logical_request(
                runtime_store.logical_request_handle(logical_requests[0].request.id),
            )
            .expect("read durable logical failure attempt");
        assert_eq!(
            attempts[0]
                .terminal
                .as_ref()
                .map(|terminal| terminal.terminal.outcome),
            Some(crate::runtime_store::AttemptOutcome::Failed)
        );
        assert_eq!(
            state.list_recent_finished(10).await[0].status_code,
            StatusCode::BAD_GATEWAY.as_u16()
        );

        let runtime = state
            .route_plan_runtime_state_for_provider_endpoints("codex")
            .await;
        let endpoint = runtime.provider_endpoint(
            &crate::runtime_identity::ProviderEndpointKey::new("codex", "test", "default"),
        );
        assert_eq!(endpoint.failure_count, 0, "{event_type}");
        assert!(!endpoint.cooldown_active, "{event_type}");

        proxy.handle.abort();
    }
}

#[tokio::test]
async fn proxy_non_sse_stream_withholds_json_when_terminal_commit_fails() {
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            let mut response = Response::new(Body::from(
                r#"{"id":"resp-json-stream","object":"response"}"#,
            ));
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
        }),
    ));
    let service = proxy_service(make_helper_config(
        vec![upstream.upstream_config()],
        retry_config(1, "", Vec::new(), RetryStrategy::Failover),
    ));
    let state = service.state.clone();
    let runtime_store = state.runtime_store_handle();
    runtime_store.fail_next_logical_terminal_commit_for_test();
    let proxy = spawn_proxy_service(service);

    let response = Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("session-id", "sid-json-stream-terminal-failure")
        .body(r#"{"model":"gpt-5","input":"hi","stream":true}"#)
        .send()
        .await;
    match response {
        Ok(response) => {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response
                    .headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some("application/json")
            );
            let error = response
                .bytes()
                .await
                .expect_err("terminal commit failure must interrupt the JSON response body");
            assert!(error.is_body() || error.is_request(), "{error:?}");
        }
        Err(error) => {
            assert!(error.is_body() || error.is_request(), "{error:?}");
        }
    }

    let logical_requests = runtime_store
        .read_recent_logical_requests(10)
        .expect("read durable JSON logical request");
    assert_eq!(logical_requests.len(), 1);
    assert!(logical_requests[0].terminal.is_none());
    let attempts = runtime_store
        .read_attempts_for_logical_request(
            runtime_store.logical_request_handle(logical_requests[0].request.id),
        )
        .expect("read durable JSON attempt");
    assert_eq!(
        attempts[0]
            .terminal
            .as_ref()
            .map(|terminal| terminal.terminal.outcome),
        Some(crate::runtime_store::AttemptOutcome::Succeeded)
    );
    assert!(state.list_recent_finished(10).await.is_empty());
    assert!(
        state
            .peek_session_route_affinity("sid-json-stream-terminal-failure")
            .await
            .is_none()
    );

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_downstream_disconnect_before_sse_terminal_commits_failure_without_health_success() {
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            let first = stream::once(async {
                Ok::<Bytes, Infallible>(Bytes::from_static(
                    b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-disconnect\"}}\n\n",
                ))
            });
            let stalled = stream::pending::<Result<Bytes, Infallible>>();
            let mut response = Response::new(Body::from_stream(first.chain(stalled)));
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            response
        }),
    ));
    let service = proxy_service(make_helper_config(
        vec![upstream.upstream_config()],
        retry_config(1, "", Vec::new(), RetryStrategy::Failover),
    ));
    let state = service.state.clone();
    let runtime_store = state.runtime_store_handle();
    let proxy = spawn_proxy_service(service);

    let mut response = Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("session-id", "sid-downstream-disconnect")
        .body(r#"{"model":"gpt-5","input":"hi","stream":true}"#)
        .send()
        .await
        .expect("send disconnect response request");
    let first = response
        .chunk()
        .await
        .expect("read first SSE chunk")
        .expect("first SSE chunk");
    assert!(String::from_utf8_lossy(&first).contains("response.created"));
    drop(response);

    let logical_request = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let requests = runtime_store
                .read_recent_logical_requests(10)
                .expect("read disconnected logical request");
            if let Some(request) = requests
                .into_iter()
                .find(|request| request.terminal.is_some())
            {
                break request;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("downstream disconnect should finalize durably");
    assert_eq!(
        logical_request
            .terminal
            .as_ref()
            .map(|terminal| terminal.terminal.outcome),
        Some(crate::runtime_store::LogicalRequestOutcome::Failed)
    );
    let attempts = runtime_store
        .read_attempts_for_logical_request(
            runtime_store.logical_request_handle(logical_request.request.id),
        )
        .expect("read disconnected attempt");
    assert_eq!(
        attempts[0]
            .terminal
            .as_ref()
            .map(|terminal| terminal.terminal.outcome),
        Some(crate::runtime_store::AttemptOutcome::Failed)
    );
    assert!(
        state
            .peek_session_route_affinity("sid-downstream-disconnect")
            .await
            .is_none()
    );

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_does_not_write_upstream_when_pending_attempt_insert_fails() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = hits.clone();
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
            }
        }),
    ));
    let service = proxy_service(make_helper_config(
        vec![upstream.upstream_config()],
        retry_config(1, "", Vec::new(), RetryStrategy::Failover),
    ));
    let state = service.state.clone();
    state
        .runtime_store_handle()
        .fail_next_attempt_begin_for_test();
    let proxy = spawn_proxy_service(service);

    let response = post_responses_json(
        &reqwest::Client::new(),
        &proxy,
        r#"{"model":"gpt-5","input":"hi"}"#,
    )
    .await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_does_not_fail_over_when_attempt_terminal_commit_fails() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let primary_hits_for_route = primary_hits.clone();
    let primary = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = primary_hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "primary failed" })),
                )
            }
        }),
    ));
    let backup_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits_for_route = backup_hits.clone();
    let backup = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = backup_hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
            }
        }),
    ));
    let service = proxy_service(make_helper_config(
        vec![primary.upstream_config(), backup.upstream_config()],
        retry_config(2, "500", Vec::new(), RetryStrategy::Failover),
    ));
    let state = service.state.clone();
    state
        .runtime_store_handle()
        .fail_next_attempt_terminal_commit_for_test();
    let proxy = spawn_proxy_service(service);

    let response = post_responses_json(
        &reqwest::Client::new(),
        &proxy,
        r#"{"model":"gpt-5","input":"hi"}"#,
    )
    .await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 0);
    assert!(state.list_recent_finished(10).await.is_empty());
    assert_eq!(state.list_active_requests().await.len(), 1);

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_commits_each_failover_attempt_before_logical_success() {
    const AUTH_FINGERPRINT_CANARY: &str = "durable-auth-fingerprint-canary-7b684f2d-never-persist";
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let primary_hits_for_route = primary_hits.clone();
    let primary = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = primary_hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "primary failed" })),
                )
            }
        }),
    ));
    let backup_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits_for_route = backup_hits.clone();
    let backup = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = backup_hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "ok": true, "model": "gpt-5" })),
                )
            }
        }),
    ));
    let mut primary_config = primary.upstream_config();
    primary_config.auth.auth_token = Some(AUTH_FINGERPRINT_CANARY.to_string().into());
    let mut backup_config = backup.upstream_config();
    backup_config.auth.auth_token = Some(AUTH_FINGERPRINT_CANARY.to_string().into());
    let service = proxy_service(make_helper_config(
        vec![primary_config, backup_config],
        retry_config(1, "500", Vec::new(), RetryStrategy::Failover),
    ));
    let state = service.state.clone();
    let runtime_store = state.runtime_store_handle();
    let proxy = spawn_proxy_service(service);

    let response = post_responses_json(
        &reqwest::Client::new(),
        &proxy,
        r#"{"model":"gpt-5","input":"hi"}"#,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    let logical_requests = runtime_store
        .read_recent_logical_requests(10)
        .expect("read durable logical requests");
    assert_eq!(logical_requests.len(), 1);
    let logical_request = logical_requests.first().expect("logical request");
    let durable_terminal = logical_request.terminal.as_ref().expect("logical terminal");
    assert_eq!(
        durable_terminal.terminal.outcome,
        crate::runtime_store::LogicalRequestOutcome::Succeeded
    );
    let payload = durable_terminal
        .terminal
        .payload
        .as_ref()
        .expect("runtime terminal payload");
    let projected = state
        .list_recent_finished(10)
        .await
        .into_iter()
        .next()
        .expect("committed recent projection");
    assert_eq!(payload.finished_request, projected);
    assert_eq!(payload.runtime_revision, 1);
    assert_eq!(payload.requested_model.as_deref(), Some("gpt-5"));
    assert_eq!(payload.mapped_model.as_deref(), Some("gpt-5"));
    assert_eq!(payload.reported_model.as_deref(), Some("gpt-5"));
    assert_eq!(payload.requested_service_tier, None);
    assert_eq!(payload.actual_service_tier, None);

    let logical_handle = runtime_store.logical_request_handle(logical_request.request.id);
    let attempts = runtime_store
        .read_attempts_for_logical_request(logical_handle)
        .expect("read durable upstream attempts");
    assert_eq!(attempts.len(), 2);
    let legacy_raw_fingerprint = legacy_raw_bearer_account_fingerprint(AUTH_FINGERPRINT_CANARY);
    let durable_account_fingerprints = attempts
        .iter()
        .map(|attempt| {
            attempt
                .attempt
                .evidence
                .provider_epoch
                .as_ref()
                .expect("durable provider epoch")
                .scope
                .account_fingerprint
                .clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        durable_account_fingerprints[0], durable_account_fingerprints[1],
        "one credential must retain one account identity across failover endpoints"
    );
    assert!(
        durable_account_fingerprints
            .iter()
            .all(|fingerprint| fingerprint != &legacy_raw_fingerprint)
    );
    for (attempt, fingerprint) in attempts.iter().zip(&durable_account_fingerprints) {
        let pending_evidence = runtime_store
            .raw_attempt_pending_evidence_json_for_test(attempt.attempt.id)
            .expect("read raw SQLite pending evidence");
        let pending_evidence_value: serde_json::Value =
            serde_json::from_str(&pending_evidence).expect("parse raw SQLite pending evidence");
        assert_eq!(
            pending_evidence_value
                .pointer("/provider_epoch/scope/account_fingerprint")
                .and_then(serde_json::Value::as_str),
            Some(fingerprint.as_str())
        );
        assert!(!pending_evidence.contains(AUTH_FINGERPRINT_CANARY));
        assert!(!pending_evidence.contains(&legacy_raw_fingerprint));
    }
    assert_eq!(payload.winning_attempt_id, Some(attempts[1].attempt.id));
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| attempt.attempt_ordinal)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| {
                attempt
                    .terminal
                    .as_ref()
                    .map(|terminal| terminal.terminal.outcome)
            })
            .collect::<Vec<_>>(),
        vec![
            Some(crate::runtime_store::AttemptOutcome::Failed),
            Some(crate::runtime_store::AttemptOutcome::Succeeded),
        ]
    );
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| {
                (
                    attempt
                        .attempt
                        .evidence
                        .route
                        .provider_endpoint_key
                        .as_deref(),
                    attempt.attempt.evidence.route.mapped_model.as_deref(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (Some("codex/test/default"), Some("gpt-5")),
            (Some("codex/test-2/default"), Some("gpt-5")),
        ]
    );
    assert!(attempts.iter().all(|attempt| {
        attempt.attempt.evidence.runtime_revision == payload.runtime_revision
            && attempt.attempt.evidence.runtime_digest == payload.runtime_digest
    }));
    assert!(attempts.iter().all(|attempt| {
        attempt
            .attempt
            .evidence
            .provider_epoch
            .as_ref()
            .is_some_and(|epoch| {
                epoch.scope.adapter == crate::provider_catalog::ProviderAdapter::OpenAiCompatible
                    && epoch.scope.account_fingerprint.starts_with("sha256:")
                    && epoch.scope.config_revision == attempt.attempt.evidence.runtime_digest
                    && epoch.catalog_revision.is_none()
                    && epoch.pricing_revision.is_none()
            })
    }));
    assert_eq!(
        payload.provider_epoch,
        attempts[1].attempt.evidence.provider_epoch
    );

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_decodes_unlabeled_gzip_models_response_before_forwarding() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
        scoped.set_path(
            "CODEX_HELPER_CONTROL_TRACE_PATH",
            temp_dir.join("logs").join("control_trace.jsonl").as_path(),
        );
        scoped.set("CODEX_HELPER_CONTROL_TRACE", "1");
    }

    static GZIPPED_MODELS_JSON: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0xab, 0x56, 0x4a, 0x49, 0x2c,
        0x49, 0x54, 0xb2, 0x8a, 0xae, 0x56, 0xca, 0x4c, 0x51, 0xb2, 0x52, 0x4a, 0x2f, 0x28, 0xd1,
        0x35, 0xd5, 0x33, 0x55, 0xd2, 0x51, 0xca, 0x4f, 0xca, 0x4a, 0x4d, 0x2e, 0x01, 0x0a, 0xe5,
        0xe6, 0xa7, 0xa4, 0xe6, 0x28, 0xd5, 0xc6, 0xd6, 0x02, 0x00, 0x93, 0xd6, 0xe0, 0xa4, 0x2c,
        0x00, 0x00, 0x00,
    ];
    let upstream_accept_encoding = Arc::new(std::sync::Mutex::new(None::<String>));
    let seen_accept_encoding = upstream_accept_encoding.clone();
    let upstream = axum::Router::new().route(
        "/v1/models",
        get(move |headers: axum::http::HeaderMap| {
            let seen_accept_encoding = seen_accept_encoding.clone();
            async move {
                let accept_encoding = headers
                    .get(axum::http::header::ACCEPT_ENCODING)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                *seen_accept_encoding.lock().expect("lock") = accept_encoding;

                let mut response =
                    Response::new(Body::from(Bytes::from_static(GZIPPED_MODELS_JSON)));
                *response.status_mut() = StatusCode::OK;
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);

    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::builder()
        .no_gzip()
        .build()
        .expect("client");
    let resp = client
        .get(proxy.url("/models"))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        !resp
            .headers()
            .contains_key(axum::http::header::CONTENT_ENCODING)
    );
    let body = resp.bytes().await.expect("body");
    assert_openai_models_response(body.as_ref(), "gpt-5.5");
    assert_eq!(
        upstream_accept_encoding.lock().expect("lock").as_deref(),
        Some("identity")
    );
}

#[tokio::test]
async fn proxy_decodes_brotli_models_response_before_forwarding() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    static BROTLI_MODELS_JSON: &[u8] = &[
        0x8b, 0x15, 0x80, 0x7b, 0x22, 0x64, 0x61, 0x74, 0x61, 0x22, 0x3a, 0x5b, 0x7b, 0x22, 0x69,
        0x64, 0x22, 0x3a, 0x22, 0x67, 0x70, 0x74, 0x2d, 0x35, 0x2e, 0x35, 0x22, 0x2c, 0x22, 0x6f,
        0x62, 0x6a, 0x65, 0x63, 0x74, 0x22, 0x3a, 0x22, 0x6d, 0x6f, 0x64, 0x65, 0x6c, 0x22, 0x7d,
        0x5d, 0x7d, 0x03,
    ];
    let upstream_accept_encoding = Arc::new(std::sync::Mutex::new(None::<String>));
    let seen_accept_encoding = upstream_accept_encoding.clone();
    let upstream = axum::Router::new().route(
        "/v1/models",
        get(move |headers: axum::http::HeaderMap| {
            let seen_accept_encoding = seen_accept_encoding.clone();
            async move {
                let accept_encoding = headers
                    .get(axum::http::header::ACCEPT_ENCODING)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                *seen_accept_encoding.lock().expect("lock") = accept_encoding;

                let mut response =
                    Response::new(Body::from(Bytes::from_static(BROTLI_MODELS_JSON)));
                *response.status_mut() = StatusCode::OK;
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response.headers_mut().insert(
                    axum::http::header::CONTENT_ENCODING,
                    HeaderValue::from_static("br"),
                );
                response
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);

    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::builder()
        .no_gzip()
        .build()
        .expect("client");
    let resp = client
        .get(proxy.url("/models"))
        .header("accept-encoding", "br")
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        !resp
            .headers()
            .contains_key(axum::http::header::CONTENT_ENCODING)
    );
    let body = resp.bytes().await.expect("body");
    assert_openai_models_response(body.as_ref(), "gpt-5.5");
    assert_eq!(
        upstream_accept_encoding.lock().expect("lock").as_deref(),
        Some("identity")
    );
}

#[tokio::test]
async fn proxy_does_not_translate_openai_models_list_by_default() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let upstream = axum::Router::new().route(
        "/v1/models",
        get(|| async move {
            Json(serde_json::json!({
                "object": "list",
                "data": [
                    { "id": "codex-auto-review", "object": "model" },
                    { "id": "gpt-5.5", "object": "model", "display_name": "GPT-5.5" },
                    { "id": "gpt-image-1", "object": "model" }
                ]
            }))
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);

    let proxy = spawn_test_proxy(cfg);

    let resp = Client::new()
        .get(proxy.url("/models"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.bytes().await.expect("body");
    assert_openai_models_response(body.as_ref(), "gpt-5.5");
}

fn assert_openai_models_response(body: &[u8], expected_slug: &str) -> serde_json::Value {
    let value: serde_json::Value = serde_json::from_slice(body).expect("json body");
    assert!(value.get("models").is_none());
    let data = value["data"].as_array().expect("data array");
    data.iter()
        .find(|model| model["id"].as_str() == Some(expected_slug))
        .expect("expected model");
    value
}

#[tokio::test]
async fn models_status_failover_does_not_poison_inference_health_or_affinity() {
    let primary_models_hits = Arc::new(AtomicUsize::new(0));
    let primary_responses_hits = Arc::new(AtomicUsize::new(0));
    let primary_models_counter = primary_models_hits.clone();
    let primary_responses_counter = primary_responses_hits.clone();
    let primary = spawn_test_upstream(
        axum::Router::new()
            .route(
                "/v1/models",
                get(move || {
                    let hits = primary_models_counter.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(serde_json::json!({ "error": "models unsupported" })),
                        )
                    }
                }),
            )
            .route(
                "/v1/responses",
                post(move || {
                    let hits = primary_responses_counter.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        Json(serde_json::json!({ "provider": "primary" }))
                    }
                }),
            ),
    );

    let backup_models_hits = Arc::new(AtomicUsize::new(0));
    let backup_responses_hits = Arc::new(AtomicUsize::new(0));
    let backup_models_counter = backup_models_hits.clone();
    let backup_responses_counter = backup_responses_hits.clone();
    let backup = spawn_test_upstream(
        axum::Router::new()
            .route(
                "/v1/models",
                get(move || {
                    let hits = backup_models_counter.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        Json(serde_json::json!({
                            "object": "list",
                            "data": [{ "id": "gpt-5.6-sol", "object": "model" }]
                        }))
                    }
                }),
            )
            .route(
                "/v1/responses",
                post(move || {
                    let hits = backup_responses_counter.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        Json(serde_json::json!({ "provider": "backup" }))
                    }
                }),
            ),
    );

    let config = request_local_models_config(
        vec![primary.upstream_config(), backup.upstream_config()],
        request_local_models_retry_config("503", Vec::new()),
    );
    let service = proxy_service(config);
    let state = service.state.clone();
    let proxy = spawn_proxy_service(service);
    let client = local_http_test_client();
    let session_id = "models-request-local-status";

    let response = client
        .get(proxy.url("/models"))
        .header("accept", "text/event-stream")
        .header("session-id", session_id)
        .send()
        .await
        .expect("send models request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.bytes().await.expect("read models response");
    assert_openai_models_response(body.as_ref(), "gpt-5.6-sol");
    assert_eq!(primary_models_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_models_hits.load(Ordering::SeqCst), 1);
    assert!(
        state
            .peek_session_route_affinity(session_id)
            .await
            .is_none()
    );

    let runtime = state
        .route_plan_runtime_state_for_provider_endpoints("codex")
        .await;
    let primary_health =
        runtime.provider_endpoint(&ProviderEndpointKey::new("codex", "test", "default"));
    assert_eq!(primary_health.failure_count, 0);
    assert!(!primary_health.cooldown_active);

    let finished = find_finished_request(&state, 10, |request| request.path == "/models")
        .await
        .expect("finished models request");
    let first_failure = finished
        .retry
        .as_ref()
        .expect("models retry trace")
        .route_attempts
        .iter()
        .find(|attempt| attempt.status_code == Some(StatusCode::SERVICE_UNAVAILABLE.as_u16()))
        .expect("models failure attempt");
    assert_eq!(first_failure.cooldown_secs, None);
    assert_eq!(first_failure.cooldown_reason, None);
    assert!(
        first_failure
            .provider_signals
            .iter()
            .all(|signal| !signal.route_facing)
    );

    let response = client
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("session-id", session_id)
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send inference request");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("read inference response");
    assert_eq!(body["provider"].as_str(), Some("primary"));
    assert_eq!(primary_responses_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_responses_hits.load(Ordering::SeqCst), 0);

    proxy.handle.abort();
}

#[tokio::test]
async fn models_success_does_not_clear_existing_inference_failure_state() {
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/models",
        get(|| async {
            Json(serde_json::json!({
                "object": "list",
                "data": [{ "id": "gpt-5.6-sol", "object": "model" }]
            }))
        }),
    ));
    let config = request_local_models_config(
        vec![upstream.upstream_config()],
        request_local_models_retry_config("503", Vec::new()),
    );
    let service = proxy_service(config);
    let state = service.state.clone();
    let endpoint = ProviderEndpointKey::new("codex", "test", "default");
    let identity = service
        .runtime_identity_for_provider_endpoint_for_test(&endpoint)
        .await;
    state
        .record_runtime_upstream_attempt_failure(
            "codex",
            &identity,
            30,
            crate::endpoint_health::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;
    let runtime = state
        .route_plan_runtime_state_for_provider_endpoints("codex")
        .await;
    assert_eq!(runtime.provider_endpoint(&endpoint).failure_count, 1);

    let proxy = spawn_proxy_service(service);
    let response = local_http_test_client()
        .get(proxy.url("/models"))
        .send()
        .await
        .expect("send models request");
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.expect("read models response");

    let runtime = state
        .route_plan_runtime_state_for_provider_endpoints("codex")
        .await;
    let health = runtime.provider_endpoint(&endpoint);
    assert_eq!(health.failure_count, 1);
    assert!(!health.cooldown_active);

    proxy.handle.abort();
}

#[tokio::test]
async fn models_auth_failure_refreshes_credentials_without_cross_account_replay() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let primary_counter = primary_hits.clone();
    let primary = spawn_test_upstream(axum::Router::new().route(
        "/v1/models",
        get(move || {
            let hits = primary_counter.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({ "error": "credential rejected" })),
                )
            }
        }),
    ));
    let backup_hits = Arc::new(AtomicUsize::new(0));
    let backup_counter = backup_hits.clone();
    let backup = spawn_test_upstream(axum::Router::new().route(
        "/v1/models",
        get(move || {
            let hits = backup_counter.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({
                    "object": "list",
                    "data": [{ "id": "gpt-5.6-sol", "object": "model" }]
                }))
            }
        }),
    ));
    let config = request_local_models_config(
        vec![primary.upstream_config(), backup.upstream_config()],
        request_local_models_retry_config("401", Vec::new()),
    );
    let service = proxy_service(config);
    let state = service.state.clone();
    let proxy = spawn_proxy_service(service);
    let session_id = "models-request-local-auth";

    let response = local_http_test_client()
        .get(proxy.url("/models"))
        .header("session-id", session_id)
        .send()
        .await
        .expect("send models request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let _ = response.bytes().await.expect("read auth failure response");
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 0);
    assert!(
        state
            .peek_session_route_affinity(session_id)
            .await
            .is_none()
    );

    let runtime = state
        .route_plan_runtime_state_for_provider_endpoints("codex")
        .await;
    let primary_health =
        runtime.provider_endpoint(&ProviderEndpointKey::new("codex", "test", "default"));
    assert_eq!(primary_health.failure_count, 0);
    assert!(!primary_health.cooldown_active);

    proxy.handle.abort();
}

#[tokio::test]
async fn models_transport_failover_does_not_penalize_inference_health() {
    let unused_addr = reserve_unused_local_addr();
    let backup_hits = Arc::new(AtomicUsize::new(0));
    let backup_counter = backup_hits.clone();
    let backup = spawn_test_upstream(axum::Router::new().route(
        "/v1/models",
        get(move || {
            let hits = backup_counter.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({
                    "object": "list",
                    "data": [{ "id": "gpt-5.6-sol", "object": "model" }]
                }))
            }
        }),
    ));
    let mut unreachable = backup.upstream_config();
    unreachable.base_url = format!("http://{unused_addr}/v1");
    let config = request_local_models_config(
        vec![unreachable, backup.upstream_config()],
        request_local_models_retry_config("", vec!["upstream_transport_error".to_string()]),
    );
    let service = proxy_service(config);
    let state = service.state.clone();
    let proxy = spawn_proxy_service(service);

    let response = local_http_test_client()
        .get(proxy.url("/models"))
        .send()
        .await
        .expect("send models request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.bytes().await.expect("read models response");
    assert_openai_models_response(body.as_ref(), "gpt-5.6-sol");
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    let runtime = state
        .route_plan_runtime_state_for_provider_endpoints("codex")
        .await;
    let primary_health =
        runtime.provider_endpoint(&ProviderEndpointKey::new("codex", "test", "default"));
    assert_eq!(primary_health.failure_count, 0);
    assert!(!primary_health.cooldown_active);

    let finished = find_finished_request(&state, 10, |request| request.path == "/models")
        .await
        .expect("finished models request");
    let transport_failure = finished
        .retry
        .as_ref()
        .expect("models retry trace")
        .route_attempts
        .iter()
        .find(|attempt| attempt.error_class.as_deref() == Some("upstream_transport_error"))
        .expect("transport failure attempt");
    assert_eq!(transport_failure.cooldown_secs, None);
    assert_eq!(transport_failure.cooldown_reason, None);

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_codex_stream_route_unavailable_returns_retryable_response_failed_sse() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let primary_counter = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            async move {
                primary_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "primary" })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup" })),
                )
            }
        }),
    );
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let retry = RetryConfig {
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let source = HelperConfig {
        retry,
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "probe-primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "probe-backup".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "probe-primary".to_string(),
                "probe-backup".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let primary_endpoint =
        crate::runtime_identity::ProviderEndpointKey::new("codex", "probe-primary", "default");
    let primary_identity = proxy
        .runtime_identity_for_provider_endpoint_for_test(&primary_endpoint)
        .await;
    state
        .penalize_runtime_upstream_attempt(
            "codex",
            &primary_identity,
            30,
            crate::endpoint_health::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;
    let backup_endpoint =
        crate::runtime_identity::ProviderEndpointKey::new("codex", "probe-backup", "default");
    let backup_identity = proxy
        .runtime_identity_for_provider_endpoint_for_test(&backup_endpoint)
        .await;
    state
        .penalize_runtime_upstream_attempt(
            "codex",
            &backup_identity,
            30,
            crate::endpoint_health::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.starts_with("text/event-stream")),
        Some(true)
    );
    let body = resp.text().await.expect("body");
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(body.contains(r#""code":"rate_limit_exceeded""#), "{body}");
    assert!(
        body.contains(r#""codex_helper_error":"route_unavailable""#),
        "{body}"
    );
    assert!(body.contains("try again in"), "{body}");
    assert_eq!(primary_hits.load(Ordering::SeqCst), 0);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 0);

    let mut finished = Vec::new();
    for _ in 0..100 {
        finished = state.list_recent_finished(10).await;
        if finished.iter().any(|request| request.status_code == 502) {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    let failed = finished
        .iter()
        .find(|request| request.status_code == 502)
        .expect("failed request should be finalized");
    let retry = failed.retry.as_ref().expect("retry trace");
    assert_eq!(retry.attempts, 0);
    assert_eq!(
        retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.decision.as_str())
            .collect::<Vec<_>>(),
        vec!["route_unavailable", "route_unavailable"]
    );
    assert!(retry.route_attempts.iter().all(|attempt| {
        attempt.stable_code() == "route_unavailable"
            && attempt.cooldown_secs.is_some()
            && attempt.cooldown_reason.as_deref() == Some("runtime_cooldown")
    }));

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_codex_body_stream_route_unavailable_returns_retryable_response_failed_sse() {
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            (
                StatusCode::OK,
                Json(serde_json::json!({ "provider": "primary" })),
            )
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let retry = RetryConfig {
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let source = HelperConfig {
        retry,
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([(
                "probe-primary".to_string(),
                ProviderConfig {
                    base_url: Some(format!("http://{primary_addr}/v1")),
                    inline_auth: UpstreamAuth::default(),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "probe-primary".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let primary_endpoint =
        crate::runtime_identity::ProviderEndpointKey::new("codex", "probe-primary", "default");
    let primary_identity = proxy
        .runtime_identity_for_provider_endpoint_for_test(&primary_endpoint)
        .await;
    state
        .penalize_runtime_upstream_attempt(
            "codex",
            &primary_identity,
            30,
            crate::endpoint_health::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","input":"hi","stream":true}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.starts_with("text/event-stream")),
        Some(true)
    );
    let body = resp.text().await.expect("body");
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(
        body.contains(r#""codex_helper_error":"route_unavailable""#),
        "{body}"
    );
    assert!(body.contains("try again in"), "{body}");

    proxy_handle.abort();
    primary_handle.abort();
}

#[tokio::test]
async fn proxy_codex_stream_upstream_read_error_emits_response_failed_terminal_event() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind raw upstream");
    let upstream_addr = listener.local_addr().expect("upstream addr");
    let upstream_handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept raw upstream");
        let mut request_buf = [0_u8; 2048];
        let _ = socket.read(&mut request_buf).await;
        let first_chunk = b": upstream-started\n\n";
        let headers = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n{:X}\r\n",
            first_chunk.len()
        );
        socket
            .write_all(headers.as_bytes())
            .await
            .expect("write headers");
        socket.write_all(first_chunk).await.expect("write chunk");
        socket
            .write_all(b"\r\n10\r\npartial")
            .await
            .expect("write partial chunk");
        socket.flush().await.expect("flush partial chunk");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    });
    let mut retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    retry.transport_cooldown_secs = Some(30);
    let cfg = make_helper_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
            tags: HashMap::from([
                ("provider_id".to_string(), "primary".to_string()),
                ("endpoint_id".to_string(), "default".to_string()),
                (
                    "provider_endpoint_key".to_string(),
                    "codex/primary/default".to_string(),
                ),
            ]),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        retry,
    );
    let proxy_service = proxy_service(cfg);
    let state = proxy_service.state_handle();
    let proxy = spawn_proxy_service(proxy_service);

    let resp = Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt-5","input":"hi","stream":true}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("body should remain readable");
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(body.contains(r#""code":"upstream_error""#), "{body}");
    assert!(
        body.contains(r#""codex_helper_error":"upstream_stream_error""#),
        "{body}"
    );
    assert!(body.contains("Upstream stream failed:"), "{body}");

    let finished = find_finished_request(&state, 10, |request| request.path == "/v1/responses")
        .await
        .expect("finished request");
    let route_attempt = finished
        .retry
        .as_ref()
        .and_then(|retry| retry.route_attempts.first())
        .expect("route attempt");
    assert_eq!(route_attempt.provider_signals.len(), 1);
    assert_eq!(
        route_attempt.provider_signals[0].kind,
        crate::provider_signals::ProviderSignalKind::Transport
    );
    assert!(route_attempt.policy_actions.is_empty());
    assert!(
        state
            .capture_provider_policy_snapshot()
            .await
            .projections
            .is_empty()
    );

    proxy.handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_codex_stream_idle_timeout_emits_response_failed_terminal_event() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
        scoped.set("CODEX_HELPER_STREAM_IDLE_TIMEOUT_SECS", "1");
    }

    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|| async move {
            let first = stream::once(async {
                Ok::<Bytes, Infallible>(Bytes::from_static(
                    b": upstream-started\n\ndata: {\"response\":{\"service_tier\":\"default\"}}\n\n",
                ))
            });
            let stalled = stream::pending::<Result<Bytes, Infallible>>();
            let mut response = Response::new(Body::from_stream(first.chain(stalled)));
            *response.status_mut() = StatusCode::OK;
            response
                .headers_mut()
                .insert("content-type", HeaderValue::from_static("text/event-stream"));
            response
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let mut retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    retry.transport_cooldown_secs = Some(30);
    let cfg = make_helper_config(
        vec![UpstreamConfig {
            tags: HashMap::from([
                ("provider_id".to_string(), "primary".to_string()),
                ("endpoint_id".to_string(), "default".to_string()),
                (
                    "provider_endpoint_key".to_string(),
                    "codex/primary/default".to_string(),
                ),
            ]),
            ..upstream.upstream_config()
        }],
        retry,
    );
    let proxy_service = proxy_service(cfg);
    let state = proxy_service.state_handle();
    let proxy = spawn_proxy_service(proxy_service);

    let client = Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("client");
    let resp = post_responses_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","input":"hi","stream":true}"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("idle timeout should finish body");
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(
        body.contains(r#""codex_helper_error":"upstream_stream_idle_timeout""#),
        "{body}"
    );
    assert!(
        body.contains("Upstream stream idle timeout after 1s without bytes"),
        "{body}"
    );

    let finished = find_finished_request(&state, 10, |request| request.path == "/v1/responses")
        .await
        .expect("finished request");
    // Downstream headers were already committed as 200, but the logical request failed in-stream.
    assert_eq!(finished.status_code, StatusCode::BAD_GATEWAY.as_u16());
    assert!(finished.streaming);
    assert!(finished.usage.is_none());
    assert!(
        finished.duration_ms < 3_000,
        "duration should be bounded by idle watchdog: {finished:?}"
    );
    let route_attempt = finished
        .retry
        .as_ref()
        .and_then(|retry| retry.route_attempts.first())
        .expect("route attempt");
    assert_eq!(route_attempt.provider_signals.len(), 1);
    assert_eq!(
        route_attempt.provider_signals[0].kind,
        crate::provider_signals::ProviderSignalKind::Transport
    );
    assert!(route_attempt.policy_actions.is_empty());
    assert!(
        state
            .capture_provider_policy_snapshot()
            .await
            .projections
            .is_empty()
    );
}

#[tokio::test]
async fn proxy_codex_stream_mixed_upstream_failure_and_cooldown_reports_route_unavailable_sse() {
    let failing_hits = Arc::new(AtomicUsize::new(0));
    let failing_counter = failing_hits.clone();
    let failing = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let failing_counter = failing_counter.clone();
            async move {
                failing_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "err": "failing 502" })),
                )
            }
        }),
    );
    let (failing_addr, failing_handle) = spawn_axum_server(failing);

    let cooldown = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            (
                StatusCode::OK,
                Json(serde_json::json!({ "provider": "cooldown" })),
            )
        }),
    );
    let (cooldown_addr, cooldown_handle) = spawn_axum_server(cooldown);

    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let source = HelperConfig {
        retry,
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "failing".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{failing_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "cooldown".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{cooldown_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "failing".to_string(),
                "cooldown".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let cooldown_endpoint =
        crate::runtime_identity::ProviderEndpointKey::new("codex", "cooldown", "default");
    let cooldown_identity = proxy
        .runtime_identity_for_provider_endpoint_for_test(&cooldown_endpoint)
        .await;
    state
        .penalize_runtime_upstream_attempt(
            "codex",
            &cooldown_identity,
            30,
            crate::endpoint_health::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt-5","input":"hi","stream":true}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(body.contains(r#""code":"rate_limit_exceeded""#), "{body}");
    assert!(
        body.contains(r#""codex_helper_error":"route_unavailable""#),
        "{body}"
    );
    assert!(body.contains("all upstream attempts failed"), "{body}");
    assert_eq!(failing_hits.load(Ordering::SeqCst), 1);

    let finished = state.list_recent_finished(1).await;
    let retry = finished
        .first()
        .and_then(|request| request.retry.as_ref())
        .expect("retry trace");
    assert!(
        retry
            .route_attempts
            .iter()
            .any(|attempt| attempt.decision == "failed_status")
    );
    assert!(
        retry
            .route_attempts
            .iter()
            .any(|attempt| attempt.decision == "route_unavailable")
    );

    proxy_handle.abort();
    failing_handle.abort();
    cooldown_handle.abort();
}

#[tokio::test]
async fn proxy_codex_stream_usage_exhausted_route_returns_retryable_response_failed_sse() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let primary_counter = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            async move {
                primary_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "primary" })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup" })),
                )
            }
        }),
    );
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "limited-primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "limited-backup".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "limited-primary".to_string(),
                "limited-backup".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let observed_at_ms = crate::logging::now_ms();
    proxy
        .set_provider_automatic_block_for_test(
            crate::runtime_identity::ProviderEndpointKey::new(
                "codex",
                "limited-primary",
                "default",
            ),
            true,
            observed_at_ms,
        )
        .await;
    proxy
        .set_provider_automatic_block_for_test(
            crate::runtime_identity::ProviderEndpointKey::new("codex", "limited-backup", "default"),
            true,
            observed_at_ms,
        )
        .await;
    proxy
        .config
        .publish_provider_policy(state.capture_provider_policy_snapshot().await)
        .await
        .expect("publish provider policy snapshot");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(body.contains(r#""code":"rate_limit_exceeded""#), "{body}");
    assert!(body.contains(r#""retry_after_secs":8"#), "{body}");
    assert!(!body.contains("try again in 8 seconds"), "{body}");
    assert_eq!(primary_hits.load(Ordering::SeqCst), 0);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 0);

    let mut finished = Vec::new();
    for _ in 0..100 {
        finished = state.list_recent_finished(10).await;
        if finished.iter().any(|request| request.status_code == 502) {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    let failed = finished
        .iter()
        .find(|request| request.status_code == 502)
        .expect("failed request should be finalized");
    let retry = failed.retry.as_ref().expect("retry trace");
    assert_eq!(
        retry
            .route_attempts
            .iter()
            .map(RouteAttemptLog::stable_code)
            .collect::<Vec<_>>(),
        vec!["route_unavailable", "route_unavailable"]
    );

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_codex_retryable_429_enqueues_usage_probe_for_provider_endpoint() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let primary_counter = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            async move {
                primary_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({
                        "error": {
                            "code": "rate_limit_exceeded",
                            "message": "relay quota exhausted"
                        }
                    })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup" })),
                )
            }
        }),
    );
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let retry = RetryConfig {
        upstream: Some(retry_layer_config(
            1,
            "429",
            Vec::new(),
            RetryStrategy::Failover,
        )),
        provider: Some(retry_layer_config(
            2,
            "429",
            Vec::new(),
            RetryStrategy::Failover,
        )),
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let source = HelperConfig {
        retry,
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "primary".to_string(),
                "backup".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let proxy_state = proxy.state_handle();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);
    assert!(
        crate::usage_providers::request_balance_refresh_queued_for_provider_endpoint(
            proxy_state.as_ref(),
            &crate::runtime_identity::ProviderEndpointKey::new("codex", "primary", "default")
        ),
        "retryable 429 should enqueue a balance refresh for the failed provider endpoint"
    );

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_capacity_body_400_fails_over_by_overloaded_class() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let primary_counter = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            async move {
                primary_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "type": "invalid_request_error",
                            "message": "Selected model is at capacity. Please try a different model."
                        }
                    })),
                )
            }
        }),
    );
    let primary = spawn_test_upstream(primary);

    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup" })),
                )
            }
        }),
    );
    let backup = spawn_test_upstream(backup);

    let retry = RetryConfig {
        upstream: Some(retry_layer_config(
            1,
            "",
            Vec::new(),
            RetryStrategy::SameUpstream,
        )),
        provider: Some(retry_layer_config(
            2,
            "",
            vec!["upstream_overloaded".to_string()],
            RetryStrategy::Failover,
        )),
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let cfg = make_helper_config(
        vec![primary.upstream_config(), backup.upstream_config()],
        retry,
    );
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let resp = post_responses_json(&client, &proxy, r#"{"model":"gpt-5","input":"hi"}"#)
        .await
        .error_for_status()
        .expect("failover status")
        .json::<serde_json::Value>()
        .await
        .expect("json response");

    assert_eq!(resp["provider"].as_str(), Some("backup"));
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    let finished = find_finished_request(&state, 10, |request| {
        request.status_code == StatusCode::OK.as_u16()
    })
    .await
    .expect("finished request");
    let retry = finished.retry.expect("retry trace");
    let first = retry
        .route_attempts
        .first()
        .expect("first route attempt should be recorded");
    assert_eq!(first.status_code, Some(StatusCode::BAD_REQUEST.as_u16()));
    assert_eq!(first.error_class.as_deref(), Some("upstream_overloaded"));
    assert_eq!(
        first.cooldown_reason.as_deref(),
        Some("upstream_overloaded")
    );

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_new_api_saturated_group_429_fails_over_without_same_upstream_retry() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let primary_counter = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            async move {
                primary_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({
                        "error": {
                            "type": "rate_limit_error",
                            "message": "当前分组上游负载已饱和，请稍后再试"
                        }
                    })),
                )
            }
        }),
    );
    let primary = spawn_test_upstream(primary);

    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup" })),
                )
            }
        }),
    );
    let backup = spawn_test_upstream(backup);

    let retry = RetryConfig {
        upstream: Some(retry_layer_config(
            2,
            "429",
            vec!["upstream_overloaded".to_string()],
            RetryStrategy::SameUpstream,
        )),
        provider: Some(retry_layer_config(
            2,
            "429",
            vec!["upstream_overloaded".to_string()],
            RetryStrategy::Failover,
        )),
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let cfg = make_helper_config(
        vec![primary.upstream_config(), backup.upstream_config()],
        retry,
    );
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let resp = post_responses_json(&client, &proxy, r#"{"model":"gpt-5","input":"hi"}"#)
        .await
        .error_for_status()
        .expect("failover status")
        .json::<serde_json::Value>()
        .await
        .expect("json response");

    assert_eq!(resp["provider"].as_str(), Some("backup"));
    assert_eq!(
        primary_hits.load(Ordering::SeqCst),
        1,
        "saturated upstream should not be retried before failover"
    );
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    let finished = find_finished_request(&state, 10, |request| {
        request.status_code == StatusCode::OK.as_u16()
    })
    .await
    .expect("finished request");
    let retry = finished.retry.expect("retry trace");
    let first = retry
        .route_attempts
        .first()
        .expect("first route attempt should be recorded");
    assert_eq!(
        first.status_code,
        Some(StatusCode::TOO_MANY_REQUESTS.as_u16())
    );
    assert_eq!(first.error_class.as_deref(), Some("upstream_overloaded"));
    assert_eq!(
        first.cooldown_reason.as_deref(),
        Some("upstream_overloaded")
    );
    assert_eq!(first.provider_signals.len(), 1);
    assert_eq!(
        first.provider_signals[0].kind,
        crate::provider_signals::ProviderSignalKind::Capacity
    );

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_429_usage_limit_body_sets_retry_after_cooldown() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let primary_counter = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            async move {
                primary_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({
                        "error": {
                            "type": "usage_limit_reached",
                            "message": "usage limit reached",
                            "resets_in_seconds": 12
                        }
                    })),
                )
            }
        }),
    );
    let primary = spawn_test_upstream(primary);

    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup" })),
                )
            }
        }),
    );
    let backup = spawn_test_upstream(backup);

    let retry = RetryConfig {
        upstream: Some(retry_layer_config(
            1,
            "",
            Vec::new(),
            RetryStrategy::SameUpstream,
        )),
        provider: Some(retry_layer_config(
            2,
            "",
            vec!["upstream_rate_limited".to_string()],
            RetryStrategy::Failover,
        )),
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let cfg = make_helper_config(
        vec![primary.upstream_config(), backup.upstream_config()],
        retry,
    );
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let resp = post_responses_json(&client, &proxy, r#"{"model":"gpt-5","input":"hi"}"#)
        .await
        .error_for_status()
        .expect("failover status")
        .json::<serde_json::Value>()
        .await
        .expect("json response");

    assert_eq!(resp["provider"].as_str(), Some("backup"));
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    let finished = find_finished_request(&state, 10, |request| {
        request.status_code == StatusCode::OK.as_u16()
    })
    .await
    .expect("finished request");
    let retry = finished.retry.expect("retry trace");
    let first = retry
        .route_attempts
        .first()
        .expect("first route attempt should be recorded");
    assert_eq!(
        first.status_code,
        Some(StatusCode::TOO_MANY_REQUESTS.as_u16())
    );
    assert_eq!(first.error_class.as_deref(), Some("upstream_rate_limited"));
    assert_eq!(first.cooldown_secs, Some(12));
    assert_eq!(
        first.cooldown_reason.as_deref(),
        Some("upstream_rate_limited")
    );
    assert_eq!(first.provider_signals.len(), 1);
    assert_eq!(
        first.provider_signals[0].kind,
        crate::provider_signals::ProviderSignalKind::RateLimit
    );
    assert_eq!(first.provider_signals[0].reset_after_secs, Some(12));
    assert!(first.policy_actions.is_empty());
    assert!(
        state
            .capture_provider_policy_snapshot()
            .await
            .projections
            .is_empty()
    );

    proxy.handle.abort();
}

#[tokio::test]
async fn proxy_route_graph_429_usage_limit_records_signal_without_policy_action() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let primary_counter = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            async move {
                primary_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({
                        "error": {
                            "type": "usage_limit_reached",
                            "message": "usage limit reached",
                            "resets_in_seconds": 12
                        }
                    })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup" })),
                )
            }
        }),
    );
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let retry = RetryConfig {
        upstream: Some(retry_layer_config(
            1,
            "",
            Vec::new(),
            RetryStrategy::SameUpstream,
        )),
        provider: Some(retry_layer_config(
            2,
            "",
            vec!["upstream_rate_limited".to_string()],
            RetryStrategy::Failover,
        )),
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let source = HelperConfig {
        retry,
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "primary".to_string(),
                "backup".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send")
        .error_for_status()
        .expect("status")
        .json::<serde_json::Value>()
        .await
        .expect("json response");

    assert_eq!(resp["provider"].as_str(), Some("backup"));
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    let finished = find_finished_request(&state, 10, |request| {
        request.status_code == StatusCode::OK.as_u16()
    })
    .await
    .expect("finished request");
    let retry = finished.retry.expect("retry trace");
    let first = retry.route_attempts.first().expect("first route attempt");
    assert_eq!(
        first.provider_endpoint_key.as_deref(),
        Some("codex/primary/default")
    );
    assert_eq!(first.provider_signals.len(), 1);
    assert!(matches!(
        first.provider_signals[0].kind,
        crate::provider_signals::ProviderSignalKind::RateLimit
    ));
    assert!(first.provider_signals[0].route_facing);
    assert_eq!(first.provider_signals[0].reset_after_secs, Some(12));
    assert!(first.policy_actions.is_empty());
    assert!(
        state
            .capture_provider_policy_snapshot()
            .await
            .projections
            .is_empty()
    );

    let log_text =
        std::fs::read_to_string(crate::logging::request_log_path()).expect("read request log");
    let record = log_text
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|record| record["status_code"].as_u64() == Some(StatusCode::OK.as_u16() as u64))
        .expect("logged successful request");
    assert_eq!(
        record["provider_signals"][0]["kind"].as_str(),
        Some("rate_limit")
    );
    assert!(record.get("policy_actions").is_none());

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_reasoning_guard_retries_non_streaming_516_response() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let counter = counter.clone();
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                let reasoning_tokens = if attempt == 0 { 516 } else { 32 };
                let output_text = if attempt == 0 {
                    "bad-direct-final"
                } else {
                    "good-retry"
                };
                Json(serde_json::json!({
                    "id": format!("resp_{attempt}"),
                    "object": "response",
                    "output_text": output_text,
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 20,
                        "total_tokens": 30,
                        "reasoning_tokens": reasoning_tokens
                    }
                }))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_helper_config(
        vec![upstream.upstream_config()],
        reasoning_guard_retry_config(2),
    );
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let runtime_store = state.runtime_store_handle();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let body = post_responses_json(&client, &proxy, r#"{"model":"gpt-5","input":"hi"}"#)
        .await
        .error_for_status()
        .expect("response status")
        .json::<serde_json::Value>()
        .await
        .expect("json response");

    assert_eq!(body["output_text"].as_str(), Some("good-retry"));
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    let finished = find_finished_request(&state, 10, |request| {
        request.path == "/v1/responses" && request.status_code == StatusCode::OK.as_u16()
    })
    .await
    .expect("finished request");
    let retry = finished.retry.expect("retry info");
    assert_eq!(retry.attempts, 2);
    let first = retry.route_attempts.first().expect("first attempt");
    assert_eq!(first.decision, "failed_reasoning_guard");
    assert_eq!(
        first.error_class.as_deref(),
        Some("reasoning_guard_triggered")
    );
    assert_eq!(first.stable_code(), "failed_reasoning_guard");

    let logical_requests = runtime_store
        .read_recent_logical_requests(10)
        .expect("read reasoning-guard logical request");
    assert_eq!(logical_requests.len(), 1);
    let attempts = runtime_store
        .read_attempts_for_logical_request(
            runtime_store.logical_request_handle(logical_requests[0].request.id),
        )
        .expect("read reasoning-guard attempts");
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| {
                attempt
                    .terminal
                    .as_ref()
                    .map(|terminal| terminal.terminal.outcome)
            })
            .collect::<Vec<_>>(),
        vec![
            Some(crate::runtime_store::AttemptOutcome::Failed),
            Some(crate::runtime_store::AttemptOutcome::Succeeded),
        ]
    );
}

#[tokio::test]
async fn proxy_reasoning_guard_passes_final_516_response_after_retry_budget() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let counter = counter.clone();
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({
                    "id": format!("resp_{attempt}"),
                    "object": "response",
                    "output_text": format!("still-516-attempt-{attempt}"),
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 20,
                        "total_tokens": 30,
                        "reasoning_tokens": 516
                    }
                }))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_helper_config(
        vec![upstream.upstream_config()],
        reasoning_guard_retry_config(2),
    );
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let body = post_responses_json(&client, &proxy, r#"{"model":"gpt-5","input":"hi"}"#)
        .await
        .error_for_status()
        .expect("response status")
        .json::<serde_json::Value>()
        .await
        .expect("json response");

    assert_eq!(body["output_text"].as_str(), Some("still-516-attempt-1"));
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    let finished = find_finished_request(&state, 10, |request| {
        request.path == "/v1/responses" && request.status_code == StatusCode::OK.as_u16()
    })
    .await
    .expect("finished request");
    let retry = finished.retry.expect("retry info");
    assert_eq!(retry.attempts, 2);
    let first = retry.route_attempts.first().expect("first attempt");
    assert_eq!(first.decision, "failed_reasoning_guard");
    assert_eq!(
        first.error_class.as_deref(),
        Some("reasoning_guard_triggered")
    );
    assert_eq!(first.stable_code(), "failed_reasoning_guard");
    let final_attempt = retry.route_attempts.last().expect("final attempt");
    assert_eq!(final_attempt.decision, "completed");
    assert_eq!(final_attempt.status_code, Some(StatusCode::OK.as_u16()));
}

#[tokio::test]
async fn proxy_reasoning_guard_strict_buffers_streaming_516_response_before_retry() {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let counter = counter.clone();
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                let (text, reasoning_tokens) = if attempt == 0 {
                    ("bad-direct-final", 516)
                } else {
                    ("good-retry", 32)
                };
                let body = format!(
                    "event: response.output_text.delta\n\
data: {{\"delta\":\"{text}\"}}\n\n\
event: response.completed\n\
data: {{\"response\":{{\"usage\":{{\"input_tokens\":10,\"output_tokens\":20,\"total_tokens\":30,\"reasoning_tokens\":{reasoning_tokens}}}}}}}\n\n"
                );
                let mut resp = Response::new(Body::from(body));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_helper_config(
        vec![upstream.upstream_config()],
        reasoning_guard_retry_config(2),
    );
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let body = client
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt-5","input":"hi","stream":true}"#)
        .send()
        .await
        .expect("send")
        .error_for_status()
        .expect("response status")
        .text()
        .await
        .expect("sse body");

    assert!(body.contains("good-retry"), "{body}");
    assert!(!body.contains("bad-direct-final"), "{body}");
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    let finished = find_finished_request(&state, 10, |request| {
        request.path == "/v1/responses" && request.status_code == StatusCode::OK.as_u16()
    })
    .await
    .expect("finished request");
    assert!(finished.streaming);
    let retry = finished.retry.expect("retry info");
    assert_eq!(retry.attempts, 2);
    let first = retry.route_attempts.first().expect("first attempt");
    assert_eq!(first.decision, "failed_reasoning_guard");
    assert_eq!(
        first.error_class.as_deref(),
        Some("reasoning_guard_triggered")
    );
    assert_eq!(first.stable_code(), "failed_reasoning_guard");
}

#[tokio::test]
async fn proxy_streaming_parses_usage_even_when_usage_is_late_in_stream() {
    // Large prefix with no `data:` lines: should push the stream well past 1MB without triggering JSON parse.
    // The final `data:` line includes `response.usage`, which codex-helper should still detect.
    let prefix = Bytes::from(format!("event: {}\n\n", "x".repeat(4096)));
    let n = 260usize; // ~1.1MB before usage
    let usage = Bytes::from(
        "event: response.completed\n\
data: {\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
    );

    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let prefix = prefix.clone();
            let usage = usage.clone();
            async move {
                // Use a non-streaming body here to avoid flaky chunked-decoding failures on some
                // hyper/reqwest versions, while still exercising the proxy SSE path and the
                // "usage appears after >1MB of non-data bytes" scenario.
                let mut body = Vec::with_capacity(prefix.len().saturating_mul(n) + usage.len());
                for _ in 0..n {
                    body.extend_from_slice(prefix.as_ref());
                }
                body.extend_from_slice(usage.as_ref());
                let mut resp = Response::new(Body::from(body));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);

    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let mut drained_ok = false;
    let mut last_status: Option<StatusCode> = None;
    for _ in 0..3 {
        let resp = client
            .post(proxy.responses_url())
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(r#"{"model":"gpt","input":"hi"}"#)
            .send()
            .await
            .expect("send");
        last_status = Some(resp.status());
        if resp.status() == StatusCode::OK && resp.bytes().await.is_ok() {
            drained_ok = true;
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(last_status, Some(StatusCode::OK));
    assert!(
        drained_ok,
        "expected to drain SSE body without decode error"
    );

    let mut finished = Vec::new();
    for _ in 0..100 {
        finished = state.list_recent_finished(10).await;
        if finished.iter().any(|f| f.usage.is_some()) {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !finished.is_empty(),
        "expected finished request to be recorded"
    );
    let u = finished
        .iter()
        .find_map(|f| f.usage.as_ref())
        .expect("usage should be parsed");
    assert_eq!(u.total_tokens, 3);
}

#[tokio::test]
async fn proxy_records_responses_compact_unsupported_status_for_fallback_diagnostics() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));

    let hits = upstream_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses/compact",
        post(move || async move {
            hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": {
                        "code": "compact_not_supported",
                        "message": "compact is not supported"
                    }
                })),
            )
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);

    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let resp = post_compact_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

    let compact = find_finished_request(&state, 10, |request| request.path == "/responses/compact")
        .await
        .expect("expected compact request path to be visible in finished requests");
    assert_eq!(compact.status_code, StatusCode::NOT_FOUND.as_u16());
}

#[tokio::test]
async fn proxy_does_not_retry_or_failover_on_400() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "err": "bad request" })),
            )
        }),
    );
    let upstream1 = spawn_test_upstream(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let upstream2 = spawn_test_upstream(upstream2);

    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(2),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(2),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_helper_config(
        vec![upstream1.upstream_config(), upstream2.upstream_config()],
        retry,
    );

    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::new();
    let resp = post_responses_json(&client, &proxy, r#"{"model":"gpt","input":"hi"}"#).await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn proxy_buffered_401_and_403_share_one_native_refresh_without_replay_or_failover() {
    let unauthorized_hits = Arc::new(AtomicUsize::new(0));
    let forbidden_hits = Arc::new(AtomicUsize::new(0));
    let unauthorized_counter = Arc::clone(&unauthorized_hits);
    let forbidden_counter = Arc::clone(&forbidden_hits);
    let primary = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move |body: String| {
            let unauthorized_counter = Arc::clone(&unauthorized_counter);
            let forbidden_counter = Arc::clone(&forbidden_counter);
            async move {
                let body: serde_json::Value =
                    serde_json::from_str(&body).expect("parse buffered auth request body");
                let (status, message) = match body.get("input").and_then(|value| value.as_str()) {
                    Some("unauthorized") => {
                        unauthorized_counter.fetch_add(1, Ordering::SeqCst);
                        (StatusCode::UNAUTHORIZED, "unauthorized")
                    }
                    Some("forbidden") => {
                        forbidden_counter.fetch_add(1, Ordering::SeqCst);
                        (StatusCode::FORBIDDEN, "forbidden")
                    }
                    input => panic!("unexpected buffered auth test input: {input:?}"),
                };
                (status, Json(serde_json::json!({ "error": message })))
            }
        }),
    ));
    let backup_hits = Arc::new(AtomicUsize::new(0));
    let backup_counter = Arc::clone(&backup_hits);
    let backup = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = Arc::clone(&backup_counter);
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({ "ok": true }))
            }
        }),
    ));
    let mut primary_config = primary.upstream_config();
    primary_config.auth = UpstreamAuth {
        auth_token_ref: Some(crate::config::CredentialRef::Native {
            name: "relay.primary".to_string(),
        }),
        ..UpstreamAuth::default()
    };
    let source = make_helper_config(
        vec![primary_config, backup.upstream_config()],
        RetryConfig {
            upstream: Some(retry_layer_config(
                2,
                "401,403",
                Vec::new(),
                RetryStrategy::SameUpstream,
            )),
            provider: Some(retry_layer_config(
                2,
                "401,403",
                Vec::new(),
                RetryStrategy::Failover,
            )),
            ..RetryConfig::default()
        },
    );
    let (credential_sources, credential_control) =
        crate::credentials::CredentialSourceCapabilities::test_native(
            crate::credentials::SecretValue::new(b"generation-a".to_vec())
                .expect("valid initial credential"),
        );
    let runtime_store =
        Arc::new(crate::runtime_store::RuntimeStore::open_in_memory().expect("open runtime store"));
    let proxy_service = ProxyService::new_with_runtime_store_and_credential_sources(
        Client::new(),
        Arc::new(source),
        "codex",
        runtime_store,
        credential_sources,
    )
    .expect("build credential-backed proxy");
    let retained = proxy_service.clone();
    assert_eq!(credential_control.read_count(), 1);
    let proxy = spawn_proxy_service(proxy_service);
    let client = local_http_test_client();

    let (unauthorized_response, forbidden_response) = tokio::join!(
        post_responses_json(
            &client,
            &proxy,
            r#"{"model":"gpt-5","input":"unauthorized"}"#,
        ),
        post_responses_json(&client, &proxy, r#"{"model":"gpt-5","input":"forbidden"}"#,),
    );
    assert_eq!(unauthorized_response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(forbidden_response.status(), StatusCode::FORBIDDEN);
    let _ = unauthorized_response
        .bytes()
        .await
        .expect("drain buffered 401 response");
    let _ = forbidden_response
        .bytes()
        .await
        .expect("drain buffered 403 response");

    assert_eq!(unauthorized_hits.load(Ordering::SeqCst), 1);
    assert_eq!(forbidden_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 0);
    assert_eq!(credential_control.read_count(), 1);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let refresh_driver = retained.spawn_credential_refresh_driver(shutdown_rx);
    tokio::time::timeout(Duration::from_secs(2), async {
        while credential_control.read_count() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("buffered auth failures should schedule one native refresh");
    shutdown_tx.send(true).expect("signal refresh shutdown");
    refresh_driver.await.expect("join refresh driver");
    assert_eq!(credential_control.read_count(), 2);
}

#[tokio::test]
async fn proxy_buffered_cloudflare_403_does_not_schedule_native_refresh() {
    let challenge_hits = Arc::new(AtomicUsize::new(0));
    let challenge_counter = Arc::clone(&challenge_hits);
    let challenge = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let challenge_counter = Arc::clone(&challenge_counter);
            async move {
                challenge_counter.fetch_add(1, Ordering::SeqCst);
                axum::response::IntoResponse::into_response((
                    StatusCode::FORBIDDEN,
                    [
                        (axum::http::header::CONTENT_TYPE, "text/html"),
                        (axum::http::header::SERVER, "cloudflare"),
                    ],
                    "<html><script src=\"/cdn-cgi/challenge-platform/x.js\"></script></html>",
                ))
            }
        }),
    ));
    let fence_hits = Arc::new(AtomicUsize::new(0));
    let fence_counter = Arc::clone(&fence_hits);
    let fence = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let fence_counter = Arc::clone(&fence_counter);
            async move {
                fence_counter.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({ "ok": true }))
            }
        }),
    ));
    let providers = std::collections::BTreeMap::from([
        (
            "challenge".to_string(),
            ProviderConfig {
                base_url: Some(challenge.base_url()),
                auth: UpstreamAuth {
                    auth_token_ref: Some(crate::config::CredentialRef::Native {
                        name: "relay.challenge".to_string(),
                    }),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        ),
        (
            "fence".to_string(),
            ProviderConfig {
                base_url: Some(fence.base_url()),
                auth: UpstreamAuth {
                    auth_token_ref: Some(crate::config::CredentialRef::Native {
                        name: "relay.fence".to_string(),
                    }),
                    ..UpstreamAuth::default()
                },
                ..ProviderConfig::default()
            },
        ),
    ]);
    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers,
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "challenge".to_string(),
                "fence".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        retry: RetryConfig {
            upstream: Some(retry_layer_config(
                1,
                "403",
                vec!["cloudflare_challenge".to_string()],
                RetryStrategy::SameUpstream,
            )),
            provider: Some(retry_layer_config(
                1,
                "403",
                vec!["cloudflare_challenge".to_string()],
                RetryStrategy::Failover,
            )),
            ..RetryConfig::default()
        },
        ..HelperConfig::default()
    };
    let (credential_sources, credential_control) =
        crate::credentials::CredentialSourceCapabilities::test_native(
            crate::credentials::SecretValue::new(b"generation-a".to_vec())
                .expect("valid initial credential"),
        );
    let runtime_store =
        Arc::new(crate::runtime_store::RuntimeStore::open_in_memory().expect("open runtime store"));
    let proxy_service = ProxyService::new_with_runtime_store_and_credential_sources(
        Client::new(),
        Arc::new(source),
        "codex",
        runtime_store,
        credential_sources,
    )
    .expect("build credential-backed proxy");
    let retained = proxy_service.clone();
    let initial_snapshot = retained.config.capture().await;
    let route_plan = initial_snapshot
        .capture_route_plan("codex", &crate::routing_ir::RouteRequestContext::default())
        .expect("capture challenge route plan")
        .expect("challenge route plan");
    let fence_candidate = route_plan
        .template()
        .candidates
        .iter()
        .find(|candidate| candidate.provider_id == "fence")
        .expect("fence candidate");
    let fence_target = route_plan
        .template()
        .capture_candidate(fence_candidate)
        .expect("capture fence credential");
    let initial_generation_revision = initial_snapshot.credential_generation().revision();
    assert_eq!(credential_control.read_count(), 2);
    let proxy = spawn_proxy_service(proxy_service);

    let response = post_responses_json(
        &local_http_test_client(),
        &proxy,
        r#"{"model":"gpt-5","input":"challenge"}"#,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.expect("drain Cloudflare response");
    assert_eq!(challenge_hits.load(Ordering::SeqCst), 1);
    assert_eq!(fence_hits.load(Ordering::SeqCst), 1);
    assert_eq!(credential_control.read_count(), 2);

    credential_control.set_value(
        crate::credentials::SecretValue::new(b"generation-b".to_vec())
            .expect("valid rotated credential"),
    );
    retained
        .config
        .schedule_credential_refresh(fence_target.credential());
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let refresh_driver = retained.spawn_credential_refresh_driver(shutdown_rx);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if retained
                .config
                .capture()
                .await
                .credential_generation()
                .revision()
                > initial_generation_revision
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fence credential refresh should publish");
    shutdown_tx.send(true).expect("signal refresh shutdown");
    refresh_driver.await.expect("join refresh driver");

    assert_eq!(credential_control.read_count(), 3);
}

#[tokio::test]
async fn proxy_failover_retries_404_when_enabled() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            StatusCode::NOT_FOUND
        }),
    );
    let upstream1 = spawn_test_upstream(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let upstream2 = spawn_test_upstream(upstream2);

    let retry = retry_config(2, "400-599", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(
        vec![upstream1.upstream_config(), upstream2.upstream_config()],
        retry,
    );

    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::new();
    let resp = post_responses_json(&client, &proxy, r#"{"model":"gpt","input":"hi"}"#).await;

    assert_eq!(resp.status(), StatusCode::OK);
    // Two-layer model: retry current upstream first, then fail over.
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    let u2 = upstream2_hits.load(Ordering::SeqCst);
    assert!(
        matches!(u2, 1 | 2),
        "expected upstream2 hits to be 1..=2 (transport flake tolerance), got {u2}"
    );
}

#[tokio::test]
async fn proxy_does_not_failover_on_non_retryable_client_error_class() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "type": "invalid_request_error",
                        "message": "`tool_use` ids must be unique"
                    }
                })),
            )
        }),
    );
    let upstream1 = spawn_test_upstream(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let upstream2 = spawn_test_upstream(upstream2);

    let retry = retry_config(2, "400-599", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(
        vec![upstream1.upstream_config(), upstream2.upstream_config()],
        retry,
    );

    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::new();
    let resp = post_responses_json(&client, &proxy, r#"{"model":"gpt","input":"hi"}"#).await;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn proxy_skips_upstreams_that_do_not_support_model() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "err": "should not hit" })),
            )
        }),
    );
    let upstream1 = spawn_test_upstream(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "upstream": 2 })),
            )
        }),
    );
    let upstream2 = spawn_test_upstream(upstream2);

    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(
        vec![
            UpstreamConfig {
                supported_models: {
                    let mut m = HashMap::new();
                    m.insert("other-*".to_string(), true);
                    m
                },
                ..upstream1.upstream_config()
            },
            UpstreamConfig {
                supported_models: {
                    let mut m = HashMap::new();
                    m.insert("gpt-*".to_string(), true);
                    m
                },
                ..upstream2.upstream_config()
            },
        ],
        retry,
    );

    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::new();
    let resp = post_responses_json(&client, &proxy, r#"{"model":"gpt-4","input":"hi"}"#).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 0);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn proxy_no_routable_candidate_finishes_active_request() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|| async move {
            (
                StatusCode::OK,
                Json(serde_json::json!({ "err": "should not route" })),
            )
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_helper_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    state
        .set_provider_manual_eligibility(
            crate::runtime_identity::ProviderEndpointKey::new("codex", "test", "default"),
            crate::runtime_store::ProviderManualEligibility::Disabled,
            Some("test disables the only endpoint".to_string()),
            crate::logging::now_ms(),
        )
        .await
        .expect("commit manual endpoint eligibility");
    proxy
        .config
        .publish_provider_policy(state.capture_provider_policy_snapshot().await)
        .await
        .expect("publish provider policy snapshot");
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let resp = post_responses_json(&client, &proxy, r#"{"model":"gpt-5","input":"hi"}"#).await;

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert!(
        state.list_active_requests().await.is_empty(),
        "no-routable preparation failure must not leak active requests"
    );
    let finished = state.list_recent_finished(1).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].status_code, StatusCode::BAD_GATEWAY.as_u16());
}

#[tokio::test]
async fn proxy_applies_model_mapping_to_request_body() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));

    let hits = upstream_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |body: axum::body::Bytes| async move {
            hits.fetch_add(1, Ordering::SeqCst);
            let v: serde_json::Value =
                serde_json::from_slice(&body).expect("json body should parse");
            let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("");
            if model == "anthropic/claude-sonnet-4" {
                (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "model": model })),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_helper_config(
        vec![UpstreamConfig {
            supported_models: {
                let mut m = HashMap::new();
                m.insert("anthropic/claude-*".to_string(), true);
                m
            },
            model_mapping: {
                let mut m = HashMap::new();
                m.insert("claude-*".to_string(), "anthropic/claude-*".to_string());
                m
            },
            ..upstream.upstream_config()
        }],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::new();
    let resp = post_responses_json(
        &client,
        &proxy,
        r#"{"model":"claude-sonnet-4","input":"hi"}"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn proxy_previous_response_id_rectifier_retries_once_without_stale_id() {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen_bodies = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));

    let hits_for_route = hits.clone();
    let bodies_for_route = seen_bodies.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |body: String| {
            let hits = hits_for_route.clone();
            let seen_bodies = bodies_for_route.clone();
            async move {
                let value: serde_json::Value = serde_json::from_str(&body).expect("json body");
                seen_bodies.lock().expect("bodies lock").push(value.clone());
                let attempt = hits.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": {
                                "message": "No response found for previous_response_id resp-stale"
                            }
                        })),
                    )
                } else {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "ok": true,
                            "previous_present": value.get("previous_response_id").is_some()
                        })),
                    )
                }
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let resp = post_responses_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","previous_response_id":"resp-stale","input":"hi"}"#,
    )
    .await
    .error_for_status()
    .expect("rectified status")
    .json::<serde_json::Value>()
    .await
    .expect("json response");

    assert_eq!(resp["ok"].as_bool(), Some(true));
    assert_eq!(resp["previous_present"].as_bool(), Some(false));
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    let bodies = seen_bodies.lock().expect("bodies lock").clone();
    assert!(bodies[0].get("previous_response_id").is_some());
    assert!(bodies[1].get("previous_response_id").is_none());

    let finished = find_finished_request(&state, 10, |request| request.path == "/v1/responses")
        .await
        .expect("finished request");
    let retry = finished.retry.expect("retry info");
    assert_eq!(retry.attempts, 2);
    assert!(
        retry
            .route_attempts
            .iter()
            .any(|attempt| attempt.error_class.as_deref()
                == Some("codex_stale_previous_response_id"))
    );
}

#[tokio::test]
async fn proxy_codex_session_completion_fills_headers_and_prompt_cache_key() {
    let seen_headers = Arc::new(std::sync::Mutex::new(None::<HeaderMap>));
    let seen_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));

    let seen_headers_for_route = seen_headers.clone();
    let seen_body_for_route = seen_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |headers: HeaderMap, body: String| {
            let seen_headers = seen_headers_for_route.clone();
            let seen_body = seen_body_for_route.clone();
            async move {
                *seen_headers.lock().expect("headers lock") = Some(headers);
                *seen_body.lock().expect("body lock") =
                    Some(serde_json::from_str(&body).expect("json body"));
                Json(serde_json::json!({ "ok": true }))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let resp = post_responses_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","metadata":{"session_id":"meta-session-1"},"input":"hi"}"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let headers = seen_headers
        .lock()
        .expect("headers lock")
        .clone()
        .expect("upstream headers");
    assert_eq!(
        headers.get("session_id"),
        Some(&HeaderValue::from_static("meta-session-1"))
    );
    assert_eq!(
        headers.get("x-session-id"),
        Some(&HeaderValue::from_static("meta-session-1"))
    );
    assert_eq!(
        headers.get("session-id"),
        Some(&HeaderValue::from_static("meta-session-1"))
    );
    assert_eq!(
        headers.get("thread-id"),
        Some(&HeaderValue::from_static("meta-session-1"))
    );
    let body = seen_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("upstream body");
    assert_eq!(body["prompt_cache_key"].as_str(), Some("meta-session-1"));

    let finished = find_finished_request(&state, 10, |request| {
        request.session_id.as_deref() == Some("meta-session-1")
    })
    .await
    .expect("finished session request");
    assert_eq!(
        finished.session_identity_source,
        Some(SessionIdentitySource::MetadataSessionId)
    );
}

#[tokio::test]
async fn proxy_response_fixer_decodes_gzip_codex_response_json() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|| async move {
            const RESPONSE_JSON: &[u8] = br#"{"ok":true,"response":{"service_tier":"priority"}}"#;
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(RESPONSE_JSON).expect("gzip write");
            let compressed = encoder.finish().expect("gzip finish");
            let compressed_len = compressed.len();

            let mut response = Response::new(Body::from(compressed));
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response.headers_mut().insert(
                axum::http::header::CONTENT_ENCODING,
                HeaderValue::from_static("gzip"),
            );
            response.headers_mut().insert(
                axum::http::header::CONTENT_LENGTH,
                HeaderValue::from(compressed_len),
            );
            response.headers_mut().insert(
                axum::http::header::ETAG,
                HeaderValue::from_static("\"upstream-compressed\""),
            );
            response.headers_mut().insert(
                "content-md5",
                HeaderValue::from_static("upstream-compressed-md5"),
            );
            response.headers_mut().insert(
                "digest",
                HeaderValue::from_static("sha-256=upstream-compressed-digest"),
            );
            response
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::builder()
        .no_gzip()
        .build()
        .expect("client");
    let resp = post_responses_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","service_tier":"default","input":"hi"}"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::ETAG)
            .and_then(|value| value.to_str().ok()),
        Some("\"sha256-7ad8b6eca3b195969745d2e223c9e4436f699adb31d7aa3e8a7229ac8de0f84a\"")
    );
    assert!(
        !resp
            .headers()
            .contains_key(axum::http::header::CONTENT_ENCODING)
    );
    assert!(!resp.headers().contains_key("content-md5"));
    assert!(!resp.headers().contains_key("digest"));
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some("50")
    );
    let body = resp.bytes().await.expect("body");
    assert_eq!(
        body.as_ref(),
        br#"{"ok":true,"response":{"service_tier":"priority"}}"#
    );
    let value: serde_json::Value = serde_json::from_slice(body.as_ref()).expect("json response");
    assert_eq!(value["ok"].as_bool(), Some(true));
    assert_eq!(value["response"]["service_tier"].as_str(), Some("priority"));

    let finished = find_finished_request(&state, 10, |request| request.path == "/v1/responses")
        .await
        .expect("finished request");
    assert_eq!(finished.service_tier.as_deref(), Some("priority"));
}

#[tokio::test]
async fn proxy_preserves_gzip_encoding_for_untransformed_chat_response() {
    const RESPONSE_JSON: &[u8] = br#"{"id":"chatcmpl_1","choices":[]}"#;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(RESPONSE_JSON).expect("gzip write");
    let compressed = encoder.finish().expect("gzip finish");
    let upstream_compressed = compressed.clone();
    let upstream_accept_encoding = Arc::new(std::sync::Mutex::new(None::<String>));
    let seen_accept_encoding = upstream_accept_encoding.clone();
    let upstream = axum::Router::new().route(
        "/v1/chat/completions",
        post(move |headers: axum::http::HeaderMap| {
            let compressed = upstream_compressed.clone();
            let seen_accept_encoding = seen_accept_encoding.clone();
            async move {
                *seen_accept_encoding.lock().expect("accept encoding lock") = headers
                    .get(axum::http::header::ACCEPT_ENCODING)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                let mut response = Response::new(Body::from(compressed));
                *response.status_mut() = StatusCode::OK;
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response.headers_mut().insert(
                    axum::http::header::CONTENT_ENCODING,
                    HeaderValue::from_static("gzip"),
                );
                response.headers_mut().insert(
                    axum::http::header::ETAG,
                    HeaderValue::from_static("\"upstream-chat-gzip\""),
                );
                response
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let proxy = spawn_test_proxy(make_helper_config(vec![upstream.upstream_config()], retry));
    let client = reqwest::Client::builder()
        .no_gzip()
        .build()
        .expect("downstream client");

    let resp = client
        .post(proxy.url("/v1/chat/completions"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(r#"{"model":"gpt-5","messages":[{"role":"user","content":"hi"}]}"#)
        .send()
        .await
        .expect("send chat request");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("gzip")
    );
    assert_eq!(
        resp.headers()
            .get(axum::http::header::ETAG)
            .and_then(|value| value.to_str().ok()),
        Some("\"upstream-chat-gzip\"")
    );
    assert_eq!(resp.bytes().await.expect("body").as_ref(), compressed);
    assert_eq!(
        upstream_accept_encoding
            .lock()
            .expect("accept encoding lock")
            .as_deref(),
        Some("identity")
    );
}

#[tokio::test]
async fn proxy_preserves_upstream_validators_for_byte_for_byte_response() {
    const RESPONSE_JSON: &[u8] = br#"{"ok":true}"#;
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|| async move {
            let mut response = Response::new(Body::from(RESPONSE_JSON));
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response.headers_mut().insert(
                axum::http::header::ETAG,
                HeaderValue::from_static("\"upstream-raw\""),
            );
            response
                .headers_mut()
                .insert("content-md5", HeaderValue::from_static("upstream-raw-md5"));
            response.headers_mut().insert(
                "digest",
                HeaderValue::from_static("sha-256=upstream-raw-digest"),
            );
            response
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);
    let proxy = spawn_proxy_service(proxy_service(cfg));

    let resp =
        post_responses_json(&Client::new(), &proxy, r#"{"model":"gpt-5","input":"hi"}"#).await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::ETAG)
            .and_then(|value| value.to_str().ok()),
        Some("\"upstream-raw\"")
    );
    assert_eq!(
        resp.headers()
            .get("content-md5")
            .and_then(|value| value.to_str().ok()),
        Some("upstream-raw-md5")
    );
    assert_eq!(
        resp.headers()
            .get("digest")
            .and_then(|value| value.to_str().ok()),
        Some("sha-256=upstream-raw-digest")
    );
    assert_eq!(resp.bytes().await.expect("body").as_ref(), RESPONSE_JSON);
}

#[tokio::test]
async fn proxy_captures_codex_auth_json_until_runtime_reload() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let auth_field = format!(
        "CODEX_HELPER_TEST_RELAY_AUTH_{}",
        uuid::Uuid::new_v4().simple()
    );
    let provider_token = "provider-token-from-auth-json";
    let auth_json = serde_json::to_string_pretty(&serde_json::json!({
        auth_field.as_str(): provider_token,
    }))
    .expect("serialize Codex auth.json");
    write_text_file(&codex_home.join("auth.json"), &auth_json);

    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }

    let upstream_authorization = Arc::new(std::sync::Mutex::new(Vec::<Option<String>>::new()));
    let seen_authorization = Arc::clone(&upstream_authorization);
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |headers: axum::http::HeaderMap| {
            let seen_authorization = Arc::clone(&seen_authorization);
            async move {
                seen_authorization.lock().expect("authorization lock").push(
                    headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_string),
                );
                Json(serde_json::json!({ "ok": true }))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let mut upstream_config = upstream.upstream_config();
    upstream_config.auth.auth_token_env = Some(auth_field.clone());
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let proxy = spawn_test_proxy(make_helper_config(vec![upstream_config], retry));

    let response = local_http_test_client()
        .post(proxy.responses_url())
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::AUTHORIZATION,
            "Bearer codex-client-account-token",
        )
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send through proxy");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        upstream_authorization
            .lock()
            .expect("authorization lock")
            .as_slice(),
        [Some("Bearer provider-token-from-auth-json".to_string())]
    );

    let rotated_token = "rotated-provider-token-from-auth-json";
    let rotated_auth_json = serde_json::to_string_pretty(&serde_json::json!({
        auth_field.as_str(): rotated_token,
    }))
    .expect("serialize rotated Codex auth.json");
    write_text_file(&codex_home.join("auth.json"), &rotated_auth_json);
    tokio::time::sleep(Duration::from_millis(40)).await;

    let rotated_response = local_http_test_client()
        .post(proxy.responses_url())
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::AUTHORIZATION,
            "Bearer codex-client-account-token",
        )
        .body(r#"{"model":"gpt-5","input":"after rotation"}"#)
        .send()
        .await
        .expect("send through proxy after auth rotation");

    assert_eq!(rotated_response.status(), StatusCode::OK);
    assert_eq!(
        upstream_authorization
            .lock()
            .expect("authorization lock")
            .as_slice(),
        [
            Some("Bearer provider-token-from-auth-json".to_string()),
            Some("Bearer provider-token-from-auth-json".to_string()),
        ]
    );
}

#[tokio::test]
async fn proxy_failover_keeps_the_request_credential_generation_when_auth_json_changes() {
    const RELAY_HOST: &str = "relay-auth-missing-env.test";

    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let auth_path = codex_home.join("auth.json");
    let missing_reference = format!(
        "CODEX_HELPER_TEST_MISSING_RELAY_AUTH_{}",
        uuid::Uuid::new_v4().simple()
    );
    let auth_json = serde_json::to_string_pretty(&serde_json::json!({
        missing_reference.as_str(): "temporary-provider-token",
    }))
    .expect("serialize Codex auth.json");
    write_text_file(&auth_path, &auth_json);
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }

    let primary_hits = Arc::new(AtomicUsize::new(0));
    let primary_hits_for_route = primary_hits.clone();
    let auth_path_for_route = auth_path.clone();
    let primary = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = primary_hits_for_route.clone();
            let auth_path = auth_path_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                write_text_file(&auth_path, "{}");
                tokio::time::sleep(Duration::from_millis(40)).await;
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": "primary failed" })),
                )
            }
        }),
    ));

    let backup_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits_for_route = backup_hits.clone();
    let backup = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = backup_hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({ "unexpected": true }))
            }
        }),
    ));
    let mut backup_config = backup.upstream_config();
    backup_config.base_url = format!("http://{RELAY_HOST}:{}/v1", backup.addr.port());
    backup_config.auth.auth_token_env = Some(missing_reference.clone());
    let proxy_client = crate::proxy::upstream_http_client_builder()
        .no_proxy()
        .resolve(RELAY_HOST, backup.addr)
        .build()
        .expect("build remote relay test client");
    let proxy = spawn_proxy_service(ProxyService::new(
        proxy_client,
        Arc::new(make_helper_config(
            vec![primary.upstream_config(), backup_config],
            retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
        )),
        "codex",
    ));

    let response = local_http_test_client()
        .post(proxy.responses_url())
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::AUTHORIZATION,
            "Bearer codex-client-account-token",
        )
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send through proxy");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn proxy_fails_closed_before_http_upstream_when_auth_header_is_invalid() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = upstream_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({ "unexpected": true }))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let mut upstream_config = upstream.upstream_config();
    upstream_config.auth.auth_token = Some("invalid\r\nbearer".to_string().into());
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let proxy = spawn_test_proxy(make_helper_config(vec![upstream_config], retry));

    let response = local_http_test_client()
        .post(proxy.responses_url())
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::AUTHORIZATION,
            "Bearer codex-client-account-token",
        )
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send through proxy");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn proxy_fails_closed_before_remote_http_upstream_when_auth_is_unconfigured() {
    const RELAY_HOST: &str = "relay-auth-default.test";

    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = upstream_hits.clone();
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({ "unexpected": true }))
            }
        }),
    ));
    let mut upstream_config = upstream.upstream_config();
    upstream_config.base_url = format!("http://{RELAY_HOST}:{}/v1", upstream.addr.port());
    let proxy_client = crate::proxy::upstream_http_client_builder()
        .no_proxy()
        .resolve(RELAY_HOST, upstream.addr)
        .build()
        .expect("build remote relay test client");
    let service = ProxyService::new(
        proxy_client,
        Arc::new(make_helper_config(
            vec![upstream_config],
            retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
        )),
        "codex",
    );
    let state = service.state.clone();
    let proxy = spawn_proxy_service(service);

    let response = local_http_test_client()
        .post(proxy.responses_url())
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::AUTHORIZATION,
            "Bearer codex-client-account-token",
        )
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send through proxy");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);

    let failed = find_finished_request(&state, 10, |request| {
        request.status_code == StatusCode::SERVICE_UNAVAILABLE.as_u16()
    })
    .await
    .expect("credential-blocked request");
    let retry = failed.retry.as_ref().expect("route decision trace");
    assert!(!retry.route_attempts.is_empty());
    assert!(retry.route_attempts.iter().all(|attempt| {
        attempt.decision == "route_unavailable"
            && attempt.cooldown_secs.is_none()
            && attempt.cooldown_reason.is_none()
    }));
}

#[tokio::test]
async fn proxy_allows_explicit_anonymous_remote_http_upstream_without_client_account_headers() {
    const RELAY_HOST: &str = "relay-auth-opt-in.test";

    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let upstream_headers = Arc::new(std::sync::Mutex::new(None::<axum::http::HeaderMap>));
    let hits_for_route = upstream_hits.clone();
    let headers_for_route = upstream_headers.clone();
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move |headers: axum::http::HeaderMap| {
            let hits = hits_for_route.clone();
            let seen_headers = headers_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                *seen_headers.lock().expect("upstream headers lock") = Some(headers);
                Json(serde_json::json!({ "ok": true }))
            }
        }),
    ));
    let mut upstream_config = upstream.upstream_config();
    upstream_config.base_url = format!("http://{RELAY_HOST}:{}/v1", upstream.addr.port());
    upstream_config.auth.allow_anonymous = Some(true);
    let proxy_client = crate::proxy::upstream_http_client_builder()
        .no_proxy()
        .resolve(RELAY_HOST, upstream.addr)
        .build()
        .expect("build remote relay test client");
    let proxy = spawn_proxy_service(ProxyService::new(
        proxy_client,
        Arc::new(make_helper_config(
            vec![upstream_config],
            retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
        )),
        "codex",
    ));

    let response = local_http_test_client()
        .post(proxy.responses_url())
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(
            axum::http::header::AUTHORIZATION,
            "Bearer codex-client-account-token",
        )
        .header("chatgpt-account-id", "codex-client-account")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send through proxy");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);
    let headers = upstream_headers
        .lock()
        .expect("upstream headers lock")
        .clone()
        .expect("captured upstream headers");
    assert!(!headers.contains_key(axum::http::header::AUTHORIZATION));
    assert!(!headers.contains_key("chatgpt-account-id"));
}

#[tokio::test]
async fn proxy_does_not_follow_cross_origin_redirect_with_credentials() {
    let target_hits = Arc::new(AtomicUsize::new(0));
    let target_headers = Arc::new(std::sync::Mutex::new(None::<axum::http::HeaderMap>));
    let target_hits_seen = target_hits.clone();
    let target_headers_seen = target_headers.clone();
    let target = axum::Router::new().route(
        "/v1/responses",
        axum::routing::any(move |headers: axum::http::HeaderMap| {
            let target_hits_seen = target_hits_seen.clone();
            let target_headers_seen = target_headers_seen.clone();
            async move {
                target_hits_seen.fetch_add(1, Ordering::SeqCst);
                *target_headers_seen.lock().expect("target headers lock") = Some(headers);
                Json(serde_json::json!({ "target": true }))
            }
        }),
    );
    let target = spawn_test_upstream(target);

    let source_headers = Arc::new(std::sync::Mutex::new(None::<axum::http::HeaderMap>));
    let source_headers_seen = source_headers.clone();
    let redirect_location = format!("{}/responses", target.upstream_config().base_url);
    let source = axum::Router::new().route(
        "/v1/responses",
        post(move |headers: axum::http::HeaderMap| {
            let source_headers_seen = source_headers_seen.clone();
            let redirect_location = redirect_location.clone();
            async move {
                *source_headers_seen.lock().expect("source headers lock") = Some(headers);
                let mut response = Response::new(Body::empty());
                *response.status_mut() = StatusCode::FOUND;
                response.headers_mut().insert(
                    axum::http::header::LOCATION,
                    HeaderValue::try_from(redirect_location).expect("redirect location"),
                );
                response
            }
        }),
    );
    let source = spawn_test_upstream(source);
    let mut source_config = source.upstream_config();
    source_config.auth = UpstreamAuth {
        auth_token: Some("relay-bearer-secret".to_string().into()),
        auth_token_env: None,
        auth_token_ref: None,
        api_key: Some("relay-api-key-secret".to_string().into()),
        api_key_env: None,
        api_key_ref: None,
        allow_anonymous: None,
    };
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let proxy = spawn_test_proxy(make_helper_config(vec![source_config], retry));
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("downstream client");

    let resp = client
        .post(proxy.url("/v1/responses"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(axum::http::header::COOKIE, "session=client-secret")
        .header("x-forwarded-api-key", "client-forwarded-secret")
        .header("x-codex-helper-admin-token", "client-admin-secret")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send through proxy");

    assert_eq!(resp.status(), StatusCode::FOUND);
    let source_headers = source_headers
        .lock()
        .expect("source headers lock")
        .clone()
        .expect("source request");
    assert_eq!(
        source_headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer relay-bearer-secret")
    );
    assert_eq!(
        source_headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok()),
        Some("relay-api-key-secret")
    );
    assert!(!source_headers.contains_key(axum::http::header::COOKIE));
    assert!(!source_headers.contains_key("x-forwarded-api-key"));
    assert!(!source_headers.contains_key("x-codex-helper-admin-token"));
    assert_eq!(target_hits.load(Ordering::SeqCst), 0);
    assert!(
        target_headers
            .lock()
            .expect("target headers lock")
            .is_none()
    );
}

#[tokio::test]
async fn proxy_response_fixer_converts_compact_sse_terminal_response_to_json() {
    let upstream = axum::Router::new().route(
        "/v1/responses/compact",
        post(|| async move {
            let mut response = Response::new(Body::from(concat!(
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ignored\"}]}}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_compact\",\"output\":[{\"type\":\"compaction\",\"encrypted_content\":\"summary\"}]}}\n\n",
                "data: [DONE]\n\n",
            )));
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            response
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);
    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::new();
    let resp = post_compact_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = resp.bytes().await.expect("body");
    let value: serde_json::Value = serde_json::from_slice(body.as_ref()).expect("json response");
    assert_eq!(value["id"].as_str(), Some("resp_compact"));
    assert_eq!(value["output"][0]["type"].as_str(), Some("compaction"));
    assert_eq!(
        value["output"][0]["encrypted_content"].as_str(),
        Some("summary")
    );
}

#[tokio::test]
async fn proxy_response_fixer_converts_compact_sse_failed_terminal_to_bad_gateway_json() {
    let upstream = axum::Router::new().route(
        "/v1/responses/compact",
        post(|| async move {
            let mut response = Response::new(Body::from(concat!(
                "event: response.failed\n",
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"compact rejected\"}}}\n\n",
            )));
            *response.status_mut() = StatusCode::OK;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            response
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);
    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::new();
    let resp = post_compact_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = resp.bytes().await.expect("body");
    let value: serde_json::Value = serde_json::from_slice(body.as_ref()).expect("json response");
    assert_eq!(value["error"]["message"].as_str(), Some("compact rejected"));
}

#[tokio::test]
async fn proxy_service_tier_log_preserves_requested_effective_and_actual_values() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let seen_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let seen_body_for_route = seen_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |body: String| {
            let seen_body = seen_body_for_route.clone();
            async move {
                let value: serde_json::Value = serde_json::from_str(&body).expect("json body");
                *seen_body.lock().expect("body lock") = Some(value);
                Json(serde_json::json!({
                    "id": "resp-1",
                    "response": { "service_tier": "priority" }
                }))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(vec![upstream.upstream_config()], retry);
    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::new();
    let resp = post_responses_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","service_tier":"default","input":"hi"}"#,
    )
    .await
    .error_for_status()
    .expect("status")
    .json::<serde_json::Value>()
    .await
    .expect("json response");

    assert_eq!(resp["response"]["service_tier"].as_str(), Some("priority"));
    let upstream_body = seen_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("upstream body");
    assert_eq!(upstream_body["service_tier"].as_str(), Some("default"));

    let request_log =
        std::fs::read_to_string(crate::logging::request_log_path()).expect("read request log");
    let record: serde_json::Value = request_log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|record| record["path"].as_str() == Some("/v1/responses"))
        .expect("request log record");

    assert_eq!(
        record["service_tier"]["requested"].as_str(),
        Some("default")
    );
    assert_eq!(
        record["service_tier"]["effective"].as_str(),
        Some("default")
    );
    assert_eq!(record["service_tier"]["actual"].as_str(), Some("priority"));
}
