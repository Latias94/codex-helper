use super::*;

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

    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", u_addr),
            auth: UpstreamAuth {
                auth_token: Some("server-token".to_string()),
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::from([("gpt-5".to_string(), "relay-gpt-5".to_string())]),
        }],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
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
async fn responses_websocket_route_unavailable_records_route_attempts() {
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
    let (u_addr, u_handle) = spawn_axum_server(upstream);

    let retry = RetryConfig {
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([(
                "ws-provider".to_string(),
                ProviderConfigV4 {
                    base_url: Some(format!("http://{u_addr}/v1")),
                    inline_auth: UpstreamAuth::default(),
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "ws-provider".to_string(),
            ])),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    state
        .penalize_provider_endpoint_attempt(
            "codex",
            crate::runtime_identity::ProviderEndpointKey::new("codex", "ws-provider", "default"),
            30,
            crate::lb::CooldownBackoff {
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
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect proxy websocket");
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","stream":true,"prompt_cache_key":"ws-route-unavailable"}"#.into(),
        ))
        .await
        .expect("send first frame");

    let close = socket.next().await.expect("close frame").expect("close ok");
    assert!(matches!(
        close,
        tokio_tungstenite::tungstenite::Message::Close(_)
    ));

    let finished = find_finished_request(&state, 10, |request| {
        request.session_id.as_deref() == Some("ws-route-unavailable")
    })
    .await
    .expect("finished websocket request");
    assert_eq!(finished.status_code, 502);
    let retry = finished.retry.as_ref().expect("retry trace");
    assert_eq!(retry.attempts, 0);
    assert_eq!(
        retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.decision.as_str())
            .collect::<Vec<_>>(),
        vec!["route_unavailable"]
    );

    proxy_handle.abort();
    u_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_rejects_compaction_trigger_without_route_affinity() {
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
                        let _ = socket
                            .send(axum::extract::ws::Message::Text(
                                r#"{"type":"response.created","response":{"id":"resp-b"}}"#.into(),
                            ))
                            .await;
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
                        let _ = socket
                            .send(axum::extract::ws::Message::Text(
                                r#"{"type":"response.created","response":{"id":"resp-c"}}"#.into(),
                            ))
                            .await;
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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::FallbackSticky;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
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
            r#"{"type":"response.create","model":"gpt-5","stream":true,"prompt_cache_key":"ws-missing-v2-affinity","input":[{"role":"user","content":"compact me"},{"type":"compaction_trigger"}]}"#.into(),
        ))
        .await
        .expect("send first frame");

    let close = socket.next().await.expect("close frame").expect("close ok");
    assert!(matches!(
        close,
        tokio_tungstenite::tungstenite::Message::Close(_)
    ));
    assert_eq!(b_hits.load(Ordering::SeqCst), 0);
    assert_eq!(c_hits.load(Ordering::SeqCst), 0);

    let finished = find_finished_request(&state, 10, |request| {
        request.session_id.as_deref() == Some("ws-missing-v2-affinity")
    })
    .await
    .expect("finished websocket request");
    assert_eq!(
        finished.status_code,
        StatusCode::SERVICE_UNAVAILABLE.as_u16()
    );

    let traces = crate::logging::read_recent_control_trace_entries(20)
        .expect("read recent control trace entries");
    let block = traces
        .iter()
        .rev()
        .find(|entry| entry.event.as_deref() == Some("route_continuity_blocked"))
        .expect("route continuity blocked trace");
    assert_eq!(
        block.payload["continuity_class"].as_str(),
        Some("provider_state_bound")
    );
    assert_eq!(
        block.payload["reason"].as_str(),
        Some("state_bound_compact_missing_affinity")
    );
    assert_eq!(
        block.payload["transport"].as_str(),
        Some("responses_websocket")
    );

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
    let _ = std::fs::remove_dir_all(temp_dir);
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
                        let _ = socket
                            .send(axum::extract::ws::Message::Text(
                                r#"{"type":"response.created","response":{"id":"resp-single"}}"#
                                    .into(),
                            ))
                            .await;
                    }
                })
            }
        }),
    );
    let (u_addr, u_handle) = spawn_axum_server(upstream);

    let mut routing = RoutingConfigV4::ordered_failover(vec!["single".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::FallbackSticky;
    let v4 = ProxyConfigV4 {
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([(
                "single".to_string(),
                ProviderConfigV4 {
                    base_url: Some(format!("http://{u_addr}/v1")),
                    inline_auth: UpstreamAuth::default(),
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
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
async fn responses_websocket_no_routable_station_finishes_active_request() {
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

    let cfg = make_proxy_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    state
        .set_station_runtime_state_override(
            "codex",
            "test".to_string(),
            RuntimeConfigState::BreakerOpen,
            crate::logging::now_ms(),
        )
        .await;
    let proxy = spawn_proxy_service(proxy);

    let request = format!("ws://{}/v1/responses", proxy.addr)
        .into_client_request()
        .expect("ws request");
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect proxy websocket");
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","stream":true,"prompt_cache_key":"ws-no-routable"}"#.into(),
        ))
        .await
        .expect("send first frame");

    let close = socket.next().await.expect("close frame").expect("close ok");
    assert!(matches!(
        close,
        tokio_tungstenite::tungstenite::Message::Close(_)
    ));
    assert!(
        state.list_active_requests().await.is_empty(),
        "websocket no-routable preparation failure must not leak active requests"
    );
    let finished = find_finished_request(&state, 10, |request| {
        request.session_id.as_deref() == Some("ws-no-routable")
    })
    .await
    .expect("finished websocket request");
    assert_eq!(finished.status_code, StatusCode::BAD_GATEWAY.as_u16());

    let _ = std::fs::remove_dir_all(codex_home);
}

#[tokio::test]
async fn responses_websocket_success_records_legacy_route_affinity() {
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
    let first_upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let first_hits = first_hits_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    first_hits.fetch_add(1, Ordering::SeqCst);
                    if socket.recv().await.is_some() {
                        let _ = socket
                            .send(axum::extract::ws::Message::Text(
                                r#"{"type":"response.created","response":{"id":"resp-first"}}"#
                                    .into(),
                            ))
                            .await;
                    }
                })
            }
        }),
    );
    let first_upstream = spawn_test_upstream(first_upstream);

    let second_hits = Arc::new(AtomicUsize::new(0));
    let second_hits_for_route = second_hits.clone();
    let second_upstream = axum::Router::new().route(
        "/v1/responses",
        get(move |ws: axum::extract::ws::WebSocketUpgrade| {
            let second_hits = second_hits_for_route.clone();
            async move {
                ws.on_upgrade(move |mut socket| async move {
                    second_hits.fetch_add(1, Ordering::SeqCst);
                    if socket.recv().await.is_some() {
                        let _ = socket
                            .send(axum::extract::ws::Message::Text(
                                r#"{"type":"response.created","response":{"id":"resp-affinity"}}"#
                                    .into(),
                            ))
                            .await;
                    }
                })
            }
        }),
    );
    let second_upstream = spawn_test_upstream(second_upstream);

    let cfg = make_proxy_config(
        vec![
            first_upstream.upstream_config(),
            second_upstream.upstream_config(),
        ],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let legacy_template =
        crate::routing_ir::compile_legacy_route_plan_template("codex", cfg.codex.configs.values());
    let route_graph_key = legacy_template.route_graph_key();
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    state
        .record_session_route_affinity_success(
            "ws-legacy-affinity",
            SessionRouteAffinityTarget {
                route_graph_key: route_graph_key.clone(),
                session_identity_source: Some(SessionIdentitySource::PromptCacheKey),
                provider_endpoint: ProviderEndpointKey::new("codex", "test#1", "1"),
                upstream_base_url: second_upstream.base_url(),
                route_path: vec![
                    "legacy".to_string(),
                    "test".to_string(),
                    "test#1".to_string(),
                ],
            },
            Some("test_seed".to_string()),
            crate::logging::now_ms(),
        )
        .await;
    let proxy = spawn_proxy_service(proxy);

    let request = format!("ws://{}/v1/responses", proxy.addr)
        .into_client_request()
        .expect("ws request");
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("connect proxy websocket");
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"response.create","model":"gpt-5","stream":true,"prompt_cache_key":"ws-legacy-affinity"}"#.into(),
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

    socket.close(None).await.expect("close websocket");
    let finished = find_finished_request(&state, 10, |request| {
        request.session_id.as_deref() == Some("ws-legacy-affinity")
    })
    .await
    .expect("finished websocket request");
    assert_eq!(
        finished.status_code,
        StatusCode::SWITCHING_PROTOCOLS.as_u16()
    );
    let affinity = state
        .get_session_route_affinity("ws-legacy-affinity")
        .await
        .expect("websocket success should record route affinity");
    assert_eq!(affinity.route_graph_key, route_graph_key);
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "test#1");
    assert_eq!(
        affinity.session_identity_source,
        Some(SessionIdentitySource::PromptCacheKey)
    );
    assert_eq!(first_hits.load(Ordering::SeqCst), 0);
    assert_eq!(second_hits.load(Ordering::SeqCst), 1);

    let _ = std::fs::remove_dir_all(codex_home);
}
