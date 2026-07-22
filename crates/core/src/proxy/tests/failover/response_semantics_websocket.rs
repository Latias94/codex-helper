use super::*;
use crate::proxy::tests::harness::TestUpstreamServer;
use crate::state::{SessionRouteAffinityControlCommand, session_route_affinity_revision};

const WS_PROVIDER_ENDPOINT_HEADER: &str = "x-codex-helper-provider-endpoint";

fn enable_responses_websocket_for_test(codex_home: &std::path::Path) {
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );
}

fn single_provider_websocket_config(
    upstream_addr: std::net::SocketAddr,
    scheduling_preset: SchedulingPreset,
    max_concurrent_requests: u32,
) -> HelperConfig {
    let mut routing = RouteGraphConfig::ordered_failover(vec!["single".to_string()]);
    routing.affinity_policy = crate::config::RouteAffinityPolicy::FallbackSticky;
    routing.scheduling_preset = scheduling_preset;
    HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([(
                "single".to_string(),
                ProviderConfig {
                    base_url: Some(format!("http://{upstream_addr}/v1")),
                    inline_auth: UpstreamAuth::default(),
                    limits: ProviderConcurrencyLimits {
                        max_concurrent_requests: Some(max_concurrent_requests),
                        limit_group: None,
                    },
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    }
}

async fn send_successful_websocket_response(
    socket: &mut axum::extract::ws::WebSocket,
    response_id: &str,
) {
    let created = serde_json::json!({
        "type": "response.created",
        "response": { "id": response_id }
    });
    let completed = serde_json::json!({
        "type": "response.completed",
        "response": { "id": response_id }
    });
    let _ = socket
        .send(axum::extract::ws::Message::Text(created.to_string().into()))
        .await;
    let _ = socket
        .send(axum::extract::ws::Message::Text(
            completed.to_string().into(),
        ))
        .await;
}

struct CountingWebSocketUpstream {
    server: TestUpstreamServer,
    frames: Arc<AtomicUsize>,
    frame_received: Arc<tokio::sync::Notify>,
}

fn spawn_counting_websocket_upstream(response_id: &'static str) -> CountingWebSocketUpstream {
    let frames = Arc::new(AtomicUsize::new(0));
    let frame_received = Arc::new(tokio::sync::Notify::new());
    let frames_for_route = Arc::clone(&frames);
    let received_for_route = Arc::clone(&frame_received);
    let app = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let frames = Arc::clone(&frames_for_route);
            let received = Arc::clone(&received_for_route);
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if !matches!(
                            message,
                            axum::extract::ws::Message::Text(_)
                                | axum::extract::ws::Message::Binary(_)
                        ) {
                            continue;
                        }
                        frames.fetch_add(1, Ordering::SeqCst);
                        received.notify_one();
                        send_successful_websocket_response(&mut socket, response_id).await;
                    }
                })
            }
        }),
    );
    CountingWebSocketUpstream {
        server: spawn_test_upstream(app),
        frames,
        frame_received,
    }
}

async fn send_failed_websocket_response(
    socket: &mut axum::extract::ws::WebSocket,
    response_id: &str,
) {
    let created = serde_json::json!({
        "type": "response.created",
        "response": { "id": response_id }
    });
    let failed = serde_json::json!({
        "type": "response.failed",
        "response": {
            "id": response_id,
            "error": { "message": "upstream rejected" }
        }
    });
    let _ = socket
        .send(axum::extract::ws::Message::Text(created.to_string().into()))
        .await;
    let _ = socket
        .send(axum::extract::ws::Message::Text(failed.to_string().into()))
        .await;
}

async fn send_logical_failure_websocket_response(
    socket: &mut axum::extract::ws::WebSocket,
    response_id: &str,
    event_type: &str,
) {
    let created = serde_json::json!({
        "type": "response.created",
        "response": { "id": response_id }
    });
    let terminal = serde_json::json!({
        "type": event_type,
        "response": { "id": response_id }
    });
    let _ = socket
        .send(axum::extract::ws::Message::Text(created.to_string().into()))
        .await;
    let _ = socket
        .send(axum::extract::ws::Message::Text(
            terminal.to_string().into(),
        ))
        .await;
}

type TestWebSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn spawn_single_provider_websocket_proxy(
    upstream_addr: std::net::SocketAddr,
    scheduling_preset: SchedulingPreset,
    max_concurrent_requests: u32,
) -> (
    ProxyService,
    std::net::SocketAddr,
    tokio::task::JoinHandle<()>,
) {
    let source =
        single_provider_websocket_config(upstream_addr, scheduling_preset, max_concurrent_requests);
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let retained = proxy.clone();
    let app = crate::proxy::router(proxy);
    let (addr, handle) = spawn_axum_server(app);
    (retained, addr, handle)
}

async fn connect_test_websocket(
    proxy_addr: std::net::SocketAddr,
    session_id: &str,
) -> TestWebSocket {
    let request = test_websocket_request(proxy_addr, session_id);
    tokio_tungstenite::connect_async(request)
        .await
        .expect("websocket handshake")
        .0
}

fn test_websocket_request(
    proxy_addr: std::net::SocketAddr,
    session_id: &str,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("ws request");
    request.headers_mut().insert(
        "session-id",
        HeaderValue::from_str(session_id).expect("session header"),
    );
    request
}

async fn send_test_response_create(socket: &mut TestWebSocket, input: &str) {
    let message = serde_json::json!({
        "type": "response.create",
        "model": "gpt-5",
        "input": input,
    });
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            message.to_string().into(),
        ))
        .await
        .expect("send response.create");
}

async fn next_test_websocket_json(socket: &mut TestWebSocket) -> serde_json::Value {
    let message = socket
        .next()
        .await
        .expect("websocket event")
        .expect("websocket event frame");
    serde_json::from_str(message.to_text().expect("websocket text event"))
        .expect("websocket json event")
}

async fn wait_for_credential_generation_after(proxy: &ProxyService, revision: u64) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if proxy
                .config
                .capture()
                .await
                .credential_generation()
                .revision()
                > revision
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("credential rotation should publish a new generation");
}

