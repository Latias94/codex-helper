use super::*;

mod config_failover;
mod response_semantics;

fn retry_layer_config(
    max_attempts: u32,
    on_status: &str,
    on_class: Vec<String>,
    strategy: RetryStrategy,
) -> crate::config::RetryLayerConfig {
    crate::config::RetryLayerConfig {
        max_attempts: Some(max_attempts),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some(on_status.to_string()),
        on_class: Some(on_class),
        strategy: Some(strategy),
    }
}

fn retry_config(
    max_attempts: u32,
    on_status: &str,
    on_class: Vec<String>,
    strategy: RetryStrategy,
) -> RetryConfig {
    retry_config_with_cooldowns(max_attempts, on_status, on_class, strategy, 0, 0, 0)
}

fn retry_config_with_cooldowns(
    max_attempts: u32,
    on_status: &str,
    on_class: Vec<String>,
    strategy: RetryStrategy,
    cloudflare_challenge_cooldown_secs: u64,
    cloudflare_timeout_cooldown_secs: u64,
    transport_cooldown_secs: u64,
) -> RetryConfig {
    RetryConfig {
        upstream: Some(retry_layer_config(
            max_attempts,
            on_status,
            on_class,
            strategy,
        )),
        cloudflare_challenge_cooldown_secs: Some(cloudflare_challenge_cooldown_secs),
        cloudflare_timeout_cooldown_secs: Some(cloudflare_timeout_cooldown_secs),
        transport_cooldown_secs: Some(transport_cooldown_secs),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    }
}

#[tokio::test]
async fn proxy_failover_retries_502_then_uses_second_upstream() {
    run_failover_retries_502_then_uses_second_upstream(false).await;
}

#[tokio::test]
async fn route_executor_request_path_retries_502_then_uses_second_upstream() {
    let before = super::super::provider_execution::route_executor_request_path_test_invocations();

    run_failover_retries_502_then_uses_second_upstream(true).await;

    let after = super::super::provider_execution::route_executor_request_path_test_invocations();
    assert_eq!(after, before + 1);
}

