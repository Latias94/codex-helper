use super::*;
use crate::proxy::tests::harness::{
    find_finished_request, post_compact_json, post_responses_json, proxy_service,
    spawn_proxy_service, spawn_test_proxy, spawn_test_upstream,
};
use crate::state::SessionIdentitySource;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Cursor;
use std::io::Write;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[tokio::test]
async fn proxy_decodes_unlabeled_gzip_models_response_before_forwarding() {
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
    assert_codex_models_response(body.as_ref(), "gpt-5.5", true);
    assert_eq!(
        upstream_accept_encoding.lock().expect("lock").as_deref(),
        Some("identity")
    );
}

#[tokio::test]
async fn proxy_decodes_brotli_models_response_before_forwarding() {
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
    assert_codex_models_response(body.as_ref(), "gpt-5.5", true);
    assert_eq!(
        upstream_accept_encoding.lock().expect("lock").as_deref(),
        Some("identity")
    );
}

#[tokio::test]
async fn proxy_translates_openai_models_list_to_codex_models_response() {
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
async fn proxy_forwards_responses_compact_to_upstream_v1_compact_path() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));

    let hits = upstream_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses/compact",
        post(move |body: axum::body::Bytes| async move {
            hits.fetch_add(1, Ordering::SeqCst);
            let value: serde_json::Value =
                serde_json::from_slice(&body).expect("compact body should parse");
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "compact": true,
                    "model": value.get("model").and_then(|model| model.as_str()).unwrap_or("")
                })),
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
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let resp = post_compact_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp
        .json::<serde_json::Value>()
        .await
        .expect("response json");
    assert_eq!(body["compact"], true);
    assert_eq!(body["model"], "gpt-5");
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

    find_finished_request(&state, 10, |request| request.path == "/responses/compact")
        .await
        .expect("expected compact request path to be visible in finished requests");
}

#[tokio::test]
async fn proxy_normalizes_responses_compact_body_before_forwarding() {
    let upstream_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));

    let seen_body = upstream_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses/compact",
        post(move |body: axum::body::Bytes| {
            let seen_body = seen_body.clone();
            async move {
                *seen_body.lock().expect("body lock") =
                    Some(serde_json::from_slice(&body).expect("compact body should parse"));
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "output": [
                            { "type": "compaction", "encrypted_content": "summary" }
                        ]
                    })),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_proxy_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::new();
    let resp = post_compact_json(
        &client,
        &proxy,
        r#"{
            "model":"gpt-5.5",
            "input":[{"type":"message","role":"user","content":"compact me"}],
            "instructions":"compact-test",
            "tools":[{"type":"function","name":"shell"}],
            "parallel_tool_calls":true,
            "reasoning":{"effort":"high"},
            "text":{"verbosity":"low"},
            "previous_response_id":"resp_123",
            "store":true,
            "stream":true,
            "service_tier":"flex",
            "prompt_cache_key":"cache_123",
            "include":["reasoning.encrypted_content"]
        }"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = upstream_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("upstream compact body");

    assert_eq!(body["model"].as_str(), Some("gpt-5.5"));
    assert_eq!(body["instructions"].as_str(), Some("compact-test"));
    assert!(body.get("tools").is_some());
    assert_eq!(body["parallel_tool_calls"].as_bool(), Some(true));
    assert_eq!(body["reasoning"]["effort"].as_str(), Some("high"));
    assert_eq!(body["text"]["verbosity"].as_str(), Some("low"));
    assert_eq!(body["previous_response_id"].as_str(), Some("resp_123"));
    assert!(body.get("store").is_none());
    assert!(body.get("stream").is_none());
    assert!(body.get("service_tier").is_none());
    assert!(body.get("prompt_cache_key").is_none());
    assert!(body.get("include").is_none());
}

#[tokio::test]
async fn proxy_request_content_encoding_normalizes_zstd_body_before_forwarding() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let upstream_content_encoding = Arc::new(std::sync::Mutex::new(None::<String>));
    let upstream_body = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));

    let hits = upstream_hits.clone();
    let seen_encoding = upstream_content_encoding.clone();
    let seen_body = upstream_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |headers: axum::http::HeaderMap, body: Bytes| {
            let hits = hits.clone();
            let seen_encoding = seen_encoding.clone();
            let seen_body = seen_body.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                *seen_encoding.lock().expect("lock") = headers
                    .get(axum::http::header::CONTENT_ENCODING)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                *seen_body.lock().expect("lock") = body.to_vec();
                (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_proxy_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = spawn_test_proxy(cfg);

    let body = br#"{"model":"gpt-5","input":"hi"}"#;
    let compressed = zstd::stream::encode_all(Cursor::new(body), 0).expect("zstd encode");
    let resp = reqwest::Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("content-encoding", "zstd")
        .body(compressed)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);
    let upstream_json: serde_json::Value =
        serde_json::from_slice(&upstream_body.lock().expect("lock")).expect("upstream json");
    assert_eq!(
        upstream_json,
        serde_json::json!({ "model": "gpt-5", "input": "hi" })
    );
    assert_eq!(*upstream_content_encoding.lock().expect("lock"), None);
}