#[tokio::test]
async fn responses_websocket_fails_closed_before_upstream_when_auth_reference_is_missing() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let handshake_hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = handshake_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(|mut socket| async move { while socket.recv().await.is_some() {} })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let mut source = single_provider_websocket_config(upstream_addr, SchedulingPreset::Balanced, 1);
    let missing_auth_reference = format!(
        "CODEX_HELPER_TEST_MISSING_WS_AUTH_{}",
        uuid::Uuid::new_v4().simple()
    );
    source
        .codex
        .providers
        .get_mut("single")
        .expect("single provider")
        .inline_auth
        .auth_token_env = Some(missing_auth_reference.clone());
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let error =
        tokio_tungstenite::connect_async(test_websocket_request(proxy_addr, "ws-missing-auth"))
            .await
            .expect_err("missing auth must reject the downstream handshake");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP authentication failure, got {error:?}");
    };

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = String::from_utf8_lossy(response.body().as_deref().unwrap_or_default());
    assert_eq!(body, "configured upstream credentials are unavailable");
    assert!(!body.contains(missing_auth_reference.as_str()));
    assert_eq!(handshake_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_fails_closed_before_upstream_when_auth_header_is_invalid() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let handshake_hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = handshake_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(|mut socket| async move { while socket.recv().await.is_some() {} })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let mut source = single_provider_websocket_config(upstream_addr, SchedulingPreset::Balanced, 1);
    source
        .codex
        .providers
        .get_mut("single")
        .expect("single provider")
        .inline_auth
        .auth_token = Some("invalid\r\nbearer".to_string().into());
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let error =
        tokio_tungstenite::connect_async(test_websocket_request(proxy_addr, "ws-invalid-auth"))
            .await
            .expect_err("invalid auth must reject the downstream handshake");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP authentication failure, got {error:?}");
    };

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.body().as_deref().unwrap_or_default(),
        b"configured upstream credentials are unavailable"
    );
    assert_eq!(handshake_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_never_replays_auth_handshake_failures_and_refreshes_once() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    for (status, cloudflare_challenge, expected_native_reads) in [
        (StatusCode::UNAUTHORIZED, false, 2),
        (StatusCode::FORBIDDEN, false, 2),
        (StatusCode::FORBIDDEN, true, 1),
    ] {
        let primary_hits = Arc::new(AtomicUsize::new(0));
        let primary_authorizations = Arc::new(std::sync::Mutex::new(Vec::new()));
        let hits_for_route = Arc::clone(&primary_hits);
        let authorizations_for_route = Arc::clone(&primary_authorizations);
        let primary = axum::Router::new().route(
            "/v1/responses",
            get(move |headers: HeaderMap| {
                let hits = Arc::clone(&hits_for_route);
                let authorizations = Arc::clone(&authorizations_for_route);
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    authorizations
                        .lock()
                        .expect("authorization capture lock")
                        .push(
                            headers
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_owned),
                        );
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
                            Json(serde_json::json!({
                                "error": {
                                    "code": "API_KEY_REQUIRED",
                                    "message": "API key is required in Authorization header"
                                }
                            })),
                        ))
                    }
                }
            }),
        );
        let (primary_addr, primary_handle) = spawn_axum_server(primary);

        let backup_hits = Arc::new(AtomicUsize::new(0));
        let backup_hits_for_route = Arc::clone(&backup_hits);
        let backup = axum::Router::new().route(
            "/v1/responses",
            get(move || {
                let hits = Arc::clone(&backup_hits_for_route);
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }),
        );
        let (backup_addr, backup_handle) = spawn_axum_server(backup);

        let retry = RetryConfig {
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
        };
        let source = HelperConfig {
            retry,
            codex: ServiceRouteConfig {
                providers: std::collections::BTreeMap::from([
                    (
                        "primary".to_string(),
                        ProviderConfig {
                            base_url: Some(format!("http://{primary_addr}/v1")),
                            auth: UpstreamAuth {
                                auth_token_ref: Some(crate::config::CredentialRef::Native {
                                    name: "relay.primary".to_string(),
                                }),
                                ..UpstreamAuth::default()
                            },
                            ..ProviderConfig::default()
                        },
                    ),
                    (
                        "backup".to_string(),
                        ProviderConfig {
                            base_url: Some(format!("http://{backup_addr}/v1")),
                            auth: UpstreamAuth {
                                allow_anonymous: Some(true),
                                ..UpstreamAuth::default()
                            },
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
        credential_control.set_value(
            crate::credentials::SecretValue::new(b"generation-b".to_vec())
                .expect("valid rotated credential"),
        );

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let refresh_driver = proxy.spawn_credential_refresh_driver(shutdown_rx);
        let app = crate::proxy::router(proxy);
        let (proxy_addr, proxy_handle) = spawn_axum_server(app);

        let mut request = test_websocket_request(proxy_addr, "ws-auth-handshake-failure");
        request.headers_mut().insert(
            WS_PROVIDER_ENDPOINT_HEADER,
            HeaderValue::from_static("codex/primary/default"),
        );
        let error = tokio_tungstenite::connect_async(request)
            .await
            .expect_err("upstream authentication failure must reject the handshake");
        let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
            panic!("expected HTTP authentication failure, got {error:?}");
        };
        assert_eq!(
            response.status(),
            status,
            "unexpected handshake body: {}",
            String::from_utf8_lossy(response.body().as_deref().unwrap_or_default())
        );

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

        assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
        assert_eq!(backup_hits.load(Ordering::SeqCst), 0);
        assert_eq!(
            credential_control.read_count(),
            expected_native_reads,
            "Cloudflare challenge pages are transport failures, not credential evidence"
        );
        assert_eq!(
            *primary_authorizations
                .lock()
                .expect("authorization capture lock"),
            vec![Some("Bearer generation-a".to_string())]
        );

        shutdown_tx.send(true).expect("signal refresh shutdown");
        refresh_driver.await.expect("join refresh driver");
        proxy_handle.abort();
        primary_handle.abort();
        backup_handle.abort();
    }

    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_generic_handshake_rejections_do_not_refresh_or_cool_http_inference() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    for (status, body) in [
        (
            StatusCode::UNAUTHORIZED,
            r#"{"error":{"code":"websocket_auth_scheme_unsupported","message":"use query authorization for websocket upgrade"}}"#,
        ),
        (
            StatusCode::UPGRADE_REQUIRED,
            r#"{"error":{"message":"websocket beta required"}}"#,
        ),
    ] {
        let inference_hits = Arc::new(AtomicUsize::new(0));
        let inference_hits_for_route = Arc::clone(&inference_hits);
        let upstream = axum::Router::new().route(
            "/v1/responses",
            get(move || async move {
                let mut response = Response::new(Body::from(body));
                *response.status_mut() = status;
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response
            })
            .post(move || {
                let inference_hits = Arc::clone(&inference_hits_for_route);
                async move {
                    inference_hits.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({
                        "id": "resp-http-healthy",
                        "object": "response",
                        "output": []
                    }))
                }
            }),
        );
        let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
        let mut source =
            single_provider_websocket_config(upstream_addr, SchedulingPreset::Balanced, 1);
        source
            .codex
            .providers
            .get_mut("single")
            .expect("single provider")
            .auth = UpstreamAuth {
            auth_token_ref: Some(crate::config::CredentialRef::Native {
                name: "relay.single".to_string(),
            }),
            ..UpstreamAuth::default()
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
        let app = crate::proxy::router(proxy);
        let (proxy_addr, proxy_handle) = spawn_axum_server(app);

        let error = tokio_tungstenite::connect_async(test_websocket_request(
            proxy_addr,
            "ws-generic-handshake-rejection",
        ))
        .await
        .expect_err("upstream handshake rejection must be returned to the client");
        let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
            panic!("expected HTTP handshake rejection, got {error:?}");
        };
        assert_eq!(response.status(), status);

        let inference = reqwest::Client::new()
            .post(format!("http://{proxy_addr}/v1/responses"))
            .header("content-type", "application/json")
            .body(r#"{"model":"gpt-5","input":"HTTP remains healthy"}"#)
            .send()
            .await
            .expect("send HTTP inference after WebSocket handshake rejection");
        assert_eq!(inference.status(), StatusCode::OK, "status={status}");
        let _ = inference.bytes().await.expect("read inference response");
        assert_eq!(inference_hits.load(Ordering::SeqCst), 1, "status={status}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            credential_control.read_count(),
            1,
            "status={status} must not schedule a native credential refresh"
        );

        shutdown_tx.send(true).expect("signal refresh shutdown");
        refresh_driver.await.expect("join refresh driver");
        proxy_handle.abort();
        upstream_handle.abort();
    }

    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_requires_reconnect_when_another_endpoint_credential_rotates() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let primary_handshakes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let primary_frames = Arc::new(AtomicUsize::new(0));
    let handshakes_for_route = Arc::clone(&primary_handshakes);
    let frames_for_route = Arc::clone(&primary_frames);
    let primary = axum::Router::new().route(
        "/v1/responses",
        get(
            move |ws: axum::extract::ws::WebSocketUpgrade, headers: HeaderMap| {
                let handshakes = Arc::clone(&handshakes_for_route);
                let frames = Arc::clone(&frames_for_route);
                async move {
                    handshakes.lock().expect("handshake capture lock").push(
                        headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_owned),
                    );
                    ws.on_upgrade(move |mut socket| async move {
                        while let Some(Ok(message)) = socket.recv().await {
                            if !matches!(
                                message,
                                axum::extract::ws::Message::Text(_)
                                    | axum::extract::ws::Message::Binary(_)
                            ) {
                                continue;
                            }
                            frames.fetch_add(1, Ordering::SeqCst);
                            send_successful_websocket_response(&mut socket, "resp-primary").await;
                        }
                    })
                }
            },
        ),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let secondary_handshakes = Arc::new(AtomicUsize::new(0));
    let secondary_handshakes_for_route = Arc::clone(&secondary_handshakes);
    let secondary = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let handshakes = Arc::clone(&secondary_handshakes_for_route);
            async move {
                handshakes.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(|mut socket| async move { while socket.recv().await.is_some() {} })
            }
        }),
    );
    let (secondary_addr, secondary_handle) = spawn_axum_server(secondary);

    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        auth: UpstreamAuth {
                            auth_token_ref: Some(crate::config::CredentialRef::Native {
                                name: "relay.primary".to_string(),
                            }),
                            ..UpstreamAuth::default()
                        },
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "secondary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{secondary_addr}/v1")),
                        auth: UpstreamAuth {
                            auth_token_ref: Some(crate::config::CredentialRef::Native {
                                name: "relay.secondary".to_string(),
                            }),
                            ..UpstreamAuth::default()
                        },
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "primary".to_string(),
                "secondary".to_string(),
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
    let runtime_store =
        Arc::new(crate::runtime_store::RuntimeStore::open_in_memory().expect("open runtime store"));
    let proxy = ProxyService::new_with_runtime_store_and_credential_sources(
        Client::new(),
        Arc::new(source),
        "codex",
        runtime_store,
        credential_sources,
    )
    .expect("build credential-backed proxy");
    let retained = proxy.clone();
    let before = retained.config.capture().await;
    let route_plan = before
        .capture_route_plan("codex", &crate::routing_ir::RouteRequestContext::default())
        .expect("capture route plan")
        .expect("route plan");
    let secondary_candidate = route_plan
        .template()
        .candidates
        .iter()
        .find(|candidate| candidate.provider_id == "secondary")
        .expect("secondary candidate");
    let secondary_target = route_plan
        .template()
        .capture_candidate(secondary_candidate)
        .expect("capture secondary credential");
    let initial_generation_revision = before.credential_generation().revision();
    assert_eq!(credential_control.read_count(), 2);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let refresh_driver = retained.spawn_credential_refresh_driver(shutdown_rx);
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let mut request = test_websocket_request(proxy_addr, "ws-other-credential-rotation");
    request.headers_mut().insert(
        WS_PROVIDER_ENDPOINT_HEADER,
        HeaderValue::from_static("codex/primary/default"),
    );
    let mut socket = tokio_tungstenite::connect_async(request)
        .await
        .expect("initial WebSocket handshake")
        .0;

    credential_control.set_value(
        crate::credentials::SecretValue::new(b"generation-b".to_vec())
            .expect("valid rotated credential"),
    );
    retained
        .config
        .schedule_credential_refresh(secondary_target.credential());
    wait_for_credential_generation_after(&retained, initial_generation_revision).await;
    assert_eq!(credential_control.read_count(), 3);

    send_test_response_create(&mut socket, "after other endpoint rotation").await;
    let rejection = next_test_websocket_json(&mut socket).await;
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_reconnect_required")
    );
    assert_eq!(primary_frames.load(Ordering::SeqCst), 0);
    assert_eq!(secondary_handshakes.load(Ordering::SeqCst), 0);
    assert_eq!(
        *primary_handshakes.lock().expect("handshake capture lock"),
        [Some("Bearer generation-a".to_string())]
    );

    shutdown_tx.send(true).expect("signal refresh shutdown");
    refresh_driver.await.expect("join refresh driver");
    proxy_handle.abort();
    primary_handle.abort();
    secondary_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_finishes_active_generation_then_reconnects_for_rotated_credential() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let handshake_authorizations = Arc::new(std::sync::Mutex::new(Vec::new()));
    let create_hits = Arc::new(AtomicUsize::new(0));
    let first_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let authorizations_for_route = Arc::clone(&handshake_authorizations);
    let hits_for_route = Arc::clone(&create_hits);
    let started_for_route = Arc::clone(&first_started);
    let release_for_route = Arc::clone(&release_first);
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(
            move |ws: axum::extract::ws::WebSocketUpgrade, headers: HeaderMap| {
                let authorizations = Arc::clone(&authorizations_for_route);
                let hits = Arc::clone(&hits_for_route);
                let first_started = Arc::clone(&started_for_route);
                let release_first = Arc::clone(&release_for_route);
                async move {
                    authorizations
                        .lock()
                        .expect("authorization capture lock")
                        .push(
                            headers
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_owned),
                        );
                    ws.on_upgrade(move |mut socket| async move {
                        while let Some(Ok(message)) = socket.recv().await {
                            if !matches!(
                                message,
                                axum::extract::ws::Message::Text(_)
                                    | axum::extract::ws::Message::Binary(_)
                            ) {
                                continue;
                            }
                            let hit = hits.fetch_add(1, Ordering::SeqCst);
                            if hit == 0 {
                                first_started.notify_one();
                                release_first.notified().await;
                            }
                            send_successful_websocket_response(
                                &mut socket,
                                format!("resp-generation-{hit}").as_str(),
                            )
                            .await;
                        }
                    })
                }
            },
        ),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let mut source = single_provider_websocket_config(upstream_addr, SchedulingPreset::Balanced, 2);
    source
        .codex
        .providers
        .get_mut("single")
        .expect("single provider")
        .auth = UpstreamAuth {
        auth_token_ref: Some(crate::config::CredentialRef::Native {
            name: "relay.primary".to_string(),
        }),
        ..UpstreamAuth::default()
    };
    let (credential_sources, credential_control) =
        crate::credentials::CredentialSourceCapabilities::test_native(
            crate::credentials::SecretValue::new(b"generation-a".to_vec())
                .expect("valid initial credential"),
        );
    let runtime_store =
        Arc::new(crate::runtime_store::RuntimeStore::open_in_memory().expect("open runtime store"));
    let proxy = ProxyService::new_with_runtime_store_and_credential_sources(
        Client::new(),
        Arc::new(source),
        "codex",
        runtime_store,
        credential_sources,
    )
    .expect("build credential-backed proxy");
    let retained = proxy.clone();
    let before = retained.config.capture().await;
    let route_plan = before
        .capture_route_plan("codex", &crate::routing_ir::RouteRequestContext::default())
        .expect("capture route plan")
        .expect("route plan");
    let old_target = route_plan
        .template()
        .capture_candidate(
            route_plan
                .template()
                .candidates
                .first()
                .expect("single candidate"),
        )
        .expect("capture initial credential");
    let initial_generation_revision = before.credential_generation().revision();
    assert_eq!(credential_control.read_count(), 1);

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let refresh_driver = retained.spawn_credential_refresh_driver(shutdown_rx);
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let mut old_socket = connect_test_websocket(proxy_addr, "ws-generation-a").await;
    send_test_response_create(&mut old_socket, "active on generation A").await;
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("generation A response should start upstream");

    credential_control.set_value(
        crate::credentials::SecretValue::new(b"generation-b".to_vec())
            .expect("valid rotated credential"),
    );
    retained
        .config
        .schedule_credential_refresh(old_target.credential());
    wait_for_credential_generation_after(&retained, initial_generation_revision).await;
    assert_eq!(credential_control.read_count(), 2);

    release_first.notify_one();
    assert_eq!(
        next_test_websocket_json(&mut old_socket).await["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        next_test_websocket_json(&mut old_socket).await["type"].as_str(),
        Some("response.completed")
    );

    send_test_response_create(&mut old_socket, "must reconnect for generation B").await;
    let rejection = next_test_websocket_json(&mut old_socket).await;
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_reconnect_required")
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 1);

    let mut new_socket = connect_test_websocket(proxy_addr, "ws-generation-b").await;
    send_test_response_create(&mut new_socket, "new connection uses generation B").await;
    assert_eq!(
        next_test_websocket_json(&mut new_socket).await["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        next_test_websocket_json(&mut new_socket).await["type"].as_str(),
        Some("response.completed")
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 2);
    assert_eq!(
        *handshake_authorizations
            .lock()
            .expect("authorization capture lock"),
        [
            Some("Bearer generation-a".to_string()),
            Some("Bearer generation-b".to_string()),
        ]
    );

    new_socket.close(None).await.expect("close new WebSocket");
    shutdown_tx.send(true).expect("signal refresh shutdown");
    refresh_driver.await.expect("join refresh driver");
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_connection_budget_limits_idle_upstream_sockets() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let handshake_hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = handshake_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(|mut socket| async move { while socket.recv().await.is_some() {} })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (_, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
    let mut first = connect_test_websocket(proxy_addr, "ws-idle-budget-first").await;
    let mut second = connect_test_websocket(proxy_addr, "ws-idle-budget-second").await;

    let third = tokio_tungstenite::connect_async(test_websocket_request(
        proxy_addr,
        "ws-idle-budget-third",
    ))
    .await
    .expect_err("third idle connection must be rejected by the physical socket budget");
    let tokio_tungstenite::tungstenite::Error::Http(response) = third else {
        panic!("expected HTTP capacity response, got {third:?}");
    };
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(handshake_hits.load(Ordering::SeqCst), 2);

    first.close(None).await.expect("close first WebSocket");
    let mut replacement = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match tokio_tungstenite::connect_async(test_websocket_request(
                proxy_addr,
                "ws-idle-budget-replacement",
            ))
            .await
            {
                Ok((socket, _)) => break socket,
                Err(tokio_tungstenite::tungstenite::Error::Http(response))
                    if response.status() == StatusCode::TOO_MANY_REQUESTS =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => panic!("unexpected third WebSocket handshake failure: {error}"),
            }
        }
    })
    .await
    .expect("released physical connection permit");
    assert_eq!(handshake_hits.load(Ordering::SeqCst), 3);

    second.close(None).await.expect("close second WebSocket");
    replacement
        .close(None)
        .await
        .expect("close replacement WebSocket");
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_compatible_sol_rejects_ultra_before_upstream_write() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let handshake_hits = Arc::new(AtomicUsize::new(0));
    let create_hits = Arc::new(AtomicUsize::new(0));
    let handshakes_for_route = handshake_hits.clone();
    let creates_for_route = create_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let handshakes = handshakes_for_route.clone();
            let creates = creates_for_route.clone();
            async move {
                handshakes.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        let body = match message {
                            axum::extract::ws::Message::Text(text) => {
                                serde_json::from_str::<serde_json::Value>(text.as_str()).ok()
                            }
                            axum::extract::ws::Message::Binary(bytes) => {
                                serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                            }
                            _ => None,
                        };
                        if body
                            .as_ref()
                            .and_then(|value| value.get("type"))
                            .and_then(serde_json::Value::as_str)
                            == Some("response.create")
                        {
                            creates.fetch_add(1, Ordering::SeqCst);
                            let completed = serde_json::json!({
                                "type": "response.completed",
                                "response": { "id": "unexpected-compatible-sol" }
                            });
                            let _ = socket
                                .send(axum::extract::ws::Message::Text(
                                    completed.to_string().into(),
                                ))
                                .await;
                        }
                    }
                })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (_, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
    let mut socket = connect_test_websocket(proxy_addr, "ws-compatible-sol-ultra").await;
    assert_eq!(handshake_hits.load(Ordering::SeqCst), 1);

    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::json!({
                "type": "response.create",
                "model": "gpt-5.6-sol",
                "reasoning": {
                    "effort": "ultra",
                    "mode": "pro"
                },
                "input": "hi"
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("send compatible Sol response.create");
    let rejection = tokio::time::timeout(
        Duration::from_secs(2),
        next_test_websocket_json(&mut socket),
    )
    .await
    .expect("compatible Sol rejection timeout");
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_request_rejected")
    );
    assert_eq!(
        rejection["message"].as_str(),
        Some("reasoning intent requires a captured provider request contract")
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_withholds_completed_when_terminal_commit_fails() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
            ws.on_upgrade(|mut socket| async move {
                if socket.recv().await.is_some() {
                    send_successful_websocket_response(&mut socket, "resp-commit-failure").await;
                }
            })
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (proxy, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
    let state = proxy.state.clone();
    let runtime_store = state.runtime_store_handle();
    runtime_store.fail_next_logical_terminal_commit_for_test();

    let mut socket = connect_test_websocket(proxy_addr, "ws-terminal-commit-failure").await;
    send_test_response_create(&mut socket, "fail terminal commit").await;

    let created = next_test_websocket_json(&mut socket).await;
    assert_eq!(created["type"].as_str(), Some("response.created"));
    let failure = next_test_websocket_json(&mut socket).await;
    assert_eq!(failure["type"].as_str(), Some("error"));
    assert_eq!(
        failure["code"].as_str(),
        Some("websocket_terminal_commit_failed")
    );
    assert_ne!(failure["type"].as_str(), Some("response.completed"));

    let logical_requests = runtime_store
        .read_recent_logical_requests(10)
        .expect("read durable WebSocket request");
    assert_eq!(logical_requests.len(), 1);
    assert!(logical_requests[0].terminal.is_none());
    let attempts = runtime_store
        .read_attempts_for_logical_request(
            runtime_store.logical_request_handle(logical_requests[0].request.id),
        )
        .expect("read durable WebSocket attempt");
    assert!(
        attempts
            .first()
            .is_some_and(|attempt| attempt.terminal.is_some())
    );
    assert!(state.list_recent_finished(10).await.is_empty());

    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_logical_failures_are_durable_but_health_neutral() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    for event_type in ["response.incomplete", "response.cancelled"] {
        let terminal_type = event_type.to_string();
        let upstream = axum::Router::new().route(
            "/v1/responses",
            get(move |ws: axum::extract::ws::WebSocketUpgrade| {
                let terminal_type = terminal_type.clone();
                async move {
                    ws.on_upgrade(move |mut socket| async move {
                        if socket.recv().await.is_some() {
                            send_logical_failure_websocket_response(
                                &mut socket,
                                "resp-logical-failure",
                                terminal_type.as_str(),
                            )
                            .await;
                        }
                    })
                }
            }),
        );
        let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
        let (proxy, proxy_addr, proxy_handle) =
            spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
        let state = proxy.state.clone();
        let runtime_store = state.runtime_store_handle();
        let mut socket = connect_test_websocket(proxy_addr, event_type).await;

        send_test_response_create(&mut socket, "logical failure").await;
        assert_eq!(
            next_test_websocket_json(&mut socket).await["type"].as_str(),
            Some("response.created")
        );
        assert_eq!(
            next_test_websocket_json(&mut socket).await["type"].as_str(),
            Some(event_type)
        );

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
        assert_eq!(attempts.len(), 1);
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
        let route_attempt = finished[0]
            .retry
            .as_ref()
            .and_then(|retry| retry.route_attempts.first())
            .expect("logical failure route attempt");
        assert_eq!(route_attempt.decision, "failed_status");
        assert_eq!(route_attempt.cooldown_secs, None);

        let runtime = state
            .route_plan_runtime_state_for_provider_endpoints("codex")
            .await;
        let endpoint =
            runtime.provider_endpoint(&ProviderEndpointKey::new("codex", "single", "default"));
        assert_eq!(endpoint.failure_count, 0, "{event_type}");
        assert!(!endpoint.cooldown_active, "{event_type}");

        socket.close(None).await.expect("close WebSocket");
        proxy_handle.abort();
        upstream_handle.abort();
    }

    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_incomplete_half_open_probe_is_neutral_and_once_per_epoch() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
            ws.on_upgrade(|mut socket| async move {
                if socket.recv().await.is_some() {
                    send_logical_failure_websocket_response(
                        &mut socket,
                        "resp-half-open-incomplete",
                        "response.incomplete",
                    )
                    .await;
                }
            })
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (proxy, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
    let state = Arc::clone(&proxy.state);
    let endpoint = ProviderEndpointKey::new("codex", "single", "default");
    let identity = proxy
        .runtime_identity_for_provider_endpoint_for_test(&endpoint)
        .await;
    state
        .penalize_runtime_upstream_attempt_for_domain(
            "codex",
            &identity,
            crate::endpoint_health::RuntimeHealthDomain::Capability(
                crate::endpoint_health::RouteCapability::ResponsesWebSocket,
            ),
            30,
            crate::endpoint_health::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;

    let mut socket = connect_test_websocket(proxy_addr, "ws-half-open-incomplete").await;
    send_test_response_create(&mut socket, "incomplete half-open").await;
    assert_eq!(
        next_test_websocket_json(&mut socket).await["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        next_test_websocket_json(&mut socket).await["type"].as_str(),
        Some("response.incomplete")
    );
    socket
        .close(None)
        .await
        .expect("close incomplete probe socket");

    let next = test_websocket_request(proxy_addr, "ws-half-open-incomplete-next");
    let error = tokio_tungstenite::connect_async(next)
        .await
        .expect_err("a neutral half-open terminal must not repeat in the same breaker epoch");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP half-open rejection, got: {error}");
    };
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(state.list_recent_finished(10).await.len(), 1);

    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_failure_does_not_cool_http_inference() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let http_hits = Arc::new(AtomicUsize::new(0));
    let http_hits_for_route = http_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
            ws.on_upgrade(|mut socket| async move {
                if socket.recv().await.is_some() {
                    send_failed_websocket_response(&mut socket, "resp-ws-failed").await;
                }
            })
        })
        .post(move || {
            let hits = http_hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({
                    "id": "resp-http-healthy",
                    "object": "response",
                    "output": []
                }))
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (_proxy, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);

    let mut socket = connect_test_websocket(proxy_addr, "ws-health-isolation").await;
    send_test_response_create(&mut socket, "fail websocket capability").await;
    assert_eq!(
        next_test_websocket_json(&mut socket).await["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        next_test_websocket_json(&mut socket).await["type"].as_str(),
        Some("response.failed")
    );
    socket.close(None).await.expect("close WebSocket");

    let response = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","input":"still healthy"}"#)
        .send()
        .await
        .expect("send HTTP inference after WebSocket failure");
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response
        .bytes()
        .await
        .expect("read HTTP inference response");
    assert_eq!(http_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

async fn run_websocket_failed_terminal_commit_failure(fail_attempt_commit: bool) {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
            ws.on_upgrade(|mut socket| async move {
                if socket.recv().await.is_some() {
                    send_failed_websocket_response(&mut socket, "resp-failed-commit").await;
                }
            })
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (proxy, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
    let state = proxy.state.clone();
    let runtime_store = state.runtime_store_handle();
    if fail_attempt_commit {
        runtime_store.fail_next_attempt_terminal_commit_for_test();
    } else {
        runtime_store.fail_next_logical_terminal_commit_for_test();
    }

    let session_id = if fail_attempt_commit {
        "ws-failed-attempt-commit"
    } else {
        "ws-failed-logical-commit"
    };
    let mut socket = connect_test_websocket(proxy_addr, session_id).await;
    send_test_response_create(&mut socket, "fail failed-terminal commit").await;

    let created = next_test_websocket_json(&mut socket).await;
    assert_eq!(created["type"].as_str(), Some("response.created"));
    let failure = next_test_websocket_json(&mut socket).await;
    assert_eq!(failure["type"].as_str(), Some("error"));
    assert_eq!(
        failure["code"].as_str(),
        Some("websocket_terminal_commit_failed")
    );
    assert_ne!(failure["type"].as_str(), Some("response.failed"));

    let logical_requests = runtime_store
        .read_recent_logical_requests(10)
        .expect("read durable failed WebSocket request");
    assert_eq!(logical_requests.len(), 1);
    assert!(logical_requests[0].terminal.is_none());
    let attempts = runtime_store
        .read_attempts_for_logical_request(
            runtime_store.logical_request_handle(logical_requests[0].request.id),
        )
        .expect("read durable failed WebSocket attempt");
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0]
            .terminal
            .as_ref()
            .map(|terminal| terminal.terminal.outcome),
        (!fail_attempt_commit).then_some(crate::runtime_store::AttemptOutcome::Failed)
    );
    assert!(state.list_recent_finished(10).await.is_empty());
    assert!(
        state
            .peek_session_route_affinity(session_id)
            .await
            .is_none()
    );

    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_withholds_failed_when_attempt_terminal_commit_fails() {
    run_websocket_failed_terminal_commit_failure(true).await;
}

#[tokio::test]
async fn responses_websocket_withholds_failed_when_logical_terminal_commit_fails() {
    run_websocket_failed_terminal_commit_failure(false).await;
}

#[tokio::test]
async fn responses_websocket_relays_headers_model_mapping_and_frames() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );

    let seen_headers = Arc::new(std::sync::Mutex::new(None::<HeaderMap>));
    let seen_first_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let headers_sink = seen_headers.clone();
    let body_sink = seen_first_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(
            move |ws: axum::extract::ws::WebSocketUpgrade, headers: HeaderMap| {
                let headers_sink = headers_sink.clone();
                let body_sink = body_sink.clone();
                async move {
                    ws.on_upgrade(move |mut socket| async move {
                        *headers_sink.lock().expect("headers lock") = Some(headers);
                        if let Some(Ok(message)) = socket.recv().await {
                            let body = match message {
                                axum::extract::ws::Message::Text(text) => {
                                    serde_json::from_str::<serde_json::Value>(text.as_str()).ok()
                                }
                                axum::extract::ws::Message::Binary(bytes) => {
                                    serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                                }
                                _ => None,
                            };
                            *body_sink.lock().expect("body lock") = body;
                            let _ = socket
                                .send(axum::extract::ws::Message::Text(
                                    r#"{"type":"response.created","response":{"id":"resp-1"}}"#
                                        .into(),
                                ))
                                .await;
                        }
                    })
                }
            },
        ),
    );
    let (u_addr, u_handle) = spawn_axum_server(upstream);

    let cfg = make_helper_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", u_addr),
            auth: UpstreamAuth {
                auth_token: Some("server-token".to_string().into()),
                auth_token_env: None,
                auth_token_ref: None,
                api_key: None,
                api_key_env: None,
                api_key_ref: None,
                allow_anonymous: None,
            },
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::from([("gpt-5".to_string(), "relay-gpt-5".to_string())]),
        }],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = ProxyService::new(Client::new(), Arc::new(cfg), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let mut request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("ws request");
    request
        .headers_mut()
        .insert("session-id", HeaderValue::from_static("ws-cache"));
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect proxy websocket");
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","stream":true,"prompt_cache_key":"ws-cache"}"#.into(),
        ))
        .await
        .expect("send first frame");

    let event = socket
        .next()
        .await
        .expect("event")
        .expect("event ok")
        .to_text()
        .expect("event text")
        .to_string();
    assert!(event.contains("response.created"), "{event}");

    let headers = seen_headers
        .lock()
        .expect("headers lock")
        .clone()
        .expect("upstream headers");
    assert_eq!(
        headers.get("authorization"),
        Some(&HeaderValue::from_static("Bearer server-token"))
    );
    assert_eq!(
        headers.get("openai-beta"),
        Some(&HeaderValue::from_static("responses_websockets=2026-02-06"))
    );
    assert_eq!(
        headers.get("session-id"),
        Some(&HeaderValue::from_static("ws-cache"))
    );
    assert_eq!(
        headers.get("thread-id"),
        Some(&HeaderValue::from_static("ws-cache"))
    );

    let body = seen_first_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("upstream first body");
    assert_eq!(body["type"].as_str(), Some("response.create"));
    assert_eq!(body["model"].as_str(), Some("relay-gpt-5"));
    assert_eq!(body["prompt_cache_key"].as_str(), Some("ws-cache"));

    proxy_handle.abort();
    u_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_half_open_probe_is_singleflight_and_completed_create_recovers_route() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );

    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
            ws.on_upgrade(|mut socket| async move {
                if socket.recv().await.is_some() {
                    send_successful_websocket_response(&mut socket, "resp-half-open").await;
                }
            })
        }),
    );
    let (u_addr, u_handle) = spawn_axum_server(upstream);

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
                "ws-provider".to_string(),
                ProviderConfig {
                    base_url: Some(format!("http://{u_addr}/v1")),
                    inline_auth: UpstreamAuth::default(),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "ws-provider".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let provider_endpoint =
        crate::runtime_identity::ProviderEndpointKey::new("codex", "ws-provider", "default");
    let provider_identity = proxy
        .runtime_identity_for_provider_endpoint_for_test(&provider_endpoint)
        .await;
    state
        .penalize_runtime_upstream_attempt_for_domain(
            "codex",
            &provider_identity,
            crate::endpoint_health::RuntimeHealthDomain::Capability(
                crate::endpoint_health::RouteCapability::ResponsesWebSocket,
            ),
            30,
            crate::endpoint_health::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("ws request");
    let (mut first, first_response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("the unique cooled route should admit one half-open WebSocket probe");
    assert_eq!(first_response.status(), StatusCode::SWITCHING_PROTOCOLS);

    let second_request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("second ws request");
    let error = tokio_tungstenite::connect_async(second_request)
        .await
        .expect_err("a dispatched half-open probe must prevent a concurrent second probe");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP half-open rejection, got: {error}");
    };
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    send_test_response_create(&mut first, "recover cooled WebSocket route").await;
    assert_eq!(
        next_test_websocket_json(&mut first).await["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        next_test_websocket_json(&mut first).await["type"].as_str(),
        Some("response.completed")
    );
    let runtime = state
        .route_plan_runtime_state_for_provider_endpoints("codex")
        .await;
    let endpoint = runtime.provider_endpoint(&provider_endpoint);
    assert_eq!(endpoint.failure_count, 0);
    assert!(!endpoint.cooldown_active);

    first.close(None).await.expect("close recovered WebSocket");
    let replacement_request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("replacement ws request");
    let (_, replacement_response) = tokio_tungstenite::connect_async(replacement_request)
        .await
        .expect("a completed half-open probe should restore normal route selection");
    assert_eq!(
        replacement_response.status(),
        StatusCode::SWITCHING_PROTOCOLS
    );

    proxy_handle.abort();
    u_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_client_close_during_active_half_open_create_is_neutral_and_durable() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let create_started = Arc::new(tokio::sync::Notify::new());
    let create_started_for_route = Arc::clone(&create_started);
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let create_started = Arc::clone(&create_started_for_route);
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    if socket.recv().await.is_some() {
                        create_started.notify_one();
                        while socket.recv().await.is_some() {}
                    }
                })
            }
        })
        .post(|| async {
            Json(serde_json::json!({
                "id": "http-stays-healthy-after-client-close",
                "object": "response",
                "output": []
            }))
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (proxy, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
    let state = Arc::clone(&proxy.state);
    let endpoint = ProviderEndpointKey::new("codex", "single", "default");
    let identity = proxy
        .runtime_identity_for_provider_endpoint_for_test(&endpoint)
        .await;
    state
        .penalize_runtime_upstream_attempt_for_domain(
            "codex",
            &identity,
            crate::endpoint_health::RuntimeHealthDomain::Capability(
                crate::endpoint_health::RouteCapability::ResponsesWebSocket,
            ),
            30,
            crate::endpoint_health::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;

    let mut socket = connect_test_websocket(proxy_addr, "ws-client-close-half-open").await;
    send_test_response_create(&mut socket, "close while active").await;
    tokio::time::timeout(Duration::from_secs(2), create_started.notified())
        .await
        .expect("the half-open response.create should reach upstream");
    socket
        .close(None)
        .await
        .expect("client closes active websocket");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !state.list_recent_finished(10).await.is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("client-close request should receive a durable terminal");
    let finished = state.list_recent_finished(10).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].status_code, StatusCode::BAD_GATEWAY.as_u16());

    let http_response = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","input":"client closure is not endpoint transport"}"#)
        .send()
        .await
        .expect("send inference after client-side WebSocket close");
    assert_eq!(
        http_response.status(),
        StatusCode::OK,
        "client closure must not create shared endpoint transport cooldown"
    );

    let second_request = test_websocket_request(proxy_addr, "ws-client-close-half-open-second");
    let error = tokio_tungstenite::connect_async(second_request)
        .await
        .expect_err("neutral completion must not repeat the same half-open epoch");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP half-open rejection, got: {error}");
    };
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_three_upstream_disconnects_cool_only_websocket_capability() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let disconnects = Arc::new(AtomicUsize::new(0));
    let disconnects_for_route = Arc::clone(&disconnects);
    let release_disconnects = Arc::new(tokio::sync::Barrier::new(4));
    let release_disconnects_for_route = Arc::clone(&release_disconnects);
    let http_hits = Arc::new(AtomicUsize::new(0));
    let http_hits_for_route = Arc::clone(&http_hits);
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let disconnects = Arc::clone(&disconnects_for_route);
            let release_disconnects = Arc::clone(&release_disconnects_for_route);
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if matches!(
                            message,
                            axum::extract::ws::Message::Text(_)
                                | axum::extract::ws::Message::Binary(_)
                        ) {
                            disconnects.fetch_add(1, Ordering::SeqCst);
                            release_disconnects.wait().await;
                            let _ = socket.close().await;
                            break;
                        }
                    }
                })
            }
        })
        .post(move || {
            let http_hits = Arc::clone(&http_hits_for_route);
            async move {
                http_hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({
                    "id": "resp-http-after-ws-disconnects",
                    "object": "response",
                    "output": []
                }))
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (proxy, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 3);
    let state = Arc::clone(&proxy.state);

    let mut sockets = Vec::new();
    for index in 0..3 {
        sockets.push(
            connect_test_websocket(
                proxy_addr,
                format!("ws-upstream-disconnect-{index}").as_str(),
            )
            .await,
        );
    }
    for socket in &mut sockets {
        send_test_response_create(socket, "disconnect upstream").await;
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while disconnects.load(Ordering::SeqCst) < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all three response.create frames should reach upstream before disconnects");
    release_disconnects.wait().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while state.list_recent_finished(10).await.len() < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("all upstream disconnects should receive durable terminals");
    for socket in &mut sockets {
        let _ = socket.close(None).await;
    }
    let finished = state.list_recent_finished(10).await;
    assert_eq!(finished.len(), 3);
    assert!(
        finished
            .iter()
            .all(|request| request.status_code == StatusCode::BAD_GATEWAY.as_u16())
    );

    let runtime = state
        .route_plan_runtime_state_for_provider_endpoints("codex")
        .await;
    let inference_health =
        runtime.provider_endpoint(&ProviderEndpointKey::new("codex", "single", "default"));
    assert_eq!(inference_health.failure_count, 0);
    assert!(!inference_health.cooldown_active);

    let http_response = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","input":"WebSocket transport is capability-local"}"#)
        .send()
        .await
        .expect("send inference after upstream WebSocket disconnect");
    assert_eq!(
        http_response.status(),
        StatusCode::OK,
        "post-upgrade WebSocket failures must not cool HTTP inference"
    );
    assert_eq!(
        http_hits.load(Ordering::SeqCst),
        1,
        "HTTP inference should still reach the healthy upstream"
    );

    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_allows_fallback_sticky_compaction_without_route_affinity() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
        scoped.set_path(
            "CODEX_HELPER_CONTROL_TRACE_PATH",
            temp_dir.join("logs").join("control_trace.jsonl").as_path(),
        );
        scoped.set("CODEX_HELPER_CONTROL_TRACE", "1");
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );

    let b_hits = Arc::new(AtomicUsize::new(0));
    let b_hits_for_route = b_hits.clone();
    let upstream_b = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let b_hits = b_hits_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    b_hits.fetch_add(1, Ordering::SeqCst);
                    if socket.recv().await.is_some() {
                        send_successful_websocket_response(&mut socket, "resp-b").await;
                    }
                })
            }
        }),
    );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_hits = Arc::new(AtomicUsize::new(0));
    let c_hits_for_route = c_hits.clone();
    let upstream_c = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let c_hits = c_hits_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    c_hits.fetch_add(1, Ordering::SeqCst);
                    if socket.recv().await.is_some() {
                        send_successful_websocket_response(&mut socket, "resp-c").await;
                    }
                })
            }
        }),
    );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

    let retry = RetryConfig {
        upstream: Some(retry_layer_config(
            1,
            "502",
            Vec::new(),
            RetryStrategy::Failover,
        )),
        provider: Some(retry_layer_config(
            2,
            "502",
            Vec::new(),
            RetryStrategy::Failover,
        )),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let mut routing = RouteGraphConfig::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RouteAffinityPolicy::FallbackSticky;
    let source = HelperConfig {
        retry,
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let mut request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("ws request");
    request.headers_mut().insert(
        WS_PROVIDER_ENDPOINT_HEADER,
        HeaderValue::from_static("codex/b/default"),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect proxy websocket");
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","stream":true,"prompt_cache_key":"ws-missing-v2-affinity","input":[{"role":"user","content":"compact me"},{"type":"compaction_trigger"}]}"#.into(),
        ))
        .await
        .expect("send first frame");

    let event = socket
        .next()
        .await
        .expect("event")
        .expect("event ok")
        .to_text()
        .expect("event text")
        .to_string();
    assert!(event.contains("response.created"), "{event}");
    assert_eq!(b_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c_hits.load(Ordering::SeqCst), 0);
    let terminal = socket
        .next()
        .await
        .expect("terminal event")
        .expect("terminal event ok")
        .to_text()
        .expect("terminal event text")
        .to_string();
    assert!(terminal.contains("response.completed"), "{terminal}");

    socket.close(None).await.expect("close websocket");
    let finished = find_finished_request(&state, 10, |request| {
        request.session_id.as_deref() == Some("ws-missing-v2-affinity")
    })
    .await
    .expect("finished websocket request");
    assert_eq!(
        finished.status_code,
        StatusCode::SWITCHING_PROTOCOLS.as_u16()
    );

    let affinity = state
        .get_session_route_affinity("ws-missing-v2-affinity")
        .await
        .expect("route affinity recorded after first websocket compact");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn responses_websocket_requires_reconnect_when_same_domain_affinity_no_longer_matches_bound_endpoint()
 {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );

    let upstream_b =
        axum::Router::new().route(
            "/v1/responses",
            get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                ws.on_upgrade(|_| async move {})
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_hits = Arc::new(AtomicUsize::new(0));
    let c_hits_for_route = c_hits.clone();
    let upstream_c = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let c_hits = c_hits_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while socket.recv().await.is_some() {
                        let hit = c_hits.fetch_add(1, Ordering::SeqCst) + 1;
                        send_successful_websocket_response(
                            &mut socket,
                            format!("resp-c-{hit}").as_str(),
                        )
                        .await;
                    }
                })
            }
        }),
    );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

    let d_hits = Arc::new(AtomicUsize::new(0));
    let d_hits_for_route = d_hits.clone();
    let upstream_d = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let d_hits = d_hits_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    d_hits.fetch_add(1, Ordering::SeqCst);
                    if socket.recv().await.is_some() {
                        send_successful_websocket_response(&mut socket, "resp-d").await;
                    }
                })
            }
        }),
    );
    let (d_addr, d_handle) = spawn_axum_server(upstream_d);

    let retry = RetryConfig {
        upstream: Some(retry_layer_config(
            1,
            "502",
            Vec::new(),
            RetryStrategy::Failover,
        )),
        provider: Some(retry_layer_config(
            3,
            "502",
            Vec::new(),
            RetryStrategy::Failover,
        )),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let mut routing =
        RouteGraphConfig::ordered_failover(vec!["b".to_string(), "c".to_string(), "d".to_string()]);
    routing.affinity_policy = crate::config::RouteAffinityPolicy::Hard;
    let source = HelperConfig {
        retry,
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        continuity_domain: Some("relay-cluster-a".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        continuity_domain: Some("relay-cluster-a".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "d".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{d_addr}/v1")),
                        continuity_domain: Some("relay-cluster-b".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let route_graph_key = crate::routing_ir::compile_route_plan_template("codex", &source.codex)
        .expect("route template")
        .route_graph_key();
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let b_endpoint = ProviderEndpointKey::new("codex", "b", "default");
    let b_identity = proxy
        .runtime_identity_for_provider_endpoint_for_test(&b_endpoint)
        .await;
    state
        .record_session_route_affinity_success(
            None,
            "ws-explicit-domain",
            SessionRouteAffinityTarget {
                route_graph_key: route_graph_key.clone(),
                session_identity_source: Some(SessionIdentitySource::PromptCacheKey),
                provider_endpoint: ProviderEndpointKey::new("codex", "b", "default"),
                upstream_base_url: format!("http://{b_addr}/v1"),
                route_path: vec!["b".to_string()],
            },
            Some("test_seed".to_string()),
            crate::logging::now_ms(),
        )
        .await
        .expect("persist route affinity");
    state
        .penalize_runtime_upstream_attempt_for_domain(
            "codex",
            &b_identity,
            crate::endpoint_health::RuntimeHealthDomain::Capability(
                crate::endpoint_health::RouteCapability::ResponsesWebSocket,
            ),
            30,
            crate::endpoint_health::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;

    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let mut request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("ws request");
    request
        .headers_mut()
        .insert("session-id", HeaderValue::from_static("ws-explicit-domain"));
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect proxy websocket");
    state
        .record_runtime_upstream_attempt_success_for_capability(
            "codex",
            &b_identity,
            crate::endpoint_health::RouteCapability::ResponsesWebSocket,
            crate::logging::now_ms(),
        )
        .await;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","stream":true,"prompt_cache_key":"ws-explicit-domain","input":[{"role":"user","content":"compact me"},{"type":"compaction_trigger"}]}"#.into(),
        ))
        .await
        .expect("send first frame");

    let rejection = next_test_websocket_json(&mut socket).await;
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_reconnect_required")
    );
    assert_eq!(c_hits.load(Ordering::SeqCst), 0);
    assert_eq!(d_hits.load(Ordering::SeqCst), 0);

    let affinity = state
        .get_session_route_affinity("ws-explicit-domain")
        .await
        .expect("route affinity retained");
    assert_eq!(affinity.route_graph_key, route_graph_key);
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
    d_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_rebind_requires_reconnect_without_writing_old_upstream() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let b_frames = Arc::new(AtomicUsize::new(0));
    let b_frame_received = Arc::new(tokio::sync::Notify::new());
    let b_frames_for_route = Arc::clone(&b_frames);
    let b_received_for_route = Arc::clone(&b_frame_received);
    let upstream_b = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let frames = Arc::clone(&b_frames_for_route);
            let received = Arc::clone(&b_received_for_route);
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if !matches!(
                            message,
                            axum::extract::ws::Message::Text(_)
                                | axum::extract::ws::Message::Binary(_)
                        ) {
                            continue;
                        }
                        frames.fetch_add(1, Ordering::SeqCst);
                        received.notify_one();
                        send_successful_websocket_response(&mut socket, "resp-b-unexpected").await;
                    }
                })
            }
        }),
    );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_frames = Arc::new(AtomicUsize::new(0));
    let c_frame_received = Arc::new(tokio::sync::Notify::new());
    let c_frames_for_route = Arc::clone(&c_frames);
    let c_received_for_route = Arc::clone(&c_frame_received);
    let upstream_c = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let frames = Arc::clone(&c_frames_for_route);
            let received = Arc::clone(&c_received_for_route);
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if !matches!(
                            message,
                            axum::extract::ws::Message::Text(_)
                                | axum::extract::ws::Message::Binary(_)
                        ) {
                            continue;
                        }
                        frames.fetch_add(1, Ordering::SeqCst);
                        received.notify_one();
                        send_successful_websocket_response(&mut socket, "resp-c-unexpected").await;
                    }
                })
            }
        }),
    );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

    let mut routing = RouteGraphConfig::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RouteAffinityPolicy::Hard;
    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        continuity_domain: Some("relay-cluster-a".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        continuity_domain: Some("relay-cluster-a".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let template = crate::routing_ir::compile_route_plan_template("codex", &source.codex)
        .expect("route template");
    let route_graph_key = template.route_graph_key();
    let affinity_target = |provider_id: &str| {
        let candidate = template
            .candidates
            .iter()
            .find(|candidate| candidate.provider_id == provider_id)
            .expect("route candidate");
        SessionRouteAffinityTarget {
            route_graph_key: route_graph_key.clone(),
            session_identity_source: Some(SessionIdentitySource::Header),
            provider_endpoint: template.candidate_provider_endpoint_key(candidate),
            upstream_base_url: candidate.base_url.clone(),
            route_path: candidate.route_path.clone(),
        }
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = Arc::clone(&proxy.state);
    let initial = tokio::time::timeout(
        Duration::from_secs(1),
        state.record_session_route_affinity_success(
            None,
            "ws-operator-rebind",
            affinity_target("b"),
            Some("test_seed".to_string()),
            crate::logging::now_ms(),
        ),
    )
    .await
    .expect("initial affinity should not deadlock")
    .expect("persist initial affinity");
    let initial_revision = session_route_affinity_revision(&initial);

    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let mut request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("ws request");
    request
        .headers_mut()
        .insert("session-id", HeaderValue::from_static("ws-operator-rebind"));
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(2),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("WebSocket handshake should not deadlock")
    .expect("connect proxy websocket");

    let rebound = tokio::time::timeout(
        Duration::from_secs(1),
        state.compare_and_mutate_session_route_affinity(
            "ws-operator-rebind",
            Some(initial_revision.as_str()),
            SessionRouteAffinityControlCommand::Rebind(affinity_target("c")),
            crate::logging::now_ms(),
        ),
    )
    .await
    .expect("rebind should not deadlock")
    .expect("rebind session affinity");
    assert_eq!(
        rebound.status,
        crate::state::SessionRouteAffinityControlStatus::Applied
    );

    tokio::time::timeout(
        Duration::from_secs(1),
        socket.send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","stream":true,"prompt_cache_key":"ws-operator-rebind","input":"after rebind"}"#.into(),
        )),
    )
    .await
    .expect("response.create send should not deadlock")
    .expect("send response.create after rebind");
    let rejection = tokio::time::timeout(
        Duration::from_secs(2),
        next_test_websocket_json(&mut socket),
    )
    .await
    .expect("rebind mismatch must reject promptly");
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_reconnect_required")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), b_frame_received.notified())
            .await
            .is_err(),
        "the old upstream socket must receive zero response.create frames"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), c_frame_received.notified())
            .await
            .is_err(),
        "the rebound endpoint must not receive a frame before reconnect"
    );
    assert_eq!(b_frames.load(Ordering::SeqCst), 0);
    assert_eq!(c_frames.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_clear_reselects_auto_before_reestablishing_affinity() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let upstream_b = spawn_counting_websocket_upstream("resp-b-unexpected-after-clear");
    let upstream_c = spawn_counting_websocket_upstream("resp-c-unexpected-before-reconnect");
    let mut routing = RouteGraphConfig::ordered_failover(vec!["c".to_string(), "b".to_string()]);
    routing.affinity_policy = crate::config::RouteAffinityPolicy::Hard;
    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfig {
                        base_url: Some(upstream_b.server.base_url()),
                        continuity_domain: Some("relay-b".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfig {
                        base_url: Some(upstream_c.server.base_url()),
                        continuity_domain: Some("relay-c".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let template = crate::routing_ir::compile_route_plan_template("codex", &source.codex)
        .expect("route template");
    let route_graph_key = template.route_graph_key();
    let candidate_b = template
        .candidates
        .iter()
        .find(|candidate| candidate.provider_id == "b")
        .expect("provider b route candidate");
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = Arc::clone(&proxy.state);
    let initial = state
        .record_session_route_affinity_success(
            None,
            "ws-operator-clear",
            SessionRouteAffinityTarget {
                route_graph_key: route_graph_key.clone(),
                session_identity_source: Some(SessionIdentitySource::Header),
                provider_endpoint: template.candidate_provider_endpoint_key(candidate_b),
                upstream_base_url: candidate_b.base_url.clone(),
                route_path: candidate_b.route_path.clone(),
            },
            Some("test_seed".to_string()),
            crate::logging::now_ms(),
        )
        .await
        .expect("persist initial affinity");
    let initial_revision = session_route_affinity_revision(&initial);

    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let mut socket = connect_test_websocket(proxy_addr, "ws-operator-clear").await;
    let cleared = state
        .compare_and_mutate_session_route_affinity(
            "ws-operator-clear",
            Some(initial_revision.as_str()),
            SessionRouteAffinityControlCommand::Clear,
            crate::logging::now_ms(),
        )
        .await
        .expect("clear session affinity while websocket is idle");
    assert_eq!(
        cleared.status,
        crate::state::SessionRouteAffinityControlStatus::Applied
    );
    assert!(
        state
            .get_session_route_affinity("ws-operator-clear")
            .await
            .is_none()
    );

    send_test_response_create(&mut socket, "after clear").await;
    let rejection = tokio::time::timeout(
        Duration::from_secs(2),
        next_test_websocket_json(&mut socket),
    )
    .await
    .expect("auto selection mismatch must reject promptly");
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_reconnect_required")
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            upstream_b.frame_received.notified(),
        )
        .await
        .is_err(),
        "the old upstream socket must receive zero response.create frames"
    );
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            upstream_c.frame_received.notified(),
        )
        .await
        .is_err(),
        "the auto-selected endpoint must not receive a frame before reconnect"
    );
    assert_eq!(upstream_b.frames.load(Ordering::SeqCst), 0);
    assert_eq!(upstream_c.frames.load(Ordering::SeqCst), 0);
    assert!(
        state
            .get_session_route_affinity("ws-operator-clear")
            .await
            .is_none(),
        "the stale websocket endpoint must not recreate affinity after clear"
    );

    proxy_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_explicit_round_robin_binding_does_not_advance_cursor_on_create() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let upstream_b = spawn_counting_websocket_upstream("resp-explicit-b");
    let upstream_c = spawn_counting_websocket_upstream("resp-explicit-c-unexpected");
    let mut routing = RouteGraphConfig::round_robin(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RouteAffinityPolicy::Off;
    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfig {
                        base_url: Some(upstream_b.server.base_url()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfig {
                        base_url: Some(upstream_c.server.base_url()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let mut request = test_websocket_request(proxy_addr, "ws-explicit-round-robin");
    request.headers_mut().insert(
        WS_PROVIDER_ENDPOINT_HEADER,
        HeaderValue::from_static("codex/b/default"),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("explicit round-robin websocket handshake");

    send_test_response_create(&mut socket, "explicit round-robin first create").await;
    assert_eq!(
        next_test_websocket_json(&mut socket).await["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        next_test_websocket_json(&mut socket).await["type"].as_str(),
        Some("response.completed")
    );
    assert_eq!(upstream_b.frames.load(Ordering::SeqCst), 1);
    assert_eq!(upstream_c.frames.load(Ordering::SeqCst), 0);

    socket.close(None).await.expect("close websocket");
    proxy_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_rejects_first_create_after_scheduling_reload() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let create_hits = Arc::new(AtomicUsize::new(0));
    let frame_received = Arc::new(tokio::sync::Notify::new());
    let hits_for_route = Arc::clone(&create_hits);
    let received_for_route = Arc::clone(&frame_received);
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = Arc::clone(&hits_for_route);
            let received = Arc::clone(&received_for_route);
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if !matches!(
                            message,
                            axum::extract::ws::Message::Text(_)
                                | axum::extract::ws::Message::Binary(_)
                        ) {
                            continue;
                        }
                        hits.fetch_add(1, Ordering::SeqCst);
                        received.notify_one();
                        send_successful_websocket_response(&mut socket, "unexpected-create").await;
                    }
                })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let initial = single_provider_websocket_config(upstream_addr, SchedulingPreset::Balanced, 1);
    let proxy = ProxyService::new(Client::new(), Arc::new(initial), "codex");
    let retained = proxy.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let mut socket = tokio::time::timeout(
        Duration::from_secs(2),
        connect_test_websocket(proxy_addr, "ws-scheduling-reload"),
    )
    .await
    .expect("WebSocket handshake should not deadlock");

    let reloaded =
        single_provider_websocket_config(upstream_addr, SchedulingPreset::ContinuityFirst, 1);
    let changed = tokio::time::timeout(
        Duration::from_secs(1),
        retained.config.reload_with_source(|| async {
            Ok((crate::config::LoadedConfig { source: reloaded }, None))
        }),
    )
    .await
    .expect("scheduling reload should not deadlock")
    .expect("reload scheduling preset");
    assert!(changed);

    tokio::time::timeout(
        Duration::from_secs(1),
        send_test_response_create(&mut socket, "first after reload"),
    )
    .await
    .expect("response.create send should not deadlock");
    let rejection = tokio::time::timeout(
        Duration::from_secs(2),
        next_test_websocket_json(&mut socket),
    )
    .await
    .expect("scheduling drift must reject the first create");
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_reconnect_required")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), frame_received.notified())
            .await
            .is_err(),
        "the stale upstream socket must receive zero response.create frames"
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_allows_compaction_trigger_without_prior_affinity_for_single_endpoint()
{
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );

    let hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    if socket.recv().await.is_some() {
                        send_successful_websocket_response(&mut socket, "resp-single").await;
                    }
                })
            }
        }),
    );
    let (u_addr, u_handle) = spawn_axum_server(upstream);

    let mut routing = RouteGraphConfig::ordered_failover(vec!["single".to_string()]);
    routing.affinity_policy = crate::config::RouteAffinityPolicy::FallbackSticky;
    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([(
                "single".to_string(),
                ProviderConfig {
                    base_url: Some(format!("http://{u_addr}/v1")),
                    inline_auth: UpstreamAuth::default(),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("ws request");
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect proxy websocket");
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","stream":true,"prompt_cache_key":"ws-single-v2-affinity","input":[{"role":"user","content":"compact me"},{"type":"compaction_trigger"}]}"#.into(),
        ))
        .await
        .expect("send first frame");

    let event = socket
        .next()
        .await
        .expect("event")
        .expect("event ok")
        .to_text()
        .expect("event text")
        .to_string();
    assert!(event.contains("response.created"), "{event}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let terminal = socket
        .next()
        .await
        .expect("terminal event")
        .expect("terminal event ok")
        .to_text()
        .expect("terminal event text")
        .to_string();
    assert!(terminal.contains("response.completed"), "{terminal}");

    socket.close(None).await.expect("close websocket");
    let finished = find_finished_request(&state, 10, |request| {
        request.session_id.as_deref() == Some("ws-single-v2-affinity")
    })
    .await
    .expect("finished websocket request");
    assert_eq!(
        finished.status_code,
        StatusCode::SWITCHING_PROTOCOLS.as_u16()
    );

    proxy_handle.abort();
    u_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_idle_handshakes_have_independent_connection_capacity() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let handshake_hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = handshake_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(|mut socket| async move { while socket.recv().await.is_some() {} })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let source = single_provider_websocket_config(upstream_addr, SchedulingPreset::Balanced, 1);
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let first_request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("first ws request");
    let (mut first_socket, _) = tokio_tungstenite::connect_async(first_request)
        .await
        .expect("first idle websocket handshake");

    let second_request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("second ws request");
    let (mut second_socket, _) = tokio_tungstenite::connect_async(second_request)
        .await
        .expect("second idle websocket handshake");

    let third_request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("third ws request");
    let third_error = tokio_tungstenite::connect_async(third_request)
        .await
        .expect_err("third idle websocket handshake must respect connection capacity");
    let tokio_tungstenite::tungstenite::Error::Http(response) = third_error else {
        panic!("expected HTTP capacity rejection, got {third_error:?}");
    };
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(handshake_hits.load(Ordering::SeqCst), 2);

    first_socket
        .close(None)
        .await
        .expect("close first websocket");

    let replacement_request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("replacement ws request");
    let (mut replacement_socket, _) = tokio_tungstenite::connect_async(replacement_request)
        .await
        .expect("connection capacity must be released after close");
    assert_eq!(handshake_hits.load(Ordering::SeqCst), 3);
    second_socket
        .close(None)
        .await
        .expect("close second websocket");
    replacement_socket
        .close(None)
        .await
        .expect("close replacement websocket");
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_balanced_waits_until_previous_create_completes() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let create_hits = Arc::new(AtomicUsize::new(0));
    let first_started = Arc::new(tokio::sync::Notify::new());
    let second_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let hits_for_route = create_hits.clone();
    let first_started_for_route = first_started.clone();
    let second_started_for_route = second_started.clone();
    let release_first_for_route = release_first.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            let first_started = first_started_for_route.clone();
            let second_started = second_started_for_route.clone();
            let release_first = release_first_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        let body = match message {
                            axum::extract::ws::Message::Text(text) => {
                                serde_json::from_str::<serde_json::Value>(text.as_str()).ok()
                            }
                            axum::extract::ws::Message::Binary(bytes) => {
                                serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                            }
                            _ => None,
                        };
                        if body
                            .as_ref()
                            .and_then(|value| value.get("type"))
                            .and_then(serde_json::Value::as_str)
                            != Some("response.create")
                        {
                            continue;
                        }

                        let hit = hits.fetch_add(1, Ordering::SeqCst);
                        if hit == 0 {
                            first_started.notify_one();
                            release_first.notified().await;
                        } else {
                            second_started.notify_one();
                        }
                        let event = serde_json::json!({
                            "type": "response.completed",
                            "response": {
                                "id": format!("resp-{}", hit + 1),
                                "usage": {
                                    "input_tokens": 1,
                                    "output_tokens": 1,
                                    "total_tokens": 2
                                }
                            }
                        });
                        let _ = socket
                            .send(axum::extract::ws::Message::Text(event.to_string().into()))
                            .await;
                    }
                })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let source = single_provider_websocket_config(upstream_addr, SchedulingPreset::Balanced, 1);
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let mut first_request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("first ws request");
    first_request
        .headers_mut()
        .insert("session-id", HeaderValue::from_static("ws-capacity-first"));
    let (mut first_socket, _) = tokio_tungstenite::connect_async(first_request)
        .await
        .expect("first websocket handshake");

    let mut second_request = format!("ws://{proxy_addr}/v1/responses")
        .into_client_request()
        .expect("second ws request");
    second_request
        .headers_mut()
        .insert("session-id", HeaderValue::from_static("ws-capacity-second"));
    let (mut second_socket, _) = tokio_tungstenite::connect_async(second_request)
        .await
        .expect("second websocket handshake");

    first_socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","input":"first"}"#.into(),
        ))
        .await
        .expect("send first create");
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first create should reach upstream");

    second_socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","input":"second"}"#.into(),
        ))
        .await
        .expect("send second create");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), second_started.notified())
            .await
            .is_err(),
        "balanced scheduling must queue the second response.create"
    );

    release_first.notify_one();
    let first_terminal = tokio::time::timeout(Duration::from_secs(2), first_socket.next())
        .await
        .expect("first terminal event timeout")
        .expect("first terminal event")
        .expect("first terminal frame");
    assert!(
        first_terminal
            .to_text()
            .expect("first terminal text")
            .contains("response.completed")
    );

    tokio::time::timeout(Duration::from_secs(2), second_started.notified())
        .await
        .expect("second create should acquire released capacity");
    let second_terminal = tokio::time::timeout(Duration::from_secs(2), second_socket.next())
        .await
        .expect("second terminal event timeout")
        .expect("second terminal event")
        .expect("second terminal frame");
    assert!(
        second_terminal
            .to_text()
            .expect("second terminal text")
            .contains("response.completed")
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 2);

    first_socket
        .close(None)
        .await
        .expect("close first websocket");
    second_socket
        .close(None)
        .await
        .expect("close second websocket");
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_reuses_one_socket_without_warmup_economics() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let handshake_hits = Arc::new(AtomicUsize::new(0));
    let warmup_hits = Arc::new(AtomicUsize::new(0));
    let create_hits = Arc::new(AtomicUsize::new(0));
    let previous_response_id_hits = Arc::new(AtomicUsize::new(0));
    let handshakes_for_route = handshake_hits.clone();
    let warmups_for_route = warmup_hits.clone();
    let creates_for_route = create_hits.clone();
    let previous_response_ids_for_route = previous_response_id_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let handshakes = handshakes_for_route.clone();
            let warmups = warmups_for_route.clone();
            let creates = creates_for_route.clone();
            let previous_response_ids = previous_response_ids_for_route.clone();
            async move {
                handshakes.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        let body = match message {
                            axum::extract::ws::Message::Text(text) => {
                                serde_json::from_str::<serde_json::Value>(text.as_str()).ok()
                            }
                            axum::extract::ws::Message::Binary(bytes) => {
                                serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                            }
                            _ => None,
                        };
                        if body
                            .as_ref()
                            .and_then(|value| value.get("type"))
                            .and_then(serde_json::Value::as_str)
                            != Some("response.create")
                        {
                            continue;
                        }
                        let body = body.expect("response.create body");
                        if body.get("generate").and_then(serde_json::Value::as_bool) == Some(false)
                        {
                            warmups.fetch_add(1, Ordering::SeqCst);
                            let created = serde_json::json!({
                                "type": "response.created",
                                "response": { "id": "resp-warmup" }
                            });
                            let completed = serde_json::json!({
                                "type": "response.completed",
                                "response": {
                                    "id": "resp-warmup",
                                    "usage": {
                                        "input_tokens": 999,
                                        "output_tokens": 999,
                                        "total_tokens": 1998
                                    }
                                }
                            });
                            let _ = socket
                                .send(axum::extract::ws::Message::Text(created.to_string().into()))
                                .await;
                            let _ = socket
                                .send(axum::extract::ws::Message::Text(
                                    completed.to_string().into(),
                                ))
                                .await;
                            continue;
                        }
                        if body
                            .get("previous_response_id")
                            .and_then(serde_json::Value::as_str)
                            == Some("resp-warmup")
                        {
                            previous_response_ids.fetch_add(1, Ordering::SeqCst);
                        }
                        let hit = creates.fetch_add(1, Ordering::SeqCst);
                        let response_id = format!("resp-reuse-{}", hit + 1);
                        let reported_model = body
                            .get("model")
                            .and_then(serde_json::Value::as_str)
                            .expect("response.create model");
                        let actual_service_tier = if hit == 0 { "priority" } else { "default" };
                        let created = serde_json::json!({
                            "type": "response.created",
                            "response": { "id": response_id }
                        });
                        let completed = serde_json::json!({
                            "type": "response.completed",
                            "response": {
                                "id": response_id,
                                "model": reported_model,
                                "service_tier": actual_service_tier,
                                "usage": {
                                    "input_tokens": (hit + 1) * 10,
                                    "output_tokens": 1,
                                    "total_tokens": (hit + 1) * 10 + 1
                                }
                            }
                        });
                        let _ = socket
                            .send(axum::extract::ws::Message::Text(created.to_string().into()))
                            .await;
                        let _ = socket
                            .send(axum::extract::ws::Message::Text(
                                completed.to_string().into(),
                            ))
                            .await;
                    }
                })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (proxy, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
    let state = proxy.state.clone();
    let mut socket = connect_test_websocket(proxy_addr, "ws-reuse-session").await;

    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::json!({
                "type": "response.create",
                "model": "gpt-5",
                "prompt_cache_key": "ws-reuse-session",
                "generate": false
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("send warmup");
    assert_eq!(
        (next_test_websocket_json(&mut socket).await)["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        (next_test_websocket_json(&mut socket).await)["type"].as_str(),
        Some("response.completed")
    );
    let warmup = state
        .list_recent_finished(10)
        .await
        .into_iter()
        .find(|request| request.session_id.as_deref() == Some("ws-reuse-session"))
        .expect("warmup lifecycle");
    assert!(warmup.usage.is_none(), "warmup must not publish usage");
    assert!(warmup.cost.is_unknown(), "warmup must not publish cost");
    assert_eq!(
        state
            .list_session_stats("codex")
            .await
            .get("ws-reuse-session")
            .map(|stats| stats.turns_total)
            .unwrap_or_default(),
        0,
        "warmup must not increment normal turn accounting"
    );

    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::json!({
                "type": "response.create",
                "model": "gpt-5",
                "input": "first",
                "previous_response_id": "resp-warmup"
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("send first inference create");
    for _ in 0..2 {
        let event = next_test_websocket_json(&mut socket).await;
        assert!(matches!(
            event["type"].as_str(),
            Some("response.created" | "response.completed")
        ));
    }
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::json!({
                "type": "response.create",
                "model": "gpt-5-mini",
                "input": "second"
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("send second inference create");
    assert_eq!(
        (next_test_websocket_json(&mut socket).await)["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        (next_test_websocket_json(&mut socket).await)["type"].as_str(),
        Some("response.completed")
    );

    let finished = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let matching = state
                .list_recent_finished(10)
                .await
                .into_iter()
                .filter(|request| request.session_id.as_deref() == Some("ws-reuse-session"))
                .collect::<Vec<_>>();
            if matching.len() == 3 {
                break matching;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("two websocket requests should commit independently");
    let mut input_tokens = finished
        .iter()
        .filter_map(|request| {
            assert_eq!(
                request.status_code,
                StatusCode::SWITCHING_PROTOCOLS.as_u16()
            );
            let route_attempts = request
                .retry
                .as_ref()
                .map(|retry| retry.route_attempts.as_slice())
                .expect("WebSocket request should retain retry evidence");
            assert_eq!(route_attempts.len(), 1);
            assert_eq!(route_attempts[0].decision, "completed");
            assert_eq!(route_attempts[0].code.as_deref(), Some("completed"));
            request.usage.as_ref().map(|usage| usage.input_tokens)
        })
        .collect::<Vec<_>>();
    input_tokens.sort_unstable();
    assert_eq!(input_tokens, vec![10, 20]);
    assert_ne!(finished[0].id, finished[1].id);
    assert_ne!(finished[1].id, finished[2].id);
    let runtime_store = state.runtime_store_handle();
    let committed = runtime_store
        .query_committed_requests(&crate::runtime_store::CommittedRequestQuery {
            limit: 10,
            ..crate::runtime_store::CommittedRequestQuery::default()
        })
        .expect("query WebSocket request lifecycles");
    let websocket_requests = committed
        .items
        .into_iter()
        .filter(|request| {
            request.payload.finished_request.session_id.as_deref() == Some("ws-reuse-session")
        })
        .collect::<Vec<_>>();
    assert_eq!(websocket_requests.len(), 3);
    let mut response_metadata = websocket_requests
        .iter()
        .filter_map(|request| {
            request
                .payload
                .finished_request
                .usage
                .as_ref()
                .map(|usage| {
                    (
                        usage.input_tokens,
                        request.payload.reported_model.clone(),
                        request.payload.actual_service_tier.clone(),
                    )
                })
        })
        .collect::<Vec<_>>();
    response_metadata.sort_unstable_by_key(|metadata| metadata.0);
    assert_eq!(
        response_metadata,
        vec![
            (10, Some("gpt-5".to_string()), Some("priority".to_string())),
            (
                20,
                Some("gpt-5-mini".to_string()),
                Some("default".to_string())
            ),
        ]
    );
    for request in &websocket_requests {
        let attempts = runtime_store
            .read_attempts_for_logical_request(
                runtime_store.logical_request_handle(request.logical_request_id),
            )
            .expect("read WebSocket upstream attempts");
        assert_eq!(attempts.len(), 1, "each response.create owns one attempt");
        assert_eq!(
            attempts[0]
                .terminal
                .as_ref()
                .map(|terminal| terminal.terminal.outcome),
            Some(crate::runtime_store::AttemptOutcome::Succeeded)
        );
    }
    assert_eq!(handshake_hits.load(Ordering::SeqCst), 1);
    assert_eq!(warmup_hits.load(Ordering::SeqCst), 1);
    assert_eq!(create_hits.load(Ordering::SeqCst), 2);
    assert_eq!(previous_response_id_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        state
            .list_session_stats("codex")
            .await
            .get("ws-reuse-session")
            .map(|stats| stats.turns_total),
        Some(2)
    );

    socket.close(None).await.expect("close websocket");
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_throughput_first_rejects_saturated_create_immediately() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let create_hits = Arc::new(AtomicUsize::new(0));
    let first_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let hits_for_route = create_hits.clone();
    let started_for_route = first_started.clone();
    let release_for_route = release_first.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            let started = started_for_route.clone();
            let release = release_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if !matches!(
                            message,
                            axum::extract::ws::Message::Text(_)
                                | axum::extract::ws::Message::Binary(_)
                        ) {
                            continue;
                        }
                        let hit = hits.fetch_add(1, Ordering::SeqCst);
                        if hit == 0 {
                            started.notify_one();
                            release.notified().await;
                        }
                        send_successful_websocket_response(
                            &mut socket,
                            format!("resp-throughput-{hit}").as_str(),
                        )
                        .await;
                    }
                })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (_, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::ThroughputFirst, 1);
    let mut first = connect_test_websocket(proxy_addr, "ws-throughput-first").await;
    let mut second = connect_test_websocket(proxy_addr, "ws-throughput-second").await;

    send_test_response_create(&mut first, "first").await;
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first create should reach upstream");
    send_test_response_create(&mut second, "second").await;
    let rejection = tokio::time::timeout(
        Duration::from_millis(500),
        next_test_websocket_json(&mut second),
    )
    .await
    .expect("throughput-first rejection must be immediate");
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_capacity_unavailable")
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 1);

    release_first.notify_one();
    assert_eq!(
        (next_test_websocket_json(&mut first).await)["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        (next_test_websocket_json(&mut first).await)["type"].as_str(),
        Some("response.completed")
    );

    first.close(None).await.expect("close first websocket");
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_rejects_overlapping_create_without_forwarding_it() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let create_hits = Arc::new(AtomicUsize::new(0));
    let first_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let hits_for_route = create_hits.clone();
    let started_for_route = first_started.clone();
    let release_for_route = release_first.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            let started = started_for_route.clone();
            let release = release_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    if socket.recv().await.is_some() {
                        hits.fetch_add(1, Ordering::SeqCst);
                        started.notify_one();
                        release.notified().await;
                    }
                })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (_, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
    let mut socket = connect_test_websocket(proxy_addr, "ws-overlap").await;

    send_test_response_create(&mut socket, "first").await;
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first create should reach upstream");
    send_test_response_create(&mut socket, "overlap").await;
    let rejection = next_test_websocket_json(&mut socket).await;
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_overlapping_response_create")
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 1);

    release_first.notify_one();
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_cancel_removes_queued_create_without_upstream_write() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let create_hits = Arc::new(AtomicUsize::new(0));
    let cancel_hits = Arc::new(AtomicUsize::new(0));
    let first_started = Arc::new(tokio::sync::Notify::new());
    let second_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let creates_for_route = create_hits.clone();
    let cancels_for_route = cancel_hits.clone();
    let first_for_route = first_started.clone();
    let second_for_route = second_started.clone();
    let release_for_route = release_first.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let creates = creates_for_route.clone();
            let cancels = cancels_for_route.clone();
            let first_started = first_for_route.clone();
            let second_started = second_for_route.clone();
            let release = release_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        let body = match message {
                            axum::extract::ws::Message::Text(text) => {
                                serde_json::from_str::<serde_json::Value>(text.as_str()).ok()
                            }
                            axum::extract::ws::Message::Binary(bytes) => {
                                serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                            }
                            _ => None,
                        };
                        match body
                            .as_ref()
                            .and_then(|value| value.get("type"))
                            .and_then(serde_json::Value::as_str)
                        {
                            Some("response.cancel") => {
                                cancels.fetch_add(1, Ordering::SeqCst);
                            }
                            Some("response.create") => {
                                let hit = creates.fetch_add(1, Ordering::SeqCst);
                                if hit == 0 {
                                    first_started.notify_one();
                                    release.notified().await;
                                } else {
                                    second_started.notify_one();
                                }
                                send_successful_websocket_response(
                                    &mut socket,
                                    format!("resp-cancel-{hit}").as_str(),
                                )
                                .await;
                            }
                            _ => {}
                        }
                    }
                })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (_, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
    let mut first = connect_test_websocket(proxy_addr, "ws-cancel-first").await;
    let mut second = connect_test_websocket(proxy_addr, "ws-cancel-second").await;

    send_test_response_create(&mut first, "first").await;
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first create should reach upstream");
    send_test_response_create(&mut second, "queued").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    second
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.cancel"}"#.into(),
        ))
        .await
        .expect("cancel queued create");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        cancel_hits.load(Ordering::SeqCst),
        0,
        "a request that has not reached upstream must be canceled locally"
    );

    release_first.notify_one();
    assert_eq!(
        (next_test_websocket_json(&mut first).await)["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        (next_test_websocket_json(&mut first).await)["type"].as_str(),
        Some("response.completed")
    );
    send_test_response_create(&mut second, "after-cancel").await;
    tokio::time::timeout(Duration::from_secs(2), second_started.notified())
        .await
        .expect("new create should use capacity after queued cancellation");
    assert_eq!(
        (next_test_websocket_json(&mut second).await)["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        (next_test_websocket_json(&mut second).await)["type"].as_str(),
        Some("response.completed")
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 2);

    first.close(None).await.expect("close first websocket");
    second.close(None).await.expect("close second websocket");
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_revalidates_manual_disable_after_capacity_wait() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let create_hits = Arc::new(AtomicUsize::new(0));
    let first_started = Arc::new(tokio::sync::Notify::new());
    let second_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let hits_for_route = create_hits.clone();
    let first_for_route = first_started.clone();
    let second_for_route = second_started.clone();
    let release_for_route = release_first.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            let first_started = first_for_route.clone();
            let second_started = second_for_route.clone();
            let release = release_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if !matches!(
                            message,
                            axum::extract::ws::Message::Text(_)
                                | axum::extract::ws::Message::Binary(_)
                        ) {
                            continue;
                        }
                        let hit = hits.fetch_add(1, Ordering::SeqCst);
                        if hit == 0 {
                            first_started.notify_one();
                            release.notified().await;
                        } else {
                            second_started.notify_one();
                        }
                        send_successful_websocket_response(
                            &mut socket,
                            format!("resp-disable-{hit}").as_str(),
                        )
                        .await;
                    }
                })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let (proxy, proxy_addr, proxy_handle) =
        spawn_single_provider_websocket_proxy(upstream_addr, SchedulingPreset::Balanced, 1);
    let state = proxy.state.clone();
    let mut first = connect_test_websocket(proxy_addr, "ws-disable-first").await;
    let mut second = connect_test_websocket(proxy_addr, "ws-disable-second").await;

    send_test_response_create(&mut first, "first").await;
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first create should reach upstream");
    send_test_response_create(&mut second, "queued").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    state
        .set_provider_manual_eligibility(
            ProviderEndpointKey::new("codex", "single", "default"),
            crate::runtime_store::ProviderManualEligibility::Disabled,
            Some("test disables the bound endpoint".to_string()),
            crate::logging::now_ms(),
        )
        .await
        .expect("commit manual endpoint eligibility");
    proxy
        .config
        .publish_provider_policy(state.capture_provider_policy_snapshot().await)
        .await
        .expect("publish provider policy snapshot");

    release_first.notify_one();
    assert_eq!(
        (next_test_websocket_json(&mut first).await)["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        (next_test_websocket_json(&mut first).await)["type"].as_str(),
        Some("response.completed")
    );
    let rejection = tokio::time::timeout(
        Duration::from_secs(2),
        next_test_websocket_json(&mut second),
    )
    .await
    .expect("disabled binding must reject before upstream write");
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_reconnect_required")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), second_started.notified())
            .await
            .is_err(),
        "disabled endpoint must not receive the queued response.create"
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 1);

    first.close(None).await.expect("close first websocket");
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_reuses_compatible_socket_after_pricing_reload() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let helper_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
        scoped.set_path("CODEX_HELPER_HOME", &helper_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let create_hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = create_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if !matches!(
                            message,
                            axum::extract::ws::Message::Text(_)
                                | axum::extract::ws::Message::Binary(_)
                        ) {
                            continue;
                        }
                        let hit = hits.fetch_add(1, Ordering::SeqCst);
                        send_successful_websocket_response(
                            &mut socket,
                            format!("resp-price-{hit}").as_str(),
                        )
                        .await;
                    }
                })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let initial = single_provider_websocket_config(upstream_addr, SchedulingPreset::Balanced, 1);
    crate::config::save_helper_config(&initial)
        .await
        .expect("save initial route config");
    std::fs::write(
        helper_home.join("pricing_overrides.toml"),
        r#"[models.gpt-5]
input_per_1m_usd = "1"
output_per_1m_usd = "2"
confidence = "exact"
"#,
    )
    .expect("write initial pricing override");
    let loaded = crate::config::load_config_with_source()
        .await
        .expect("load initial route config");
    let proxy = proxy_with_loaded_route_graph_config(loaded);
    let retained = proxy.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let mut socket = connect_test_websocket(proxy_addr, "ws-compatible-pricing-reload").await;

    send_test_response_create(&mut socket, "before pricing reload").await;
    assert_eq!(
        next_test_websocket_json(&mut socket).await["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        next_test_websocket_json(&mut socket).await["type"].as_str(),
        Some("response.completed")
    );
    let before = retained.config.capture().await;

    std::fs::write(
        helper_home.join("pricing_overrides.toml"),
        r#"[models.gpt-5]
input_per_1m_usd = "3"
output_per_1m_usd = "4"
confidence = "exact"
"#,
    )
    .expect("write reloaded pricing override");
    assert!(
        retained
            .config
            .force_reload_from_disk()
            .await
            .expect("reload pricing override"),
        "changed pricing must publish a new runtime revision"
    );
    let after = retained.config.capture().await;
    assert_eq!(after.revision(), before.revision() + 1);
    assert_ne!(
        after.operator_pricing_catalog().revision(),
        before.operator_pricing_catalog().revision()
    );

    send_test_response_create(&mut socket, "after pricing reload").await;
    assert_eq!(
        next_test_websocket_json(&mut socket).await["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        next_test_websocket_json(&mut socket).await["type"].as_str(),
        Some("response.completed")
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 2);

    socket.close(None).await.expect("close WebSocket");
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
    let _ = std::fs::remove_dir_all(helper_home);
}

#[tokio::test]
async fn responses_websocket_requires_reconnect_when_revision_changes_during_capacity_wait() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let helper_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
        scoped.set_path("CODEX_HELPER_HOME", &helper_home);
    }
    enable_responses_websocket_for_test(&codex_home);

    let create_hits = Arc::new(AtomicUsize::new(0));
    let first_started = Arc::new(tokio::sync::Notify::new());
    let second_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let hits_for_route = create_hits.clone();
    let first_for_route = first_started.clone();
    let second_for_route = second_started.clone();
    let release_for_route = release_first.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = hits_for_route.clone();
            let first_started = first_for_route.clone();
            let second_started = second_for_route.clone();
            let release = release_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if !matches!(
                            message,
                            axum::extract::ws::Message::Text(_)
                                | axum::extract::ws::Message::Binary(_)
                        ) {
                            continue;
                        }
                        let hit = hits.fetch_add(1, Ordering::SeqCst);
                        if hit == 0 {
                            first_started.notify_one();
                            release.notified().await;
                        } else {
                            second_started.notify_one();
                        }
                        send_successful_websocket_response(
                            &mut socket,
                            format!("resp-revision-{hit}").as_str(),
                        )
                        .await;
                    }
                })
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let initial = single_provider_websocket_config(upstream_addr, SchedulingPreset::Balanced, 1);
    crate::config::save_helper_config(&initial)
        .await
        .expect("save initial route config");
    let loaded = crate::config::load_config_with_source()
        .await
        .expect("load initial route config");
    let proxy = proxy_with_loaded_route_graph_config(loaded);
    let retained = proxy.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let mut first = connect_test_websocket(proxy_addr, "ws-revision-first").await;
    let mut second = connect_test_websocket(proxy_addr, "ws-revision-second").await;

    send_test_response_create(&mut first, "first").await;
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first create should reach upstream");
    send_test_response_create(&mut second, "queued").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut reloaded = initial;
    reloaded
        .codex
        .routing
        .as_mut()
        .expect("routing config")
        .scheduling_preset = SchedulingPreset::ContinuityFirst;
    crate::config::save_helper_config(&reloaded)
        .await
        .expect("save reloaded route config");
    assert!(
        retained
            .config
            .force_reload_from_disk()
            .await
            .expect("reload route config"),
        "changed route config should publish a new runtime revision"
    );

    release_first.notify_one();
    assert_eq!(
        (next_test_websocket_json(&mut first).await)["type"].as_str(),
        Some("response.created")
    );
    assert_eq!(
        (next_test_websocket_json(&mut first).await)["type"].as_str(),
        Some("response.completed")
    );
    let rejection = tokio::time::timeout(
        Duration::from_secs(2),
        next_test_websocket_json(&mut second),
    )
    .await
    .expect("revision drift must reject before upstream write");
    assert_eq!(rejection["type"].as_str(), Some("error"));
    assert_eq!(
        rejection["code"].as_str(),
        Some("websocket_reconnect_required")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), second_started.notified())
            .await
            .is_err(),
        "stale route revision must not receive the queued response.create"
    );
    assert_eq!(create_hits.load(Ordering::SeqCst), 1);

    first.close(None).await.expect("close first websocket");
    proxy_handle.abort();
    upstream_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
    let _ = std::fs::remove_dir_all(helper_home);
}

