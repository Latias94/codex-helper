use super::*;

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
        allow_cross_station_before_first_output: Some(true),
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
    // Two-layer model: retry the current station/upstream first, then fail over to the next station.
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
        allow_cross_station_before_first_output: Some(true),
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
    // Two-layer model: retry the current station/upstream first, then fail over to the next station.
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
        retry: RetryConfig {
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
            allow_cross_station_before_first_output: Some(true),
            cloudflare_challenge_cooldown_secs: Some(0),
            cloudflare_timeout_cooldown_secs: Some(0),
            transport_cooldown_secs: Some(0),
            cooldown_backoff_factor: Some(1),
            cooldown_backoff_max_secs: Some(0),
            ..Default::default()
        },
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
    // 404 is treated as provider/station-level failure by default (no upstream retries).
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
            "http://{}/__codex_helper/api/v1/runtime/status",
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

    assert_eq!(
        v.get("runtime_source_path").and_then(|x| x.as_str()),
        v.get("config_path").and_then(|x| x.as_str())
    );
    assert!(
        v.get("runtime_source_path")
            .and_then(|x| x.as_str())
            .is_some_and(|value| value.ends_with("config.toml") || value.ends_with("config.json"))
    );

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