async fn run_failover_retries_502_then_uses_second_upstream(use_route_executor: bool) {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "nope" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

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
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = retry_config(2, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let mut req = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#);
    if use_route_executor {
        req = req.header(
            super::super::provider_execution::ROUTE_EXECUTOR_REQUEST_PATH_TEST_HEADER,
            "1",
        );
    }
    let resp = req.send().await.expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(
        body.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body}"
    );
    let finished = state.list_recent_finished(10).await;
    let request = finished
        .iter()
        .find(|request| request.status_code == StatusCode::OK)
        .expect("finished request");
    let retry = request.retry.as_ref().expect("retry info");
    assert_eq!(retry.route_attempts.len(), 3);
    assert_eq!(
        retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.endpoint_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("0"), Some("0"), Some("1")]
    );
    assert_eq!(
        retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.route_path.clone())
            .collect::<Vec<_>>(),
        vec![
            vec!["legacy".to_string(), "test".to_string(), "u1".to_string()],
            vec!["legacy".to_string(), "test".to_string(), "u1".to_string()],
            vec!["legacy".to_string(), "test".to_string(), "u2".to_string()],
        ]
    );
    let route_decision = request
        .route_decision
        .as_ref()
        .expect("finished route decision");
    assert_eq!(route_decision.endpoint_id.as_deref(), Some("1"));
    assert_eq!(route_decision.route_path, vec!["legacy", "test", "u2"]);
    // Two-layer model: retry current upstream first, then fail over.
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_same_upstream_retries_502_then_succeeds_without_failover() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            let n = u1_hits.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "err": "first attempt 502" })),
                )
            } else {
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "ok": true, "upstream": 1 })),
                )
            }
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

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
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = retry_config(2, "502", Vec::new(), RetryStrategy::SameUpstream);
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(
        body.contains(r#""upstream":1"#),
        "expected response from upstream1, got: {body}"
    );
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_across_requests_penalizes_502_when_no_internal_retry() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "always 502" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(
                        b"data: {\"ok\":true,\"upstream\":2}\n\n",
                    )]
                    .into_iter()
                    .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
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
            max_attempts: Some(2),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        allow_cross_station_before_first_output: Some(true),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    // First request hits upstream1, gets a retryable 502, and fails over to upstream2.
    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body1_s}"
    );
    let mut finished = Vec::new();
    for _ in 0..100 {
        finished = state.list_recent_finished(10).await;
        if finished.iter().any(|request| request.retry.is_some()) {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    let retry = finished
        .iter()
        .find_map(|request| request.retry.as_ref())
        .expect("streaming failover should record retry info");
    assert_eq!(retry.attempts, 2);
    assert_eq!(retry.route_attempts.len(), 2);
    assert_eq!(retry.route_attempts[0].decision, "failed_status");
    assert_eq!(retry.route_attempts[0].provider_id.as_deref(), Some("u1"));
    assert_eq!(retry.route_attempts[0].provider_attempt, Some(1));
    assert_eq!(retry.route_attempts[1].decision, "completed");
    assert_eq!(retry.route_attempts[1].provider_id.as_deref(), Some("u2"));
    assert_eq!(retry.route_attempts[1].provider_attempt, Some(1));
    assert_eq!(retry.route_attempts[1].upstream_index, Some(1));
    assert!(retry.route_attempts[1].upstream_headers_ms.is_some());

    // Second request should now go directly to upstream2 thanks to the cooldown on upstream1.
    let (status2, body2) = {
        let mut last_status = StatusCode::INTERNAL_SERVER_ERROR;
        let mut last_body: Bytes = Bytes::new();
        for attempt in 0..3 {
            let resp2 = client
                .post(format!("http://{}/v1/responses", proxy_addr))
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .body(r#"{"model":"gpt","input":"hi"}"#)
                .send()
                .await
                .expect("send");
            last_status = resp2.status();
            last_body = resp2.bytes().await.expect("read bytes");
            if last_status == StatusCode::OK {
                break;
            }
            if attempt < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }
        }
        (last_status, last_body)
    };
    assert_eq!(status2, StatusCode::OK);
    let body_s = String::from_utf8_lossy(&body2);
    assert!(
        body_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body_s}"
    );

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_across_requests_penalizes_transport_error_when_no_internal_retry() {
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(
                        b"data: {\"ok\":true,\"upstream\":2}\n\n",
                    )]
                    .into_iter()
                    .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let unused = reserve_unused_local_addr();

    let proxy_client = Client::new();
    let retry = retry_config_with_cooldowns(
        1,
        "502",
        vec!["upstream_transport_error".to_string()],
        RetryStrategy::Failover,
        0,
        0,
        60,
    );
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", unused),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body1_s}"
    );

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body = resp2.bytes().await.expect("read bytes");
    let body_s = String::from_utf8_lossy(&body);
    assert!(
        body_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body_s}"
    );
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_across_requests_penalizes_cloudflare_challenge_when_no_internal_retry() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            let mut resp = Response::new(Body::from(
                "<html><body>/cdn-cgi/ challenge-platform __CF$cv$params</body></html>",
            ));
            *resp.status_mut() = StatusCode::FORBIDDEN;
            resp.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            resp.headers_mut()
                .insert("server", HeaderValue::from_static("cloudflare"));
            resp.headers_mut()
                .insert("cf-ray", HeaderValue::from_static("test"));
            resp
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(
                        b"data: {\"ok\":true,\"upstream\":2}\n\n",
                    )]
                    .into_iter()
                    .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = retry_config_with_cooldowns(
        1,
        "502",
        vec!["cloudflare_challenge".to_string()],
        RetryStrategy::Failover,
        60,
        0,
        0,
    );
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body1_s}"
    );

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = resp2.bytes().await.expect("read bytes");
    let body2_s = String::from_utf8_lossy(&body2);
    assert!(
        body2_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body2_s}"
    );

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_multi_config_failover_across_requests_respects_cooldown() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let p_hits = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            p_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "primary 502" })),
            )
        }),
    );
    let (p_addr, p_handle) = spawn_axum_server(primary);

    let b_hits = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            b_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(
                        b"data: {\"ok\":true,\"upstream\":\"backup\"}\n\n",
                    )]
                    .into_iter()
                    .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (b_addr, b_handle) = spawn_axum_server(backup);

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
            max_attempts: Some(2),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        allow_cross_station_before_first_output: Some(true),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };

    let mut mgr = ServiceConfigManager {
        active: Some("primary".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "primary".to_string(),
        ServiceConfig {
            name: "primary".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", p_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "primary".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "backup".to_string(),
        ServiceConfig {
            name: "backup".to_string(),
            alias: None,
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", b_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "backup".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );

    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry,
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":"backup""#),
        "expected response from backup, got: {body1_s}"
    );

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = resp2.bytes().await.expect("read bytes");
    let body2_s = String::from_utf8_lossy(&body2);
    assert!(
        body2_s.contains(r#""upstream":"backup""#),
        "expected response from backup, got: {body2_s}"
    );

    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    p_handle.abort();
    b_handle.abort();
}

#[tokio::test]
async fn proxy_multi_config_does_not_cross_station_failover_when_pre_output_guard_disabled() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let p_hits = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            p_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "primary 502" })),
            )
        }),
    );
    let (p_addr, p_handle) = spawn_axum_server(primary);

    let b_hits = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            b_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "upstream": "backup" })),
            )
        }),
    );
    let (b_addr, b_handle) = spawn_axum_server(backup);

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
            max_attempts: Some(2),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        allow_cross_station_before_first_output: Some(false),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };

    let mut mgr = ServiceConfigManager {
        active: Some("primary".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "primary".to_string(),
        ServiceConfig {
            name: "primary".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", p_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "primary".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "backup".to_string(),
        ServiceConfig {
            name: "backup".to_string(),
            alias: None,
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", b_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "backup".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );

    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry,
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::BAD_GATEWAY);
    let body1 = resp1.text().await.expect("read body");
    assert!(
        body1.contains("primary 502"),
        "expected first request to fail at primary, got: {body1}"
    );

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::BAD_GATEWAY);
    let body2 = resp2.text().await.expect("read body");
    assert!(
        !body2.contains("backup"),
        "expected second request to stay blocked instead of using backup, got: {body2}"
    );

    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    p_handle.abort();
    b_handle.abort();
}

#[tokio::test]
async fn proxy_does_not_failover_when_502_is_not_retryable_and_threshold_not_reached() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "always 502" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(
                        b"data: {\"ok\":true,\"upstream\":2}\n\n",
                    )]
                    .into_iter()
                    .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::BAD_GATEWAY);

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::BAD_GATEWAY);

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_retries_each_upstream_once_and_stops_when_all_avoided() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "u1 502" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "u2 502" })),
            )
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
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
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(
        proxy_client,
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = resp.text().await.expect("read body");
    assert!(
        body.contains("all upstream attempts failed"),
        "expected aggregated failure summary, got: {body}"
    );
    assert!(
        body.contains("upstream[0]") && body.contains("upstream[1]"),
        "expected both upstream attempts in failure summary, got: {body}"
    );
    assert!(
        body.contains("last_error:") && body.contains("u2 502"),
        "expected final upstream error body in failure summary, got: {body}"
    );
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn failed_single_attempt_records_route_attempts_for_logs() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "single 502" })),
            )
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

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
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", upstream_addr),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: {
                let mut tags = HashMap::new();
                tags.insert("provider_id".to_string(), "solo".to_string());
                tags
            },
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        retry,
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = resp.text().await.expect("read body");
    assert!(
        body.contains("all upstream attempts failed") && body.contains("single 502"),
        "expected single-attempt failure summary, got: {body}"
    );

    let finished = state.list_recent_finished(1).await;
    let retry = finished
        .first()
        .and_then(|request| request.retry.as_ref())
        .expect("failed single attempt should keep route attempts in request logs");
    assert_eq!(retry.attempts, 1);
    assert_eq!(retry.route_attempts.len(), 1);
    assert_eq!(retry.route_attempts[0].decision, "failed_status");
    assert_eq!(retry.route_attempts[0].provider_id.as_deref(), Some("solo"));
    assert_eq!(retry.route_attempts[0].status_code, Some(502));

    proxy_handle.abort();
    upstream_handle.abort();
}
