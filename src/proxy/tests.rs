use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Json;
use axum::body::{Body, Bytes};
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use futures_util::stream;
use reqwest::Client;
use tokio::time::{Duration, sleep};

use crate::config::{
    ProxyConfig, RetryConfig, RetryProfileName, RetryStrategy, ServiceConfig, ServiceConfigManager,
    UiConfig, UpstreamAuth, UpstreamConfig,
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
async fn proxy_api_v1_capabilities_and_overrides_work() {
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
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

    let client = reqwest::Client::new();

    let caps = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/capabilities",
            proxy_addr
        ))
        .send()
        .await
        .expect("caps send")
        .error_for_status()
        .expect("caps status")
        .json::<serde_json::Value>()
        .await
        .expect("caps json");
    assert_eq!(caps.get("api_version").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        caps.get("service_name").and_then(|v| v.as_str()),
        Some("codex")
    );

    let set_global = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/global-config",
            proxy_addr
        ))
        .json(&serde_json::json!({ "config_name": "test" }))
        .send()
        .await
        .expect("set global send");
    assert_eq!(set_global.status(), StatusCode::NO_CONTENT);

    let global = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/global-config",
            proxy_addr
        ))
        .send()
        .await
        .expect("get global send")
        .error_for_status()
        .expect("get global status")
        .json::<serde_json::Value>()
        .await
        .expect("get global json");
    assert_eq!(global.as_str(), Some("test"));

    let set_effort = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/effort",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s1", "effort": "high" }))
        .send()
        .await
        .expect("set effort send");
    assert_eq!(set_effort.status(), StatusCode::NO_CONTENT);

    let set_session_cfg = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/config",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s1", "config_name": "test" }))
        .send()
        .await
        .expect("set session config send");
    assert_eq!(set_session_cfg.status(), StatusCode::NO_CONTENT);

    let effort_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/effort",
            proxy_addr
        ))
        .send()
        .await
        .expect("get effort send")
        .error_for_status()
        .expect("get effort status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get effort json");
    assert_eq!(effort_map.get("s1").map(String::as_str), Some("high"));

    let session_cfg_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/config",
            proxy_addr
        ))
        .send()
        .await
        .expect("get session config send")
        .error_for_status()
        .expect("get session config status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get session config json");
    assert_eq!(session_cfg_map.get("s1").map(String::as_str), Some("test"));

    proxy_handle.abort();
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
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
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
    let retry = RetryConfig {
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::SameUpstream),
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
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
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
    let retry = RetryConfig {
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(vec!["upstream_transport_error".to_string()]),
        strategy: Some(RetryStrategy::Failover),
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
    let retry = RetryConfig {
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(vec!["cloudflare_challenge".to_string()]),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(60),
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
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
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
    let (u_addr, u_handle) = spawn_axum_server(upstream);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
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
    let mut resp_ok: Option<reqwest::Response> = None;
    let mut last_status: Option<StatusCode> = None;
    for _ in 0..5 {
        let resp = client
            .post(format!("http://{}/v1/responses", proxy_addr))
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(r#"{"model":"gpt","input":"hi"}"#)
            .send()
            .await
            .expect("send");
        last_status = Some(resp.status());
        if resp.status() == StatusCode::OK {
            resp_ok = Some(resp);
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    let resp = resp_ok.expect("expected 200 OK from proxy");
    assert_eq!(last_status, Some(StatusCode::OK));
    // Drain the body to completion to ensure the proxy consumes the upstream stream and has a
    // chance to observe late `response.usage` events. Some hyper/reqwest combinations can surface
    // spurious decode errors when the server closes a chunked response; we only care that the
    // proxy recorded the finished request with parsed usage.
    let _ = resp.bytes().await;

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
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
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
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("400-599".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
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
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    // Two-layer model: retry current upstream first, then fail over.
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
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
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("400-599".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
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
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
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
        max_attempts: Some(1),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
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
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
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
    // Two-layer model: retry current config/upstream first, then fail over to next config.
    assert_eq!(level1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(level2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    l1_handle.abort();
    l2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_can_switch_configs_with_same_level() {
    let c1_hits = Arc::new(AtomicUsize::new(0));
    let c2_hits = Arc::new(AtomicUsize::new(0));

    let c1_hits2 = c1_hits.clone();
    let config1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            c1_hits2.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "config1 nope" })),
            )
        }),
    );
    let (c1_addr, c1_handle) = spawn_axum_server(config1);

    let c2_hits2 = c2_hits.clone();
    let config2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            c2_hits2.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let (c2_addr, c2_handle) = spawn_axum_server(config2);

    let retry = RetryConfig {
        max_attempts: Some(2),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some("502".to_string()),
        on_class: Some(Vec::new()),
        strategy: Some(RetryStrategy::Failover),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };

    let mut mgr = ServiceConfigManager {
        active: Some("config-1".to_string()),
        ..Default::default()
    };
    mgr.configs.insert(
        "config-1".to_string(),
        ServiceConfig {
            name: "config-1".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", c1_addr),
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
        "config-2".to_string(),
        ServiceConfig {
            name: "config-2".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", c2_addr),
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
    // Two-layer model: retry current config/upstream first, then fail over to next config.
    assert_eq!(c1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(c2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    c1_handle.abort();
    c2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_can_switch_configs_with_same_level_on_404() {
    let c1_hits = Arc::new(AtomicUsize::new(0));
    let c2_hits = Arc::new(AtomicUsize::new(0));

    let c1_hits2 = c1_hits.clone();
    let config1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            c1_hits2.fetch_add(1, Ordering::SeqCst);
            StatusCode::NOT_FOUND
        }),
    );
    let (c1_addr, c1_handle) = spawn_axum_server(config1);

    let c2_hits2 = c2_hits.clone();
    let config2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            c2_hits2.fetch_add(1, Ordering::SeqCst);
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }),
    );
    let (c2_addr, c2_handle) = spawn_axum_server(config2);

    let cfg = ProxyConfig {
        version: Some(1),
        codex: {
            let mut mgr = ServiceConfigManager {
                active: Some("config1".to_string()),
                ..Default::default()
            };
            mgr.configs.insert(
                "config1".to_string(),
                ServiceConfig {
                    name: "config1".to_string(),
                    alias: None,
                    enabled: true,
                    level: 1,
                    upstreams: vec![UpstreamConfig {
                        base_url: format!("http://{}/v1", c1_addr),
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
                "config2".to_string(),
                ServiceConfig {
                    name: "config2".to_string(),
                    alias: None,
                    enabled: true,
                    level: 1,
                    upstreams: vec![UpstreamConfig {
                        base_url: format!("http://{}/v1", c2_addr),
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
            mgr
        },
        claude: ServiceConfigManager::default(),
        retry: RetryConfig::default(),
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
    // 404 is treated as provider/config-level failure by default (no upstream retries).
    assert_eq!(c1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    c1_handle.abort();
    c2_handle.abort();
}

#[tokio::test]
async fn proxy_runtime_config_reports_resolved_retry_profile() {
    let proxy_client = Client::new();
    let retry = RetryConfig {
        profile: Some(RetryProfileName::CostPrimary),
        ..Default::default()
    };
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:1/v1".to_string(),
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
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let v: serde_json::Value = client
        .get(format!(
            "http://{}/__codex_helper/config/runtime",
            proxy_addr
        ))
        .send()
        .await
        .expect("send")
        .error_for_status()
        .expect("status ok")
        .json()
        .await
        .expect("json");

    let retry = v.get("retry").expect("retry field");
    assert!(
        retry.get("profile").is_none(),
        "runtime endpoint should expose resolved retry config (no profile field)"
    );
    assert!(retry.get("strategy").is_none());
    assert!(retry.get("max_attempts").is_none());
    assert_eq!(
        retry
            .get("upstream")
            .and_then(|x| x.get("strategy"))
            .and_then(|x| x.as_str()),
        Some("same_upstream")
    );
    assert_eq!(
        retry
            .get("provider")
            .and_then(|x| x.get("strategy"))
            .and_then(|x| x.as_str()),
        Some("failover")
    );
    assert_eq!(
        retry
            .get("provider")
            .and_then(|x| x.get("max_attempts"))
            .and_then(|x| x.as_u64()),
        Some(2)
    );
    assert_eq!(
        retry
            .get("cooldown_backoff_factor")
            .and_then(|x| x.as_u64()),
        Some(2)
    );
    assert_eq!(
        retry
            .get("cooldown_backoff_max_secs")
            .and_then(|x| x.as_u64()),
        Some(900)
    );

    proxy_handle.abort();
}