#[tokio::test]
async fn responses_websocket_no_routable_candidate_rejects_without_request() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );

    let upstream =
        axum::Router::new().route(
            "/v1/responses",
            get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                ws.on_upgrade(|_| async move {})
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

    let request = format!("ws://{}/v1/responses", proxy.addr)
        .into_client_request()
        .expect("ws request");
    let error = tokio_tungstenite::connect_async(request)
        .await
        .expect_err("no routable candidate must prevent downstream 101");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP route rejection, got: {error}");
    };
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        state.list_active_requests().await.is_empty(),
        "websocket no-routable preparation failure must not leak active requests"
    );
    assert!(state.list_recent_finished(10).await.is_empty());

    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_returns_upstream_426_before_downstream_upgrade() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );

    let hits = Arc::new(AtomicUsize::new(0));
    let hits_for_route = hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(move || {
            let hits = hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                let mut response = Response::new(Body::from("websocket beta required"));
                *response.status_mut() = StatusCode::UPGRADE_REQUIRED;
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain"),
                );
                response.headers_mut().insert(
                    axum::http::header::RETRY_AFTER,
                    HeaderValue::from_static("17"),
                );
                response.headers_mut().insert(
                    axum::http::header::SET_COOKIE,
                    HeaderValue::from_static("upstream-secret=1"),
                );
                response
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let proxy_service = proxy_service(make_helper_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    ));
    let state = proxy_service.state.clone();
    let proxy = spawn_proxy_service(proxy_service);

    let request = format!("ws://{}/v1/responses", proxy.addr)
        .into_client_request()
        .expect("ws request");
    let error = tokio_tungstenite::connect_async(request)
        .await
        .expect_err("upstream rejection must prevent downstream 101");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP handshake rejection, got: {error}");
    };

    assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
    if let Some(body) = response.body().as_deref() {
        assert!(
            body.is_empty()
                || body == b"websocket beta required"
                || body == b"upstream WebSocket handshake rejected",
            "unexpected best-effort upstream rejection body: {}",
            String::from_utf8_lossy(body)
        );
    }
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok()),
        Some("17")
    );
    assert!(
        !response
            .headers()
            .contains_key(axum::http::header::SET_COOKIE)
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert!(state.list_active_requests().await.is_empty());
    assert!(state.list_recent_finished(10).await.is_empty());
    assert!(state.get_provider_balance_view("codex").await.is_empty());
    assert!(
        state
            .capture_provider_policy_snapshot()
            .await
            .projections
            .is_empty()
    );

    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_transport_failure_returns_502_without_request() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );

    let unused_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind unused port");
    let unused_addr = unused_listener.local_addr().expect("unused address");
    drop(unused_listener);
    let cfg = make_helper_config(
        vec![UpstreamConfig {
            base_url: format!("http://{unused_addr}/v1"),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy_service = proxy_service(cfg);
    let state = proxy_service.state.clone();
    let proxy = spawn_proxy_service(proxy_service);

    let request = format!("ws://{}/v1/responses", proxy.addr)
        .into_client_request()
        .expect("ws request");
    let error = tokio_tungstenite::connect_async(request)
        .await
        .expect_err("upstream transport failure must prevent downstream 101");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP transport rejection, got: {error}");
    };

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(state.list_active_requests().await.is_empty());
    assert!(state.list_recent_finished(10).await.is_empty());
    assert!(state.get_provider_balance_view("codex").await.is_empty());
    assert!(
        state
            .capture_provider_policy_snapshot()
            .await
            .projections
            .is_empty()
    );

    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_connects_upstream_once_before_101_and_isolates_credentials() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );

    let handshake_hits = Arc::new(AtomicUsize::new(0));
    let seen_headers = Arc::new(std::sync::Mutex::new(None::<HeaderMap>));
    let upstream_entered = Arc::new(tokio::sync::Notify::new());
    let release_upstream = Arc::new(tokio::sync::Notify::new());
    let hits_for_route = handshake_hits.clone();
    let headers_for_route = seen_headers.clone();
    let entered_for_route = upstream_entered.clone();
    let release_for_route = release_upstream.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        get(
            move |ws: axum::extract::ws::WebSocketUpgrade, headers: HeaderMap| {
                let hits = hits_for_route.clone();
                let seen_headers = headers_for_route.clone();
                let entered = entered_for_route.clone();
                let release = release_for_route.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    *seen_headers.lock().expect("headers lock") = Some(headers);
                    entered.notify_one();
                    release.notified().await;
                    let mut response = ws.on_upgrade(move |mut socket| async move {
                        if socket.recv().await.is_some() {
                            let _ = socket
                                .send(axum::extract::ws::Message::Text(
                                    r#"{"type":"response.created","response":{"id":"resp-preflight"}}"#
                                        .into(),
                                ))
                                .await;
                        }
                    });
                    response.headers_mut().insert(
                        "x-codex-turn-state",
                        HeaderValue::from_static("reviewed-turn-state"),
                    );
                    response.headers_mut().insert(
                        "x-request-id",
                        HeaderValue::from_static("request-safe"),
                    );
                    response.headers_mut().insert(
                        "x-upstream-private",
                        HeaderValue::from_static("must-not-cross-boundary"),
                    );
                    response.headers_mut().insert(
                        axum::http::header::SET_COOKIE,
                        HeaderValue::from_static("upstream-session=secret"),
                    );
                    response
                }
            },
        ),
    );
    let upstream = spawn_test_upstream(upstream);
    let mut upstream_config = upstream.upstream_config();
    upstream_config.auth.auth_token = Some("server-owned-token".to_string().into());
    upstream_config.auth.api_key = Some("server-owned-api-key".to_string().into());
    let proxy = spawn_test_proxy(make_helper_config(
        vec![upstream_config],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    ));

    let mut request = format!("ws://{}/v1/responses", proxy.addr)
        .into_client_request()
        .expect("ws request");
    request.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        HeaderValue::from_static("Bearer client-secret"),
    );
    request.headers_mut().insert(
        axum::http::header::PROXY_AUTHORIZATION,
        HeaderValue::from_static("Basic client-proxy-secret"),
    );
    request.headers_mut().insert(
        "x-api-key",
        HeaderValue::from_static("client-api-key-secret"),
    );
    request.headers_mut().insert(
        axum::http::header::COOKIE,
        HeaderValue::from_static("session=client-secret"),
    );
    request.headers_mut().insert(
        "x-codex-helper-admin-token",
        HeaderValue::from_static("client-admin-secret"),
    );
    request.headers_mut().insert(
        "x-forwarded-api-key",
        HeaderValue::from_static("client-forwarded-secret"),
    );
    request.headers_mut().insert(
        "x-private-metadata",
        HeaderValue::from_static("must-not-cross-boundary"),
    );
    request.headers_mut().insert(
        "session-id",
        HeaderValue::from_static("ws-preflight-session"),
    );

    let mut connect = tokio::spawn(tokio_tungstenite::connect_async(request));
    tokio::time::timeout(Duration::from_secs(2), upstream_entered.notified())
        .await
        .expect("upstream handshake request");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut connect)
            .await
            .is_err(),
        "downstream handshake completed before upstream returned 101"
    );
    release_upstream.notify_one();
    let (mut socket, downstream_response) = connect
        .await
        .expect("join downstream connect")
        .expect("connect proxy websocket");
    assert_eq!(
        handshake_hits.load(Ordering::SeqCst),
        1,
        "downstream 101 must wait for exactly one upstream handshake"
    );
    assert_eq!(
        downstream_response
            .headers()
            .get("x-codex-turn-state")
            .and_then(|value| value.to_str().ok()),
        Some("reviewed-turn-state")
    );
    assert_eq!(
        downstream_response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok()),
        Some("request-safe")
    );
    assert!(
        !downstream_response
            .headers()
            .contains_key("x-upstream-private")
    );
    assert!(
        !downstream_response
            .headers()
            .contains_key(axum::http::header::SET_COOKIE)
    );

    let headers = seen_headers
        .lock()
        .expect("headers lock")
        .clone()
        .expect("upstream handshake headers");
    assert_eq!(
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer server-owned-token")
    );
    assert_eq!(
        headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok()),
        Some("server-owned-api-key")
    );
    assert_eq!(
        headers
            .get("openai-beta")
            .and_then(|value| value.to_str().ok()),
        Some("responses_websockets=2026-02-06")
    );
    assert_eq!(
        headers
            .get("session-id")
            .and_then(|value| value.to_str().ok()),
        Some("ws-preflight-session")
    );
    for stripped in [
        "cookie",
        "proxy-authorization",
        "x-codex-helper-admin-token",
        "x-forwarded-api-key",
        "x-private-metadata",
    ] {
        assert!(
            !headers.contains_key(stripped),
            "client-local header leaked upstream: {stripped}"
        );
    }

    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","stream":true,"prompt_cache_key":"ws-preflight-session"}"#.into(),
        ))
        .await
        .expect("send first frame");
    let event = socket
        .next()
        .await
        .expect("event")
        .expect("event ok")
        .to_text()
        .expect("event text")
        .to_string();
    assert!(event.contains("response.created"), "{event}");
    assert_eq!(handshake_hits.load(Ordering::SeqCst), 1);

    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_rejects_ambiguous_route_before_101() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:1"
