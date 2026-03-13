use super::*;

#[tokio::test]
async fn proxy_rejects_incompatible_profile_station_capabilities() {
    let mut cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::from([
                ("supports_fast_mode".to_string(), "false".to_string()),
                ("supports_reasoning".to_string(), "false".to_string()),
            ]),
            supported_models: HashMap::from([("gpt-5.4".to_string(), true)]),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    cfg.codex.profiles.insert(
        "strict".to_string(),
        ServiceControlProfile {
            extends: None,
            station: Some("test".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
        },
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

    let set_default = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": "strict" }))
        .send()
        .await
        .expect("set incompatible default profile send");
    assert_eq!(set_default.status(), StatusCode::BAD_REQUEST);
    let set_default_body = set_default.text().await.expect("set default body");
    assert!(set_default_body.contains("service_tier"));

    let apply_profile = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/profile",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "session_id": "sid-incompatible-profile",
            "profile_name": "strict",
        }))
        .send()
        .await
        .expect("apply incompatible profile send");
    assert_eq!(apply_profile.status(), StatusCode::BAD_REQUEST);
    let apply_profile_body = apply_profile.text().await.expect("apply profile body");
    assert!(apply_profile_body.contains("service_tier"));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_capability_mismatch_fails_over_without_poisoning_health() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let primary_hits_for_route = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_hits = primary_hits_for_route.clone();
            async move {
                primary_hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": {
                            "type": "unsupported_value",
                            "message": "service_tier 'priority' is not supported by this provider"
                        }
                    })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let backup_hits_for_route = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_hits = backup_hits_for_route.clone();
            async move {
                backup_hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "upstream": "backup" })),
                )
            }
        }),
    );
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

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
                base_url: format!("http://{}/v1", primary_addr),
                auth: UpstreamAuth::default(),
                tags: HashMap::from([("provider_id".to_string(), "primary".to_string())]),
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
                base_url: format!("http://{}/v1", backup_addr),
                auth: UpstreamAuth::default(),
                tags: HashMap::from([("provider_id".to_string(), "backup".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
        claude: ServiceConfigManager::default(),
        retry: RetryConfig {
            profile: Some(RetryProfileName::AggressiveFailover),
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
    let proxy_for_state = proxy.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"input":"hi","service_tier":"priority"}"#)
        .send()
        .await
        .expect("send capability mismatch request")
        .error_for_status()
        .expect("capability mismatch final status")
        .json::<serde_json::Value>()
        .await
        .expect("capability mismatch final json");
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("backup")
    );
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    let lb_view = proxy_for_state.state.get_lb_view().await;
    let primary_lb = lb_view.get("primary").expect("primary lb view");
    assert_eq!(primary_lb.upstreams.len(), 1);
    assert_eq!(primary_lb.upstreams[0].failure_count, 0);
    assert_eq!(primary_lb.upstreams[0].cooldown_remaining_secs, None);

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_snapshot_works() {
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
            model_mapping: HashMap::from([("gpt-5.4".to_string(), "gpt-5.4-fast".to_string())]),
        }],
        RetryConfig::default(),
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    proxy
        .state
        .set_session_station_override("sid-1".to_string(), "test".to_string(), 1)
        .await;
    proxy
        .state
        .set_session_service_tier_override("sid-1".to_string(), "priority".to_string(), 1)
        .await;
    let req_id = proxy
        .state
        .begin_request(
            "codex",
            "POST",
            "/v1/responses",
            Some("sid-1".to_string()),
            Some("Frank-Desk".to_string()),
            Some("100.64.0.12".to_string()),
            Some("G:/codes/demo".to_string()),
            Some("gpt-5.4".to_string()),
            Some("medium".to_string()),
            Some("priority".to_string()),
            1,
        )
        .await;
    proxy
        .state
        .update_request_route(
            req_id,
            "test".to_string(),
            Some("u1".to_string()),
            "http://127.0.0.1:9/v1".to_string(),
            None,
        )
        .await;

    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let snap = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/snapshot?recent_limit=10&stats_days=7",
            proxy_addr
        ))
        .send()
        .await
        .expect("snapshot send")
        .error_for_status()
        .expect("snapshot status")
        .json::<serde_json::Value>()
        .await
        .expect("snapshot json");

    assert_eq!(snap.get("api_version").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        snap.get("service_name").and_then(|v| v.as_str()),
        Some("codex")
    );
    assert!(
        snap.get("snapshot").is_some(),
        "should include snapshot object"
    );
    assert!(
        snap.get("stations").is_some(),
        "should include stations list"
    );
    assert_eq!(
        snap.get("configs").is_none(),
        true,
        "snapshot should not expose legacy configs alias"
    );
    assert_eq!(
        snap["snapshot"]["session_cards"][0]["effective_station"]["source"].as_str(),
        Some("session_override")
    );
    assert_eq!(
        snap["snapshot"]["session_cards"][0]["effective_model"]["value"].as_str(),
        Some("gpt-5.4-fast")
    );
    assert_eq!(
        snap["snapshot"]["session_cards"][0]["effective_model"]["source"].as_str(),
        Some("station_mapping")
    );
    assert_eq!(
        snap["snapshot"]["session_cards"][0]["effective_service_tier"]["source"].as_str(),
        Some("session_override")
    );

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_default_profile_binding_applies_to_new_session() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|body: Bytes| async move {
            let json: serde_json::Value =
                serde_json::from_slice(&body).expect("echo upstream json");
            (StatusCode::OK, Json(json))
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let mut cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", upstream_addr),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: {
                let mut t = HashMap::new();
                t.insert("provider_id".to_string(), "u-bind".to_string());
                t
            },
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    cfg.codex.default_profile = Some("daily".to_string());
    cfg.codex.profiles.insert(
        "daily".to_string(),
        ServiceControlProfile {
            extends: None,
            station: Some("test".to_string()),
            model: Some("gpt-5.4-fast".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: Some("priority".to_string()),
        },
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
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("session_id", "sid-bind")
        .body(r#"{"input":"hi"}"#)
        .send()
        .await
        .expect("send bind request")
        .error_for_status()
        .expect("bind request status")
        .json::<serde_json::Value>()
        .await
        .expect("bind request json");

    assert_eq!(
        resp.get("model").and_then(|v| v.as_str()),
        Some("gpt-5.4-fast")
    );
    assert_eq!(
        resp.get("service_tier").and_then(|v| v.as_str()),
        Some("priority")
    );
    assert_eq!(
        resp.get("reasoning")
            .and_then(|v| v.get("effort"))
            .and_then(|v| v.as_str()),
        Some("low")
    );

    let snap = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/snapshot?recent_limit=10&stats_days=7",
            proxy_addr
        ))
        .send()
        .await
        .expect("binding snapshot send")
        .error_for_status()
        .expect("binding snapshot status")
        .json::<serde_json::Value>()
        .await
        .expect("binding snapshot json");

    let card = &snap["snapshot"]["session_cards"][0];
    assert_eq!(
        card.get("binding_profile_name").and_then(|v| v.as_str()),
        Some("daily")
    );
    assert_eq!(
        card.get("binding_continuity_mode").and_then(|v| v.as_str()),
        Some("default_profile")
    );
    assert_eq!(
        card["effective_model"]
            .get("source")
            .and_then(|v| v.as_str()),
        Some("profile_default")
    );
    assert_eq!(
        card["effective_station"]
            .get("value")
            .and_then(|v| v.as_str()),
        Some("test")
    );
    assert_eq!(
        card["effective_station"]
            .get("source")
            .and_then(|v| v.as_str()),
        Some("profile_default")
    );

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_runtime_default_profile_override_applies_to_new_session() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|body: Bytes| async move {
            let json: serde_json::Value =
                serde_json::from_slice(&body).expect("echo upstream json");
            (StatusCode::OK, Json(json))
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let mut cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", upstream_addr),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: {
                let mut t = HashMap::new();
                t.insert("provider_id".to_string(), "u-bind-runtime".to_string());
                t
            },
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    cfg.codex.default_profile = Some("daily".to_string());
    cfg.codex.profiles.insert(
        "daily".to_string(),
        ServiceControlProfile {
            extends: None,
            station: Some("test".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("default".to_string()),
        },
    );
    cfg.codex.profiles.insert(
        "fast".to_string(),
        ServiceControlProfile {
            extends: None,
            station: Some("test".to_string()),
            model: Some("gpt-5.4-fast".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: Some("priority".to_string()),
        },
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
    let set_default = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": "fast" }))
        .send()
        .await
        .expect("set runtime default profile send");
    assert_eq!(set_default.status(), StatusCode::NO_CONTENT);

    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("session_id", "sid-bind-runtime")
        .body(r#"{"input":"hi"}"#)
        .send()
        .await
        .expect("send runtime binding request")
        .error_for_status()
        .expect("runtime binding request status")
        .json::<serde_json::Value>()
        .await
        .expect("runtime binding request json");

    assert_eq!(
        resp.get("model").and_then(|v| v.as_str()),
        Some("gpt-5.4-fast")
    );
    assert_eq!(
        resp.get("service_tier").and_then(|v| v.as_str()),
        Some("priority")
    );
    assert_eq!(
        resp.get("reasoning")
            .and_then(|v| v.get("effort"))
            .and_then(|v| v.as_str()),
        Some("low")
    );

    let snap = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/snapshot?recent_limit=10&stats_days=7",
            proxy_addr
        ))
        .send()
        .await
        .expect("runtime binding snapshot send")
        .error_for_status()
        .expect("runtime binding snapshot status")
        .json::<serde_json::Value>()
        .await
        .expect("runtime binding snapshot json");

    assert_eq!(
        snap.get("default_profile").and_then(|v| v.as_str()),
        Some("fast")
    );
    let card = &snap["snapshot"]["session_cards"][0];
    assert_eq!(
        card.get("binding_profile_name").and_then(|v| v.as_str()),
        Some("fast")
    );
    assert_eq!(
        card["effective_model"]
            .get("value")
            .and_then(|v| v.as_str()),
        Some("gpt-5.4-fast")
    );

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_runtime_config_meta_override_controls_routing() {
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(|| async move {
            (
                StatusCode::OK,
                Json(serde_json::json!({ "upstream": "primary" })),
            )
        }),
    );
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(|| async move {
            (
                StatusCode::OK,
                Json(serde_json::json!({ "upstream": "backup" })),
            )
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let mut mgr = ServiceConfigManager {
        active: None,
        ..Default::default()
    };
    mgr.configs.insert(
        "primary".to_string(),
        ServiceConfig {
            name: "primary".to_string(),
            alias: Some("primary".to_string()),
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", primary_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::from([("provider_id".to_string(), "primary".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "backup".to_string(),
        ServiceConfig {
            name: "backup".to_string(),
            alias: Some("backup".to_string()),
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", backup_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::from([("provider_id".to_string(), "backup".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
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
    let client = reqwest::Client::new();

    let send_request = || async {
        client
            .post(format!("http://{}/v1/responses", proxy_addr))
            .header("content-type", "application/json")
            .body(r#"{"input":"hi"}"#)
            .send()
            .await
            .expect("send request")
            .error_for_status()
            .expect("request status")
            .json::<serde_json::Value>()
            .await
            .expect("request json")
    };

    let resp = send_request().await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("primary")
    );

    let set_disable = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station_name": "primary",
            "enabled": false,
        }))
        .send()
        .await
        .expect("disable primary send");
    assert_eq!(set_disable.status(), StatusCode::NO_CONTENT);

    let configs = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations",
            proxy_addr
        ))
        .send()
        .await
        .expect("get configs send")
        .error_for_status()
        .expect("get configs status")
        .json::<Vec<crate::dashboard_core::StationOption>>()
        .await
        .expect("get configs json");
    let primary_cfg = configs
        .iter()
        .find(|cfg| cfg.name == "primary")
        .expect("primary config");
    assert!(!primary_cfg.enabled);
    assert!(primary_cfg.configured_enabled);
    assert_eq!(primary_cfg.runtime_enabled_override, Some(false));

    let resp = send_request().await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("backup")
    );

    let clear_disable = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station_name": "primary",
            "clear_enabled": true,
        }))
        .send()
        .await
        .expect("clear primary disable send");
    assert_eq!(clear_disable.status(), StatusCode::NO_CONTENT);

    let set_primary_level = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station_name": "primary",
            "level": 10,
        }))
        .send()
        .await
        .expect("set primary level send");
    assert_eq!(set_primary_level.status(), StatusCode::NO_CONTENT);

    let configs = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations",
            proxy_addr
        ))
        .send()
        .await
        .expect("get configs after level send")
        .error_for_status()
        .expect("get configs after level status")
        .json::<Vec<crate::dashboard_core::StationOption>>()
        .await
        .expect("get configs after level json");
    let primary_cfg = configs
        .iter()
        .find(|cfg| cfg.name == "primary")
        .expect("primary config");
    assert_eq!(primary_cfg.level, 10);
    assert_eq!(primary_cfg.configured_level, 1);
    assert_eq!(primary_cfg.runtime_level_override, Some(10));

    let resp = send_request().await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("backup")
    );

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_runtime_config_state_override_controls_routing() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));
    let primary_hits_for_route = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_hits = primary_hits_for_route.clone();
            async move {
                primary_hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "upstream": "primary" })),
                )
            }
        }),
    );
    let backup_hits_for_route = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_hits = backup_hits_for_route.clone();
            async move {
                backup_hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "upstream": "backup" })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let mut mgr = ServiceConfigManager {
        active: None,
        ..Default::default()
    };
    mgr.configs.insert(
        "primary".to_string(),
        ServiceConfig {
            name: "primary".to_string(),
            alias: Some("primary".to_string()),
            enabled: true,
            level: 1,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", primary_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::from([("provider_id".to_string(), "primary".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    mgr.configs.insert(
        "backup".to_string(),
        ServiceConfig {
            name: "backup".to_string(),
            alias: Some("backup".to_string()),
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: format!("http://{}/v1", backup_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::from([("provider_id".to_string(), "backup".to_string())]),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );
    let cfg = ProxyConfig {
        version: Some(1),
        codex: mgr,
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
    let client = reqwest::Client::new();

    let resp = send_responses_json(&client, proxy_addr, None).await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("primary")
    );

    let set_draining = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station_name": "primary",
            "runtime_state": "draining",
        }))
        .send()
        .await
        .expect("set primary draining send");
    assert_eq!(set_draining.status(), StatusCode::NO_CONTENT);

    let configs = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations",
            proxy_addr
        ))
        .send()
        .await
        .expect("get configs after draining send")
        .error_for_status()
        .expect("get configs after draining status")
        .json::<Vec<crate::dashboard_core::StationOption>>()
        .await
        .expect("get configs after draining json");
    let primary_cfg = configs
        .iter()
        .find(|cfg| cfg.name == "primary")
        .expect("primary config after draining");
    assert_eq!(primary_cfg.runtime_state, RuntimeConfigState::Draining);
    assert_eq!(
        primary_cfg.runtime_state_override,
        Some(RuntimeConfigState::Draining)
    );

    let resp = send_responses_json(&client, proxy_addr, None).await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("backup")
    );

    let set_session_cfg = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/station",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "session_id": "sid-runtime-state",
            "station_name": "primary",
        }))
        .send()
        .await
        .expect("set session config override send");
    assert_eq!(set_session_cfg.status(), StatusCode::NO_CONTENT);

    let resp = send_responses_json(&client, proxy_addr, Some("sid-runtime-state")).await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("primary")
    );

    let set_breaker_open = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station_name": "primary",
            "runtime_state": "breaker_open",
        }))
        .send()
        .await
        .expect("set primary breaker open send");
    assert_eq!(set_breaker_open.status(), StatusCode::NO_CONTENT);

    let configs = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations",
            proxy_addr
        ))
        .send()
        .await
        .expect("get configs after breaker open send")
        .error_for_status()
        .expect("get configs after breaker open status")
        .json::<Vec<crate::dashboard_core::StationOption>>()
        .await
        .expect("get configs after breaker open json");
    let primary_cfg = configs
        .iter()
        .find(|cfg| cfg.name == "primary")
        .expect("primary config after breaker open");
    assert_eq!(primary_cfg.runtime_state, RuntimeConfigState::BreakerOpen);
    assert_eq!(
        primary_cfg.runtime_state_override,
        Some(RuntimeConfigState::BreakerOpen)
    );

    let resp = send_responses_json(&client, proxy_addr, None).await;
    assert_eq!(
        resp.get("upstream").and_then(|v| v.as_str()),
        Some("backup")
    );

    let primary_before_blocked = primary_hits.load(Ordering::SeqCst);
    let backup_before_blocked = backup_hits.load(Ordering::SeqCst);
    let blocked = send_responses_request(&client, proxy_addr, Some("sid-runtime-state")).await;
    assert_eq!(blocked.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(primary_hits.load(Ordering::SeqCst), primary_before_blocked);
    assert_eq!(backup_hits.load(Ordering::SeqCst), backup_before_blocked);

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_stations_alias_works() {
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: HashMap::from([("provider_id".to_string(), "u1".to_string())]),
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

    let stations = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations",
            proxy_addr
        ))
        .send()
        .await
        .expect("get stations send")
        .error_for_status()
        .expect("get stations status")
        .json::<Vec<crate::dashboard_core::StationOption>>()
        .await
        .expect("get stations json");
    let primary = stations
        .iter()
        .find(|station| station.name == "test")
        .expect("test station");
    assert!(primary.enabled);
    assert_eq!(primary.runtime_enabled_override, None);

    let set_disable = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/runtime",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station_name": "test",
            "enabled": false,
        }))
        .send()
        .await
        .expect("disable station send");
    assert_eq!(set_disable.status(), StatusCode::NO_CONTENT);

    let stations = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations",
            proxy_addr
        ))
        .send()
        .await
        .expect("get stations after disable send")
        .error_for_status()
        .expect("get stations after disable status")
        .json::<Vec<crate::dashboard_core::StationOption>>()
        .await
        .expect("get stations after disable json");
    let primary = stations
        .iter()
        .find(|station| station.name == "test")
        .expect("test station after disable");
    assert!(!primary.enabled);
    assert_eq!(primary.runtime_enabled_override, Some(false));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_auth_file_cache_refreshes_after_source_change() {
    let _env_lock = env_lock();
    let mut scoped = ScopedEnv::default();
    let base = make_temp_test_dir();
    let codex_home = base.join("codex-home");
    let claude_home = base.join("claude-home");
    let codex_auth = codex_home.join("auth.json");
    let claude_settings = claude_home.join("settings.json");

    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
        scoped.set_path("CLAUDE_HOME", &claude_home);
    }

    write_text_file(&codex_auth, r#"{"OPENAI_API_KEY":"sk-first"}"#);
    write_text_file(
        &claude_settings,
        r#"{"env":{"ANTHROPIC_API_KEY":"claude-first"}}"#,
    );

    assert_eq!(
        super::codex_auth_json_value("OPENAI_API_KEY"),
        Some("sk-first".to_string())
    );
    assert_eq!(
        super::claude_settings_env_value("ANTHROPIC_API_KEY"),
        Some("claude-first".to_string())
    );

    sleep(Duration::from_millis(30)).await;
    write_text_file(&codex_auth, r#"{"OPENAI_API_KEY":"sk-second"}"#);
    write_text_file(
        &claude_settings,
        r#"{"env":{"ANTHROPIC_API_KEY":"claude-second"}}"#,
    );
    sleep(Duration::from_millis(30)).await;

    assert_eq!(
        super::codex_auth_json_value("OPENAI_API_KEY"),
        Some("sk-second".to_string())
    );
    assert_eq!(
        super::claude_settings_env_value("ANTHROPIC_API_KEY"),
        Some("claude-second".to_string())
    );
}

#[tokio::test]
async fn proxy_admin_routes_require_loopback_or_token_for_remote_access() {
    let _env_lock = env_lock();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set(super::ADMIN_TOKEN_ENV_VAR, "remote-secret");
    }

    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
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
        RetryConfig::default(),
    );
    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let remote_addr = std::net::SocketAddr::from(([203, 0, 113, 7], 43123));

    let mut denied_req = Request::builder()
        .uri("/__codex_helper/api/v1/capabilities")
        .body(Body::empty())
        .expect("build denied request");
    denied_req.extensions_mut().insert(ConnectInfo(remote_addr));
    let denied = app
        .clone()
        .oneshot(denied_req)
        .await
        .expect("denied response");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let denied_body = to_bytes(denied.into_body(), usize::MAX)
        .await
        .expect("denied body");
    let denied_text = String::from_utf8_lossy(&denied_body);
    assert!(denied_text.contains(super::ADMIN_TOKEN_HEADER));

    let mut allowed_req = Request::builder()
        .uri("/__codex_helper/api/v1/capabilities")
        .header(super::ADMIN_TOKEN_HEADER, "remote-secret")
        .body(Body::empty())
        .expect("build allowed request");
    allowed_req
        .extensions_mut()
        .insert(ConnectInfo(remote_addr));
    let allowed = app.oneshot(allowed_req).await.expect("allowed response");
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn proxy_split_listeners_isolate_admin_routes_from_proxy_traffic() {
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
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
        RetryConfig::default(),
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let proxy_app = crate::proxy::proxy_only_router(proxy.clone());
    let admin_app = crate::proxy::admin_listener_router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(proxy_app);
    let (admin_addr, admin_handle) = spawn_axum_server(admin_app);

    let client = reqwest::Client::new();

    let proxy_admin = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/capabilities",
            proxy_addr
        ))
        .send()
        .await
        .expect("proxy admin send");
    assert_eq!(proxy_admin.status(), StatusCode::NOT_FOUND);

    let admin_caps = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/capabilities",
            admin_addr
        ))
        .send()
        .await
        .expect("admin caps send")
        .error_for_status()
        .expect("admin caps status")
        .json::<serde_json::Value>()
        .await
        .expect("admin caps json");
    assert_eq!(
        admin_caps.get("service_name").and_then(|v| v.as_str()),
        Some("codex")
    );

    let admin_proxy = client
        .post(format!("http://{}/v1/responses", admin_addr))
        .body("{}")
        .send()
        .await
        .expect("admin proxy send");
    assert_eq!(admin_proxy.status(), StatusCode::NOT_FOUND);

    proxy_handle.abort();
    admin_handle.abort();
}

#[tokio::test]
async fn proxy_only_router_exposes_admin_discovery_document() {
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
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
        RetryConfig::default(),
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::proxy_only_router_with_admin_base_url(
        proxy,
        Some("http://127.0.0.1:4100".to_string()),
    );

    let discovery = app
        .oneshot(
            Request::builder()
                .uri("/.well-known/codex-helper-admin")
                .body(Body::empty())
                .expect("build discovery request"),
        )
        .await
        .expect("discovery response");

    assert_eq!(discovery.status(), StatusCode::OK);
    let body = to_bytes(discovery.into_body(), usize::MAX)
        .await
        .expect("discovery body");
    let json: serde_json::Value = serde_json::from_slice(&body).expect("discovery json");
    assert_eq!(json.get("api_version").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        json.get("service_name").and_then(|v| v.as_str()),
        Some("codex")
    );
    assert_eq!(
        json.get("admin_base_url").and_then(|v| v.as_str()),
        Some("http://127.0.0.1:4100")
    );
}
