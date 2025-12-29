use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::{Body, Bytes};
use axum::Json;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use futures_util::stream;
use reqwest::Client;
use tokio::time::{Duration, sleep};

use crate::config::{
    ProxyConfig, RetryConfig, ServiceConfig, ServiceConfigManager, UiConfig, UpstreamAuth,
    UpstreamConfig,
};
use crate::proxy::ProxyService;

fn spawn_axum_server(app: axum::Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    listener.set_nonblocking(true).expect("nonblocking");
    let listener = tokio::net::TcpListener::from_std(listener).expect("to tokio listener");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    (addr, handle)
}

fn make_proxy_config(upstreams: Vec<UpstreamConfig>, retry: RetryConfig) -> ProxyConfig {
    let mut mgr = ServiceConfigManager {
        active: Some("test".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "test".to_string(),
        ServiceConfig {
            name: "test".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams,
        },
    );

    ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry,
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    }
}

fn reserve_unused_local_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.local_addr().expect("local_addr")
}

#[tokio::test]
async fn proxy_failover_retries_502_then_uses_second_upstream() {
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
    let retry = RetryConfig {
        max_attempts: 2,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "502".to_string(),
        on_class: Vec::new(),
        cloudflare_challenge_cooldown_secs: 0,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 0,
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
        body.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body}"
    );
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

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
                    .map(|b| Ok::<Bytes, Infallible>(b)),
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
        max_attempts: 1,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "502".to_string(),
        on_class: Vec::new(),
        cloudflare_challenge_cooldown_secs: 0,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 60,
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
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    // First request hits upstream1 and fails.
    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::BAD_GATEWAY);

    // Second request simulates an external retry (Codex/app-level): should immediately fail over
    // to upstream2 thanks to the cooldown applied on the first 502.
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

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

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
                    .map(|b| Ok::<Bytes, Infallible>(b)),
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
    let retry = RetryConfig {
        max_attempts: 1,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "502".to_string(),
        on_class: vec!["upstream_transport_error".to_string()],
        cloudflare_challenge_cooldown_secs: 0,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 60,
    };
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
    assert_eq!(resp1.status(), StatusCode::BAD_GATEWAY);

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
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

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
                    .map(|b| Ok::<Bytes, Infallible>(b)),
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
        max_attempts: 1,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "502".to_string(),
        on_class: vec!["cloudflare_challenge".to_string()],
        cloudflare_challenge_cooldown_secs: 60,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 0,
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
    assert_eq!(resp1.status(), StatusCode::FORBIDDEN);

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::OK);

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
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
                    .map(|b| Ok::<Bytes, Infallible>(b)),
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
        max_attempts: 1,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "".to_string(),
        on_class: Vec::new(),
        cloudflare_challenge_cooldown_secs: 0,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 60,
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
        max_attempts: 5,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "502".to_string(),
        on_class: Vec::new(),
        cloudflare_challenge_cooldown_secs: 0,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 0,
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
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_streaming_parses_usage_even_when_usage_is_late_in_stream() {
    // Large prefix with no `data:` lines: should push the stream well past 1MB without triggering JSON parse.
    // The final `data:` line includes `response.usage`, which codex-helper should still detect.
    let prefix = Bytes::from(format!("event: {}\n\n", "x".repeat(4096)));
    let n = 320usize; // ~1.3MB before usage
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
                let mut items = Vec::with_capacity(n + 1);
                for _ in 0..n {
                    items.push(prefix.clone());
                }
                items.push(usage);
                let s = stream::iter(items.into_iter().map(|b| Ok::<Bytes, Infallible>(b)));
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
    let (u_addr, u_handle) = spawn_axum_server(upstream);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: 1,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "502".to_string(),
        on_class: Vec::new(),
        cloudflare_challenge_cooldown_secs: 0,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 0,
    };
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", u_addr),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
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
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = resp.bytes().await.expect("read bytes");

    let mut finished = Vec::new();
    for _ in 0..50 {
        finished = state.list_recent_finished(10).await;
        if !finished.is_empty() {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !finished.is_empty(),
        "expected finished request to be recorded"
    );
    let u = finished[0].usage.as_ref().expect("usage should be parsed");
    assert_eq!(u.total_tokens, 3);

    proxy_handle.abort();
    u_handle.abort();
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
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: 2,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "502".to_string(),
        on_class: Vec::new(),
        cloudflare_challenge_cooldown_secs: 0,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 0,
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
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
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
    let retry = RetryConfig {
        max_attempts: 1,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "502".to_string(),
        on_class: Vec::new(),
        cloudflare_challenge_cooldown_secs: 0,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 0,
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
                supported_models: {
                    let mut m = HashMap::new();
                    m.insert("other-*".to_string(), true);
                    m
                },
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
                supported_models: {
                    let mut m = HashMap::new();
                    m.insert("gpt-*".to_string(), true);
                    m
                },
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
        .body(r#"{"model":"gpt-4","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 0);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
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
    let (u_addr, u_handle) = spawn_axum_server(upstream);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: 1,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "502".to_string(),
        on_class: Vec::new(),
        cloudflare_challenge_cooldown_secs: 0,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 0,
    };
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", u_addr),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::new(),
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
        }],
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
        .body(r#"{"model":"claude-sonnet-4","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u_handle.abort();
}

#[tokio::test]
async fn proxy_falls_back_to_level_2_config_after_retryable_failure() {
    let level1_hits = Arc::new(AtomicUsize::new(0));
    let level2_hits = Arc::new(AtomicUsize::new(0));

    let l1_hits = level1_hits.clone();
    let level1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            l1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "level1 nope" })),
            )
        }),
    );
    let (l1_addr, l1_handle) = spawn_axum_server(level1);

    let l2_hits = level2_hits.clone();
    let level2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            l2_hits.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let (l2_addr, l2_handle) = spawn_axum_server(level2);

    let retry = RetryConfig {
        max_attempts: 2,
        backoff_ms: 0,
        backoff_max_ms: 0,
        jitter_ms: 0,
        on_status: "502".to_string(),
        on_class: Vec::new(),
        cloudflare_challenge_cooldown_secs: 0,
        cloudflare_timeout_cooldown_secs: 0,
        transport_cooldown_secs: 0,
    };

    let mut mgr = ServiceConfigManager {
        active: Some("level-1".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "level-1".to_string(),
        ServiceConfig {
            name: "level-1".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", l1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "level-2".to_string(),
        ServiceConfig {
            name: "level-2".to_string(),
            alias: None,
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", l2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
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

    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(level1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(level2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    l1_handle.abort();
    l2_handle.abort();
}