#[tokio::test]
async fn proxy_request_content_encoding_passthrough_env_preserves_zstd_body_for_upstream() {
    let _lock = env_lock().await;
    let mut env = ScopedEnv::default();
    unsafe {
        env.set("CODEX_HELPER_REQUEST_BODY_ENCODING", "passthrough");
    }

    let upstream_content_encoding = Arc::new(std::sync::Mutex::new(None::<String>));
    let upstream_body = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));

    let seen_encoding = upstream_content_encoding.clone();
    let seen_body = upstream_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |headers: axum::http::HeaderMap, body: Bytes| {
            let seen_encoding = seen_encoding.clone();
            let seen_body = seen_body.clone();
            async move {
                *seen_encoding.lock().expect("lock") = headers
                    .get(axum::http::header::CONTENT_ENCODING)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                *seen_body.lock().expect("lock") = body.to_vec();
                (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_proxy_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = spawn_test_proxy(cfg);

    let body = br#"{"model":"gpt-5","input":"hi"}"#;
    let compressed = zstd::stream::encode_all(Cursor::new(body), 0).expect("zstd encode");
    let resp = reqwest::Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("content-encoding", "zstd")
        .body(compressed.clone())
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(*upstream_body.lock().expect("lock"), compressed);
    assert_eq!(
        upstream_content_encoding.lock().expect("lock").as_deref(),
        Some("zstd")
    );
}

#[tokio::test]
async fn proxy_request_content_encoding_rejects_corrupt_zstd_body() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));

    let hits = upstream_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_proxy_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = spawn_test_proxy(cfg);

    let resp = reqwest::Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("content-encoding", "zstd")
        .body("not a zstd frame")
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let text = resp.text().await.expect("text");
    assert!(text.contains("Content-Encoding"));
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn proxy_uses_official_session_id_affinity_for_responses_compact() {
    let a_responses_hits = Arc::new(AtomicUsize::new(0));
    let a_compact_hits = Arc::new(AtomicUsize::new(0));
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));

    let a_responses_counter = a_responses_hits.clone();
    let a_compact_counter = a_compact_hits.clone();
    let upstream_a = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let a_responses_counter = a_responses_counter.clone();
                async move {
                    a_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "a", "err": "quota" })),
                    )
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let a_compact_counter = a_compact_counter.clone();
                async move {
                    a_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "a", "compact": true })),
                    )
                }
            }),
        );
    let (a_addr, a_handle) = spawn_axum_server(upstream_a);

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "b", "compact": true })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["a".to_string(), "b".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::FallbackSticky;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "a".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{a_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
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

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-official")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-official")
        .await
        .expect("route affinity recorded from official session-id");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");
    assert_eq!(
        affinity.session_identity_source,
        Some(SessionIdentitySource::Header)
    );

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-official")
        .body(r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(compact["provider"].as_str(), Some("b"));
    assert_eq!(compact["compact"].as_bool(), Some(true));
    assert_eq!(a_compact_hits.load(Ordering::SeqCst), 0);
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    let recent = state.list_recent_finished(20).await;
    assert!(
        recent.iter().any(|request| {
            request.session_id.as_deref() == Some("sid-official")
                && request.session_identity_source == Some(SessionIdentitySource::Header)
        }),
        "finished requests should preserve official header session source"
    );
    let cards = state.list_session_identity_cards(20).await;
    let card = cards
        .iter()
        .find(|card| card.session_id.as_deref() == Some("sid-official"))
        .expect("session card for official session");
    assert_eq!(
        card.session_identity_source,
        Some(SessionIdentitySource::Header)
    );
    assert_eq!(
        card.route_affinity
            .as_ref()
            .and_then(|affinity| affinity.session_identity_source),
        Some(SessionIdentitySource::Header)
    );
    proxy_handle.abort();
    a_handle.abort();
    b_handle.abort();
}

#[tokio::test]
async fn proxy_pins_responses_compact_to_affinity_under_preferred_group() {
    let a_responses_hits = Arc::new(AtomicUsize::new(0));
    let a_compact_hits = Arc::new(AtomicUsize::new(0));
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));

    let a_responses_counter = a_responses_hits.clone();
    let a_compact_counter = a_compact_hits.clone();
    let upstream_a = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let a_responses_counter = a_responses_counter.clone();
                async move {
                    a_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "a", "err": "quota" })),
                    )
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let a_compact_counter = a_compact_counter.clone();
                async move {
                    a_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "a", "compact": true })),
                    )
                }
            }),
        );
    let (a_addr, a_handle) = spawn_axum_server(upstream_a);

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "b", "compact": true })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["a".to_string(), "b".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::PreferredGroup;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "a".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{a_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
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

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-preferred-group")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-preferred-group")
        .await
        .expect("route affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-preferred-group")
        .body(r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(compact["provider"].as_str(), Some("b"));
    assert_eq!(a_compact_hits.load(Ordering::SeqCst), 0);
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    a_handle.abort();
    b_handle.abort();
}

#[tokio::test]
async fn proxy_does_not_provider_failover_responses_compact_after_affinity_failure() {
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));
    let c_compact_hits = Arc::new(AtomicUsize::new(0));

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "b", "err": "compact failed" })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_compact_counter = c_compact_hits.clone();
    let upstream_c =
        axum::Router::new()
            .route(
                "/v1/responses",
                post(move || async move {
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "c" })))
                }),
            )
            .route(
                "/v1/responses/compact",
                post(move || {
                    let c_compact_counter = c_compact_counter.clone();
                    async move {
                        c_compact_counter.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({ "provider": "c", "compact": true })),
                        )
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
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::PreferredGroup;
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

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-failure")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-compact-failure")
        .await
        .expect("route affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-failure")
        .body(r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact");
    assert_eq!(compact.status(), StatusCode::BAD_GATEWAY);
    let body = compact.text().await.expect("compact text");
    assert!(
        body.contains("compact failed"),
        "expected affine provider error body, got: {body}"
    );
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c_compact_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
}

#[tokio::test]
async fn proxy_uses_prompt_cache_key_affinity_when_session_headers_are_absent() {
    let a_responses_hits = Arc::new(AtomicUsize::new(0));
    let a_compact_hits = Arc::new(AtomicUsize::new(0));
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));

    let a_responses_counter = a_responses_hits.clone();
    let a_compact_counter = a_compact_hits.clone();
    let upstream_a = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let a_responses_counter = a_responses_counter.clone();
                async move {
                    a_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "a", "err": "quota" })),
                    )
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let a_compact_counter = a_compact_counter.clone();
                async move {
                    a_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "a", "compact": true })),
                    )
                }
            }),
        );
    let (a_addr, a_handle) = spawn_axum_server(upstream_a);

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "b", "compact": true })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["a".to_string(), "b".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::FallbackSticky;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "a".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{a_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
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

    let client = reqwest::Client::new();
    let first_body = br#"{"model":"gpt-5","prompt_cache_key":"pcache-affinity","input":"hi"}"#;
    let first_compressed =
        zstd::stream::encode_all(Cursor::new(first_body), 0).expect("zstd encode");
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("content-encoding", "zstd")
        .body(first_compressed)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("pcache-affinity")
        .await
        .expect("route affinity recorded from prompt_cache_key");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");
    assert_eq!(
        affinity.session_identity_source,
        Some(SessionIdentitySource::PromptCacheKey)
    );

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","prompt_cache_key":"pcache-affinity","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(compact["provider"].as_str(), Some("b"));
    assert_eq!(compact["compact"].as_bool(), Some(true));
    assert_eq!(a_compact_hits.load(Ordering::SeqCst), 0);
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    let affinity_after_compact = state
        .get_session_route_affinity("pcache-affinity")
        .await
        .expect("route affinity still keyed by prompt_cache_key after compact");
    assert_eq!(
        affinity_after_compact
            .provider_endpoint
            .provider_id
            .as_str(),
        "b"
    );
    assert_eq!(
        affinity_after_compact.session_identity_source,
        Some(SessionIdentitySource::PromptCacheKey)
    );
    let recent = state.list_recent_finished(20).await;
    assert!(
        recent.iter().any(|request| {
            request.session_id.as_deref() == Some("pcache-affinity")
                && request.session_identity_source == Some(SessionIdentitySource::PromptCacheKey)
        }),
        "finished requests should preserve prompt_cache_key session source"
    );
    let cards = state.list_session_identity_cards(20).await;
    let card = cards
        .iter()
        .find(|card| card.session_id.as_deref() == Some("pcache-affinity"))
        .expect("session card for prompt_cache_key fallback");
    assert_eq!(
        card.session_identity_source,
        Some(SessionIdentitySource::PromptCacheKey)
    );
    assert_eq!(
        card.route_affinity
            .as_ref()
            .and_then(|affinity| affinity.session_identity_source),
        Some(SessionIdentitySource::PromptCacheKey)
    );

    proxy_handle.abort();
    a_handle.abort();
    b_handle.abort();
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
            r#"{"type":"response.create","model":"gpt-5","stream":true}"#.into(),
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

    let body = seen_first_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("upstream first body");
    assert_eq!(body["type"].as_str(), Some("response.create"));
    assert_eq!(body["model"].as_str(), Some("relay-gpt-5"));

    proxy_handle.abort();
    u_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
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
