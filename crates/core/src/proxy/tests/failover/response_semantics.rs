use super::*;

#[path = "response_semantics_compact.rs"]
mod response_semantics_compact;
#[path = "response_semantics_websocket.rs"]
mod response_semantics_websocket;
use crate::proxy::tests::harness::{
    find_finished_request, post_compact_json, post_responses_json, proxy_service,
    spawn_proxy_service, spawn_test_proxy, spawn_test_upstream,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::{RuntimeConfigState, SessionIdentitySource, SessionRouteAffinityTarget};
use flate2::Compression;
use flate2::write::GzEncoder;
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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);

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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);

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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);

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

#[tokio::test]
async fn proxy_translates_openai_models_list_to_codex_models_response_when_enabled() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }
    write_text_file(
        &temp_dir.join("config.toml"),
        r#"
[codex.client_patch]
translate_models = true
"#,
    );

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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);

    let proxy = spawn_test_proxy(cfg);

    let resp = Client::new()
        .get(proxy.url("/models"))
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.bytes().await.expect("body");
    let value = assert_codex_models_response(body.as_ref(), "gpt-5.5", true);
    let models = value["models"].as_array().expect("models array");
    let auto_review = models
        .iter()
        .find(|model| model["slug"].as_str() == Some("codex-auto-review"))
        .expect("auto review model");
    assert_eq!(auto_review["visibility"].as_str(), Some("hide"));
    let gpt_image = models
        .iter()
        .find(|model| model["slug"].as_str() == Some("gpt-image-1"))
        .expect("image model");
    assert_eq!(gpt_image["visibility"].as_str(), Some("hide"));
}

fn assert_codex_models_response(
    body: &[u8],
    expected_slug: &str,
    expect_image_modality: bool,
) -> serde_json::Value {
    let value: serde_json::Value = serde_json::from_slice(body).expect("json body");
    assert!(value.get("data").is_none());
    let models = value["models"].as_array().expect("models array");
    let model = models
        .iter()
        .find(|model| model["slug"].as_str() == Some(expected_slug))
        .expect("expected model");
    assert_eq!(model["visibility"].as_str(), Some("list"));
    let modalities = model["input_modalities"]
        .as_array()
        .expect("input_modalities array");
    assert_eq!(
        modalities
            .iter()
            .any(|modality| modality.as_str() == Some("image")),
        expect_image_modality
    );
    value
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
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "probe-primary".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "probe-backup".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "probe-primary".to_string(),
                "probe-backup".to_string(),
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
            crate::runtime_identity::ProviderEndpointKey::new("codex", "probe-primary", "default"),
            30,
            crate::lb::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;
    state
        .penalize_provider_endpoint_attempt(
            "codex",
            crate::runtime_identity::ProviderEndpointKey::new("codex", "probe-backup", "default"),
            30,
            crate::lb::CooldownBackoff {
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
        attempt
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("cooldown"))
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
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([(
                "probe-primary".to_string(),
                ProviderConfigV4 {
                    base_url: Some(format!("http://{primary_addr}/v1")),
                    inline_auth: UpstreamAuth::default(),
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "probe-primary".to_string(),
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
            crate::runtime_identity::ProviderEndpointKey::new("codex", "probe-primary", "default"),
            30,
            crate::lb::CooldownBackoff {
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
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
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
    let body = resp.text().await.expect("body should remain readable");
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(body.contains(r#""code":"upstream_error""#), "{body}");
    assert!(
        body.contains(r#""codex_helper_error":"upstream_stream_error""#),
        "{body}"
    );
    assert!(body.contains("Upstream stream failed:"), "{body}");

    proxy_handle.abort();
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
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);
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
    assert_eq!(finished.status_code, StatusCode::OK.as_u16());
    assert!(finished.streaming);
    assert!(finished.usage.is_none());
    assert!(
        finished.duration_ms < 3_000,
        "duration should be bounded by idle watchdog: {finished:?}"
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
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "failing".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{failing_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "cooldown".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{cooldown_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "failing".to_string(),
                "cooldown".to_string(),
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
            crate::runtime_identity::ProviderEndpointKey::new("codex", "cooldown", "default"),
            30,
            crate::lb::CooldownBackoff {
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

    let v4 = ProxyConfigV4 {
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "limited-primary".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "limited-backup".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "limited-primary".to_string(),
                "limited-backup".to_string(),
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
        .set_provider_endpoint_usage_exhausted(
            "codex",
            crate::runtime_identity::ProviderEndpointKey::new(
                "codex",
                "limited-primary",
                "default",
            ),
            true,
        )
        .await;
    state
        .set_provider_endpoint_usage_exhausted(
            "codex",
            crate::runtime_identity::ProviderEndpointKey::new("codex", "limited-backup", "default"),
            true,
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
    let body = resp.text().await.expect("body");
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(body.contains(r#""code":"rate_limit_exceeded""#), "{body}");
    assert!(body.contains("try again in 8 seconds"), "{body}");
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
            .map(|attempt| attempt.reason.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("usage_exhausted"), Some("usage_exhausted")]
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
        allow_cross_station_before_first_output: Some(true),
        transport_cooldown_secs: Some(30),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(RoutingConfigV4::ordered_failover(vec![
                "primary".to_string(),
                "backup".to_string(),
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
    let cfg = make_proxy_config(
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
    let cfg = make_proxy_config(
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

    proxy.handle.abort();
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
    let cfg = make_proxy_config(
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
    assert_eq!(first.reason.as_deref(), Some("reasoning_tokens=516"));
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
    let cfg = make_proxy_config(
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
    assert_eq!(first.reason.as_deref(), Some("reasoning_tokens=516"));
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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);

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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);

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
        allow_cross_station_before_first_output: Some(true),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_proxy_config(
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
    let cfg = make_proxy_config(
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
    let cfg = make_proxy_config(
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
    let cfg = make_proxy_config(
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
async fn proxy_no_routable_station_finishes_active_request() {
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

    let cfg = make_proxy_config(
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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);
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
            .route_attempts_or_derived()
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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);
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
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(br#"{"ok":true,"response":{"service_tier":"priority"}}"#)
                .expect("gzip write");
            let compressed = encoder.finish().expect("gzip finish");

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
            response
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let retry = retry_config(1, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);
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
    assert!(
        !resp
            .headers()
            .contains_key(axum::http::header::CONTENT_ENCODING)
    );
    let body = resp.bytes().await.expect("body");
    let value: serde_json::Value = serde_json::from_slice(body.as_ref()).expect("json response");
    assert_eq!(value["ok"].as_bool(), Some(true));
    assert_eq!(value["response"]["service_tier"].as_str(), Some("priority"));

    let finished = find_finished_request(&state, 10, |request| request.path == "/v1/responses")
        .await
        .expect("finished request");
    assert_eq!(finished.service_tier.as_deref(), Some("priority"));
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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);
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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);
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
    let cfg = make_proxy_config(vec![upstream.upstream_config()], retry);
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