wire_api = "responses"
supports_websockets = true
"#,
    );

    let first_hits = Arc::new(AtomicUsize::new(0));
    let first_hits_for_route = first_hits.clone();
    let first = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = first_hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(|_| async move {})
            }
        }),
    ));
    let second_hits = Arc::new(AtomicUsize::new(0));
    let second_hits_for_route = second_hits.clone();
    let second = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let hits = second_hits_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(|_| async move {})
            }
        }),
    ));
    let proxy = spawn_test_proxy(make_helper_config(
        vec![first.upstream_config(), second.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    ));

    let request = format!("ws://{}/v1/responses", proxy.addr)
        .into_client_request()
        .expect("ws request");
    let error = tokio_tungstenite::connect_async(request)
        .await
        .expect_err("ambiguous handshake route must prevent downstream 101");
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected HTTP route rejection, got: {error}");
    };
    assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
    let body = response.body().as_deref().unwrap_or_default();
    assert!(
        String::from_utf8_lossy(body).contains(WS_PROVIDER_ENDPOINT_HEADER),
        "unexpected route ambiguity body: {}",
        String::from_utf8_lossy(body)
    );
    assert_eq!(first_hits.load(Ordering::SeqCst), 0);
    assert_eq!(second_hits.load(Ordering::SeqCst), 0);

    let _ = std::fs::remove_dir_all(codex_home);
}
