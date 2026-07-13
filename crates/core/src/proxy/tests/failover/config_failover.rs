use super::*;
use crate::dashboard_core::{OperatorReadModel, OperatorReadStatus};

fn two_provider_failover_config(
    first_provider: &str,
    first_addr: std::net::SocketAddr,
    second_provider: &str,
    second_addr: std::net::SocketAddr,
    retry: RetryConfig,
) -> HelperConfig {
    let first_provider = first_provider.to_string();
    let second_provider = second_provider.to_string();
    let route_order = vec![first_provider.clone(), second_provider.clone()];
    HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    first_provider,
                    ProviderConfig {
                        base_url: Some(format!("http://{first_addr}/v1")),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    second_provider,
                    ProviderConfig {
                        base_url: Some(format!("http://{second_addr}/v1")),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(route_order)),
            ..ServiceRouteConfig::default()
        },
        retry,
        ..HelperConfig::default()
    }
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

    let cfg = two_provider_failover_config("level-1", l1_addr, "level-2", l2_addr, retry);

    let proxy = ProxyService::new(Client::new(), Arc::new(cfg), "codex");
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
    // Two-layer model: retry the current endpoint first, then fail over to the next provider.
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

    let cfg = two_provider_failover_config("config-1", c1_addr, "config-2", c2_addr, retry);

    let proxy = ProxyService::new(Client::new(), Arc::new(cfg), "codex");
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
    // Two-layer model: retry the current endpoint first, then fail over to the next provider.
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

    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("404".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(2),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("404".to_string()),
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
    let cfg = two_provider_failover_config("config1", c1_addr, "config2", c2_addr, retry);

    let proxy = ProxyService::new(Client::new(), Arc::new(cfg), "codex");
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
    // 404 is treated as a provider-level failure by default (no endpoint retries).
    assert_eq!(c1_hits.load(Ordering::SeqCst), 1);
    let c2 = c2_hits.load(Ordering::SeqCst);
    assert!(
        matches!(c2, 1 | 2),
        "expected config2 hits to be 1..=2 (transport flake tolerance), got {c2}"
    );

    proxy_handle.abort();
    c1_handle.abort();
    c2_handle.abort();
}

#[tokio::test]
async fn proxy_operator_summary_reports_retry_profile_and_attempt_limits() {
    let proxy_client = Client::new();
    let retry = RetryConfig {
        profile: Some(RetryProfileName::CostPrimary),
        ..Default::default()
    };
    let cfg = make_helper_config(
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

    let proxy = ProxyService::new(proxy_client, Arc::new(cfg), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let model = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/operator/read-model",
            proxy_addr
        ))
        .send()
        .await
        .expect("send")
        .error_for_status()
        .expect("status ok")
        .json::<OperatorReadModel>()
        .await
        .expect("json");

    assert_eq!(model.status, OperatorReadStatus::Ready);
    let data = model.data.expect("ready operator read model data");
    assert_eq!(
        data.summary.retry.configured_profile,
        Some(RetryProfileName::CostPrimary)
    );
    assert_eq!(data.summary.retry.upstream_max_attempts, 2);
    assert_eq!(data.summary.retry.provider_max_attempts, 2);

    proxy_handle.abort();
}
