use super::*;

#[tokio::test]
async fn proxy_api_v1_profile_config_crud_persists_and_clears_stale_runtime_override() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let mut cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::from([
                ("gpt-5.4".to_string(), true),
                ("gpt-5.4-mini".to_string(), true),
            ]),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    cfg.version = Some(2);
    cfg.codex.default_profile = Some("fast".to_string());
    cfg.codex.profiles.insert(
        "fast".to_string(),
        ServiceControlProfile {
            extends: None,
            station: Some("test".to_string()),
            model: Some("gpt-5.4-mini".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: Some("priority".to_string()),
        },
    );

    let v2 = crate::config::migrate_legacy_to_v2(&cfg);
    crate::config::save_config_v2(&v2)
        .await
        .expect("write initial v2 config");
    let loaded = crate::config::load_config()
        .await
        .expect("load initial runtime config");

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(loaded),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let upsert = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/profiles/steady",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "model": "gpt-5.4",
            "reasoning_effort": "medium",
            "service_tier": "default",
        }))
        .send()
        .await
        .expect("upsert profile send");
    assert_eq!(upsert.status(), StatusCode::OK);
    let upsert_body = upsert
        .json::<serde_json::Value>()
        .await
        .expect("upsert profile json");
    assert_eq!(
        upsert_body
            .get("configured_default_profile")
            .and_then(|value| value.as_str()),
        Some("fast")
    );
    assert!(upsert_body["profiles"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.get("name").and_then(|value| value.as_str()) == Some("steady"))
    }));

    let set_config_default = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default/persisted",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": "steady" }))
        .send()
        .await
        .expect("set persisted default send");
    assert_eq!(set_config_default.status(), StatusCode::OK);
    let set_config_default_body = set_config_default
        .json::<serde_json::Value>()
        .await
        .expect("set persisted default json");
    assert_eq!(
        set_config_default_body
            .get("configured_default_profile")
            .and_then(|value| value.as_str()),
        Some("steady")
    );
    assert_eq!(
        set_config_default_body
            .get("default_profile")
            .and_then(|value| value.as_str()),
        Some("steady")
    );

    let set_runtime_default = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": "fast" }))
        .send()
        .await
        .expect("set runtime default send");
    assert_eq!(set_runtime_default.status(), StatusCode::NO_CONTENT);

    let delete_fast = client
        .delete(format!(
            "http://{}/__codex_helper/api/v1/profiles/fast",
            proxy_addr
        ))
        .send()
        .await
        .expect("delete profile send");
    assert_eq!(delete_fast.status(), StatusCode::OK);
    let delete_body = delete_fast
        .json::<serde_json::Value>()
        .await
        .expect("delete profile json");
    assert_eq!(
        delete_body
            .get("configured_default_profile")
            .and_then(|value| value.as_str()),
        Some("steady")
    );
    assert_eq!(
        delete_body
            .get("default_profile")
            .and_then(|value| value.as_str()),
        Some("steady")
    );
    assert!(delete_body["profiles"].as_array().is_some_and(|items| {
        items.len() == 1 && items[0].get("name").and_then(|value| value.as_str()) == Some("steady")
    }));

    let reloaded_cfg = crate::config::load_config()
        .await
        .expect("reload config from disk after CRUD");
    assert_eq!(
        reloaded_cfg.codex.default_profile.as_deref(),
        Some("steady")
    );
    assert!(reloaded_cfg.codex.profiles.contains_key("steady"));
    assert!(!reloaded_cfg.codex.profiles.contains_key("fast"));
    assert_eq!(
        reloaded_cfg
            .codex
            .profiles
            .get("steady")
            .and_then(|profile| profile.model.as_deref()),
        Some("gpt-5.4")
    );

    let config_text =
        std::fs::read_to_string(temp_dir.join("config.toml")).expect("read persisted config.toml");
    assert!(config_text.contains("[codex.profiles.steady]"));
    assert!(!config_text.contains("[codex.profiles.fast]"));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_station_settings_rejects_persisted_writes_after_v2_auto_migration() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let mut cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    cfg.version = Some(2);
    cfg.codex.configs.insert(
        "zeta".to_string(),
        ServiceConfig {
            name: "zeta".to_string(),
            alias: Some("backup".to_string()),
            enabled: true,
            level: 2,
            upstreams: vec![UpstreamConfig {
                base_url: "http://127.0.0.1:10/v1".to_string(),
                auth: UpstreamAuth::default(),
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            }],
        },
    );

    let v2 = crate::config::migrate_legacy_to_v2(&cfg);
    crate::config::save_config_v2(&v2)
        .await
        .expect("write initial v2 config");
    let loaded = crate::config::load_config()
        .await
        .expect("load initial runtime config");

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(loaded),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let update_station = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/stations/zeta",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "enabled": false,
            "level": 7,
        }))
        .send()
        .await
        .expect("update station send");
    assert_eq!(update_station.status(), StatusCode::BAD_REQUEST);
    let update_station_body = update_station.text().await.expect("update station body");
    assert!(
        update_station_body
            .contains("v4 route graph configs do not support station settings writes"),
        "{update_station_body}"
    );

    let set_active = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/active",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station_name": "zeta",
        }))
        .send()
        .await
        .expect("set persisted active station send");
    assert_eq!(set_active.status(), StatusCode::BAD_REQUEST);
    let set_active_body = set_active.text().await.expect("set active body");
    assert!(
        set_active_body.contains("v4 route graph configs do not support station active writes"),
        "{set_active_body}"
    );

    let config_text =
        std::fs::read_to_string(temp_dir.join("config.toml")).expect("read persisted config.toml");
    assert!(config_text.contains("version = 4"));
    assert!(!config_text.contains("[codex.stations."));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_v4_persisted_control_plane_edits_v4_document() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let mut cfg = crate::config::ProxyConfigV4::default();
    cfg.codex.providers.insert(
        "input".to_string(),
        crate::config::ProviderConfigV4 {
            alias: Some("Input".to_string()),
            enabled: false,
            base_url: Some("https://input.example.com/v1".to_string()),
            inline_auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: Some("INPUT_KEY".to_string()),
                api_key: None,
                api_key_env: None,
            },
            tags: [("billing".to_string(), "monthly".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    );
    cfg.codex.providers.insert(
        "backup".to_string(),
        crate::config::ProviderConfigV4 {
            enabled: true,
            base_url: Some("https://backup.example.com/v1".to_string()),
            inline_auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: Some("BACKUP_KEY".to_string()),
                api_key: None,
                api_key_env: None,
            },
            tags: [("billing".to_string(), "paygo".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    );
    cfg.codex.providers.insert(
        "paygo".to_string(),
        crate::config::ProviderConfigV4 {
            enabled: true,
            base_url: Some("https://paygo.example.com/v1".to_string()),
            inline_auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: Some("PAYGO_KEY".to_string()),
                api_key: None,
                api_key_env: None,
            },
            tags: [("billing".to_string(), "paygo".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    );
    cfg.codex.routing = Some(crate::config::RoutingConfigV4::ordered_failover(vec![
        "input".to_string(),
        "backup".to_string(),
    ]));

    crate::config::save_config_v4(&cfg)
        .await
        .expect("write initial v4 config");
    let loaded = crate::config::load_config()
        .await
        .expect("load initial runtime config");

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(loaded),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let station_specs_rejected = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations/specs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get v4 station specs send");
    assert_eq!(station_specs_rejected.status(), StatusCode::BAD_REQUEST);

    let provider_specs = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/providers/specs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get v4 provider specs send")
        .error_for_status()
        .expect("get v4 provider specs status")
        .json::<serde_json::Value>()
        .await
        .expect("get v4 provider specs json");
    assert_eq!(
        provider_specs["providers"]
            .as_array()
            .map(|providers| providers.len()),
        Some(3)
    );
    let input_spec = provider_specs["providers"]
        .as_array()
        .and_then(|providers| {
            providers.iter().find(|provider| {
                provider.get("name").and_then(|value| value.as_str()) == Some("input")
            })
        })
        .expect("input provider spec");
    assert_eq!(input_spec["tags"]["billing"].as_str(), Some("monthly"));

    let capabilities = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/capabilities",
            proxy_addr
        ))
        .send()
        .await
        .expect("get v4 capabilities send")
        .error_for_status()
        .expect("get v4 capabilities status")
        .json::<serde_json::Value>()
        .await
        .expect("get v4 capabilities json");
    assert_eq!(
        capabilities["surface_capabilities"]["routing"].as_bool(),
        Some(true)
    );
    assert_eq!(
        capabilities["surface_capabilities"]["station_specs"].as_bool(),
        Some(false)
    );
    assert_eq!(
        capabilities["surface_capabilities"]["provider_specs"].as_bool(),
        Some(true)
    );
    assert_eq!(
        capabilities["surface_capabilities"]["station_persisted_settings"].as_bool(),
        Some(false)
    );

    let routing_spec = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/routing",
            proxy_addr
        ))
        .send()
        .await
        .expect("get v4 routing spec send")
        .error_for_status()
        .expect("get v4 routing spec status")
        .json::<serde_json::Value>()
        .await
        .expect("get v4 routing spec json");
    assert_eq!(
        routing_spec["order"].as_array().map(|order| order.len()),
        Some(2)
    );

    let rejected_disabled_target = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/routing",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "policy": "manual-sticky",
            "target": "input",
            "order": ["input", "backup"],
            "on_exhausted": "continue"
        }))
        .send()
        .await
        .expect("set disabled v4 routing target send");
    assert_eq!(rejected_disabled_target.status(), StatusCode::BAD_REQUEST);

    let enable_provider = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/providers/specs/input",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "alias": "Input Relay",
            "enabled": true,
            "auth_token_env": "INPUT_NEXT_KEY",
            "endpoints": [
                {
                    "name": "default",
                    "base_url": "https://input-next.example.com/v1",
                    "enabled": true
                }
            ]
        }))
        .send()
        .await
        .expect("enable v4 provider spec send");
    assert_eq!(enable_provider.status(), StatusCode::NO_CONTENT);
    let after_enable_provider_specs = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/providers/specs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get v4 provider specs after enable send")
        .error_for_status()
        .expect("get v4 provider specs after enable status")
        .json::<serde_json::Value>()
        .await
        .expect("get v4 provider specs after enable json");
    let input_after_enable = after_enable_provider_specs["providers"]
        .as_array()
        .and_then(|providers| {
            providers.iter().find(|provider| {
                provider.get("name").and_then(|value| value.as_str()) == Some("input")
            })
        })
        .expect("input provider after enable");
    assert_eq!(
        input_after_enable["tags"]["billing"].as_str(),
        Some("monthly")
    );

    let set_routing_target = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/routing",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "policy": "manual-sticky",
            "target": "input",
            "order": ["input", "backup"],
            "on_exhausted": "continue"
        }))
        .send()
        .await
        .expect("set v4 routing target send")
        .error_for_status()
        .expect("set v4 routing target status")
        .json::<serde_json::Value>()
        .await
        .expect("set v4 routing target json");
    assert_eq!(set_routing_target["policy"].as_str(), Some("manual-sticky"));
    assert_eq!(set_routing_target["target"].as_str(), Some("input"));

    let set_nested_graph = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/routing",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "entry": "monthly_first",
            "routes": {
                "monthly_pool": {
                    "strategy": "ordered-failover",
                    "children": ["input", "backup"]
                },
                "monthly_first": {
                    "strategy": "ordered-failover",
                    "children": ["monthly_pool", "paygo"]
                }
            }
        }))
        .send()
        .await
        .expect("set v4 routing graph send")
        .error_for_status()
        .expect("set v4 routing graph status")
        .json::<serde_json::Value>()
        .await
        .expect("set v4 routing graph json");
    assert_eq!(set_nested_graph["entry"].as_str(), Some("monthly_first"));
    assert_eq!(
        set_nested_graph["expanded_order"]
            .as_array()
            .map(|order| order
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()),
        Some(vec!["input", "backup", "paygo"])
    );
    assert_eq!(
        set_nested_graph["routes"]["monthly_first"]["children"]
            .as_array()
            .map(|children| children
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<Vec<_>>()),
        Some(vec!["monthly_pool", "paygo"])
    );

    let station_active_rejected = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/active",
            proxy_addr
        ))
        .json(&serde_json::json!({ "station_name": "input" }))
        .send()
        .await
        .expect("set v4 active station send");
    assert_eq!(station_active_rejected.status(), StatusCode::BAD_REQUEST);

    let station_update_rejected = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/stations/input",
            proxy_addr
        ))
        .json(&serde_json::json!({ "enabled": false }))
        .send()
        .await
        .expect("update v4 station send");
    assert_eq!(station_update_rejected.status(), StatusCode::BAD_REQUEST);

    let upsert_provider = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/providers/specs/input",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "alias": "Input Relay",
            "enabled": false,
            "auth_token_env": "INPUT_NEXT_KEY",
            "tags": { "billing": "paygo", "region": "hk" },
            "endpoints": [
                {
                    "name": "default",
                    "base_url": "https://input-next.example.com/v1",
                    "enabled": false
                }
            ]
        }))
        .send()
        .await
        .expect("upsert v4 provider spec send");
    assert_eq!(upsert_provider.status(), StatusCode::NO_CONTENT);

    let upsert_new_provider = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/providers/specs/utility",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "alias": "Utility",
            "enabled": true,
            "auth_token_env": "UTILITY_KEY",
            "endpoints": [
                {
                    "name": "default",
                    "base_url": "https://utility.example.com/v1",
                    "enabled": true
                }
            ]
        }))
        .send()
        .await
        .expect("upsert new v4 provider spec send");
    assert_eq!(upsert_new_provider.status(), StatusCode::NO_CONTENT);

    let station_bound_profile = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/profiles/station-bound",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station": "routing",
            "model": "gpt-5.4"
        }))
        .send()
        .await
        .expect("upsert v4 station-bound profile send");
    assert_eq!(station_bound_profile.status(), StatusCode::BAD_REQUEST);
    let station_bound_profile_body = station_bound_profile
        .text()
        .await
        .expect("station-bound profile error body");
    assert!(
        station_bound_profile_body
            .contains("v4 route graph profiles do not support station bindings"),
        "{station_bound_profile_body}"
    );

    let upsert_profile = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/profiles/daily",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "model": "gpt-5.4",
            "reasoning_effort": "medium"
        }))
        .send()
        .await
        .expect("upsert v4 profile send");
    assert_eq!(upsert_profile.status(), StatusCode::OK);

    let set_default_profile = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default/persisted",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": "daily" }))
        .send()
        .await
        .expect("set v4 default profile send");
    assert_eq!(set_default_profile.status(), StatusCode::OK);

    let config_text =
        std::fs::read_to_string(temp_dir.join("config.toml")).expect("read persisted v4 config");
    assert!(config_text.contains("version = 4"));
    assert!(!config_text.contains("[codex.stations."));
    let persisted_cfg: crate::config::ProxyConfigV4 =
        toml::from_str(&config_text).expect("parse persisted v4 config");
    let routing = persisted_cfg
        .codex
        .routing
        .expect("v4 routing should remain");
    let entry = routing.entry_node().expect("entry route should remain");
    assert_eq!(
        entry.strategy,
        crate::config::RoutingPolicyV4::OrderedFailover
    );
    assert_eq!(entry.target.as_deref(), None);
    assert_eq!(routing.entry, "monthly_first");
    assert_eq!(
        entry.children,
        vec![
            "monthly_pool".to_string(),
            "paygo".to_string(),
            "utility".to_string()
        ]
    );
    assert_eq!(
        routing
            .routes
            .get("monthly_pool")
            .map(|node| node.children.clone()),
        Some(vec!["input".to_string(), "backup".to_string()])
    );
    assert_eq!(
        persisted_cfg.codex.default_profile.as_deref(),
        Some("daily")
    );
    assert!(persisted_cfg.codex.profiles.contains_key("daily"));
    let input = persisted_cfg
        .codex
        .providers
        .get("input")
        .expect("input provider");
    assert_eq!(input.alias.as_deref(), Some("Input Relay"));
    assert!(!input.enabled);
    assert_eq!(
        input.base_url.as_deref(),
        Some("https://input-next.example.com/v1")
    );
    assert_eq!(
        input.inline_auth.auth_token_env.as_deref(),
        Some("INPUT_NEXT_KEY")
    );
    assert_eq!(
        input.tags.get("billing").map(|value| value.as_str()),
        Some("paygo")
    );
    assert_eq!(
        input.tags.get("region").map(|value| value.as_str()),
        Some("hk")
    );

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_retry_config_crud_persists_profile_and_cooldowns() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let mut cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    cfg.version = Some(2);

    let v2 = crate::config::migrate_legacy_to_v2(&cfg);
    crate::config::save_config_v2(&v2)
        .await
        .expect("write initial v2 config");
    let loaded = crate::config::load_config()
        .await
        .expect("load initial runtime config");

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(loaded),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let initial = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/retry/config",
            proxy_addr
        ))
        .send()
        .await
        .expect("get retry config send")
        .error_for_status()
        .expect("get retry config status")
        .json::<serde_json::Value>()
        .await
        .expect("get retry config json");
    assert_eq!(
        initial["configured"]
            .get("profile")
            .and_then(|value| value.as_str()),
        Some("balanced")
    );
    assert_eq!(
        initial["resolved"]
            .get("transport_cooldown_secs")
            .and_then(|value| value.as_u64()),
        Some(30)
    );

    let update = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/retry/config",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "profile": "cost-primary",
            "transport_cooldown_secs": 45,
            "cloudflare_timeout_cooldown_secs": 12,
            "cooldown_backoff_factor": 3,
            "cooldown_backoff_max_secs": 180,
        }))
        .send()
        .await
        .expect("set retry config send");
    assert_eq!(update.status(), StatusCode::OK);
    let update = update
        .json::<serde_json::Value>()
        .await
        .expect("set retry config json");
    assert_eq!(
        update["configured"]
            .get("profile")
            .and_then(|value| value.as_str()),
        Some("cost-primary")
    );
    assert_eq!(
        update["configured"]
            .get("transport_cooldown_secs")
            .and_then(|value| value.as_u64()),
        Some(45)
    );
    assert_eq!(
        update["resolved"]
            .get("cooldown_backoff_factor")
            .and_then(|value| value.as_u64()),
        Some(3)
    );
    assert_eq!(
        update["resolved"]
            .get("cooldown_backoff_max_secs")
            .and_then(|value| value.as_u64()),
        Some(180)
    );

    let reloaded_cfg = crate::config::load_config()
        .await
        .expect("reload config from disk after retry CRUD");
    assert_eq!(
        reloaded_cfg.retry.profile,
        Some(RetryProfileName::CostPrimary)
    );
    assert_eq!(reloaded_cfg.retry.transport_cooldown_secs, Some(45));
    assert_eq!(
        reloaded_cfg.retry.cloudflare_timeout_cooldown_secs,
        Some(12)
    );
    assert_eq!(reloaded_cfg.retry.cooldown_backoff_factor, Some(3));
    assert_eq!(reloaded_cfg.retry.cooldown_backoff_max_secs, Some(180));

    let config_text =
        std::fs::read_to_string(temp_dir.join("config.toml")).expect("read persisted config.toml");
    assert!(config_text.contains("[retry]"));
    assert!(config_text.contains("profile = \"cost-primary\""));
    assert!(config_text.contains("transport_cooldown_secs = 45"));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_station_specs_rejects_crud_after_v2_auto_migration() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let mut cfg = ProxyConfigV2 {
        version: 2,
        codex: ServiceViewV2::default(),
        claude: ServiceViewV2::default(),
        retry: RetryConfig::default(),
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };
    cfg.codex.active_group = Some("alpha".to_string());
    cfg.codex.providers.insert(
        "right".to_string(),
        ProviderConfigV2 {
            alias: Some("Right".to_string()),
            enabled: true,
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: Some("RIGHT_API_KEY".to_string()),
                api_key: None,
                api_key_env: None,
            },
            tags: Default::default(),
            supported_models: Default::default(),
            model_mapping: Default::default(),
            endpoints: [
                (
                    "default".to_string(),
                    ProviderEndpointV2 {
                        base_url: "https://right.example.com/v1".to_string(),
                        enabled: true,
                        priority: 0,
                        tags: Default::default(),
                        supported_models: Default::default(),
                        model_mapping: Default::default(),
                    },
                ),
                (
                    "hk".to_string(),
                    ProviderEndpointV2 {
                        base_url: "https://hk.right.example.com/v1".to_string(),
                        enabled: true,
                        priority: 1,
                        tags: Default::default(),
                        supported_models: Default::default(),
                        model_mapping: Default::default(),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        },
    );
    cfg.codex.groups.insert(
        "alpha".to_string(),
        GroupConfigV2 {
            alias: Some("Alpha".to_string()),
            enabled: true,
            level: 1,
            members: vec![GroupMemberRefV2 {
                provider: "right".to_string(),
                endpoint_names: vec!["default".to_string()],
                preferred: false,
            }],
        },
    );

    crate::config::save_config_v2(&cfg)
        .await
        .expect("write initial v2 config");
    let loaded = crate::config::load_config()
        .await
        .expect("load initial runtime config");

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(loaded),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let initial = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations/specs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get station specs send");
    assert_eq!(initial.status(), StatusCode::BAD_REQUEST);
    let initial_body = initial.text().await.expect("get station specs body");
    assert!(
        initial_body.contains("v4 route graph configs do not expose station specs"),
        "{initial_body}"
    );

    let update = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/stations/specs/beta",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "alias": "Beta",
            "enabled": false,
            "level": 7,
            "members": [
                {
                    "provider": "right",
                    "endpoint_names": ["hk"],
                    "preferred": true
                }
            ]
        }))
        .send()
        .await
        .expect("upsert station spec send");
    assert_eq!(update.status(), StatusCode::BAD_REQUEST);

    let delete = client
        .delete(format!(
            "http://{}/__codex_helper/api/v1/stations/specs/beta",
            proxy_addr
        ))
        .send()
        .await
        .expect("delete station spec send");
    assert_eq!(delete.status(), StatusCode::BAD_REQUEST);

    let config_text =
        std::fs::read_to_string(temp_dir.join("config.toml")).expect("read persisted config.toml");
    assert!(config_text.contains("[codex.providers.right]"));
    assert!(!config_text.contains("[codex.stations."));
    assert!(!config_text.contains("[codex.providers.beta]"));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_provider_specs_crud_persists_endpoints_and_env_refs() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let mut cfg = ProxyConfigV2 {
        version: 2,
        codex: ServiceViewV2::default(),
        claude: ServiceViewV2::default(),
        retry: RetryConfig::default(),
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };
    cfg.codex.providers.insert(
        "alpha".to_string(),
        ProviderConfigV2 {
            alias: Some("Alpha".to_string()),
            enabled: true,
            auth: UpstreamAuth {
                auth_token: Some("inline-alpha-token".to_string()),
                auth_token_env: Some("ALPHA_KEY".to_string()),
                api_key: None,
                api_key_env: None,
            },
            tags: [("provider_id".to_string(), "alpha".to_string())]
                .into_iter()
                .collect(),
            supported_models: [("gpt-5.4".to_string(), true)].into_iter().collect(),
            model_mapping: [("gpt-5.4".to_string(), "gpt-5.4-fast".to_string())]
                .into_iter()
                .collect(),
            endpoints: [(
                "default".to_string(),
                ProviderEndpointV2 {
                    base_url: "https://alpha.example.com/v1".to_string(),
                    enabled: true,
                    priority: 0,
                    tags: [("region".to_string(), "hk".to_string())]
                        .into_iter()
                        .collect(),
                    supported_models: [("gpt-5.4-mini".to_string(), true)].into_iter().collect(),
                    model_mapping: [("gpt-5.4-mini".to_string(), "gpt-5.4".to_string())]
                        .into_iter()
                        .collect(),
                },
            )]
            .into_iter()
            .collect(),
        },
    );
    cfg.codex.groups.insert(
        "main".to_string(),
        GroupConfigV2 {
            alias: None,
            enabled: true,
            level: 1,
            members: vec![GroupMemberRefV2 {
                provider: "alpha".to_string(),
                endpoint_names: vec!["default".to_string()],
                preferred: true,
            }],
        },
    );

    crate::config::save_config_v2(&cfg)
        .await
        .expect("write initial provider v2 config");
    let loaded = crate::config::load_config()
        .await
        .expect("load initial runtime config");

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(loaded),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let initial = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/providers/specs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get provider specs send")
        .error_for_status()
        .expect("get provider specs status")
        .json::<serde_json::Value>()
        .await
        .expect("get provider specs json");
    assert_eq!(
        initial["providers"]
            .as_array()
            .map(|providers| providers.len()),
        Some(1)
    );

    let update_alpha = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/providers/specs/alpha",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "alias": "Relay Alpha",
            "enabled": false,
            "auth_token_env": "ALPHA_NEXT_KEY",
            "api_key_env": "ALPHA_API_KEY",
            "endpoints": [
                {
                    "name": "default",
                    "base_url": "https://alpha2.example.com/v1",
                    "enabled": false
                }
            ]
        }))
        .send()
        .await
        .expect("update alpha provider spec send");
    assert_eq!(update_alpha.status(), StatusCode::NO_CONTENT);

    let update_beta = client
        .put(format!(
            "http://{}/__codex_helper/api/v1/providers/specs/beta",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "alias": "Beta",
            "enabled": false,
            "auth_token_env": "BETA_KEY",
            "api_key_env": "BETA_API_KEY",
            "endpoints": [
                {
                    "name": "hk",
                    "base_url": "https://beta-hk.example.com/v1",
                    "enabled": true
                },
                {
                    "name": "us",
                    "base_url": "https://beta-us.example.com/v1",
                    "enabled": false
                }
            ]
        }))
        .send()
        .await
        .expect("upsert provider spec send");
    assert_eq!(update_beta.status(), StatusCode::NO_CONTENT);

    let after_update = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/providers/specs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get provider specs after update send")
        .error_for_status()
        .expect("get provider specs after update status")
        .json::<serde_json::Value>()
        .await
        .expect("get provider specs after update json");
    let beta = after_update["providers"]
        .as_array()
        .and_then(|providers| {
            providers.iter().find(|provider| {
                provider.get("name").and_then(|value| value.as_str()) == Some("beta")
            })
        })
        .expect("beta provider");
    assert_eq!(
        beta.get("enabled").and_then(|value| value.as_bool()),
        Some(false)
    );
    assert_eq!(
        beta.get("auth_token_env").and_then(|value| value.as_str()),
        Some("BETA_KEY")
    );
    assert_eq!(
        beta["endpoints"]
            .as_array()
            .map(|endpoints| endpoints.len()),
        Some(2)
    );

    let delete = client
        .delete(format!(
            "http://{}/__codex_helper/api/v1/providers/specs/beta",
            proxy_addr
        ))
        .send()
        .await
        .expect("delete provider spec send");
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);

    let persisted_text =
        std::fs::read_to_string(temp_dir.join("config.toml")).expect("read persisted config.toml");
    let persisted_cfg: crate::config::ProxyConfigV4 =
        toml::from_str(&persisted_text).expect("parse persisted provider v4 config");
    let alpha = persisted_cfg
        .codex
        .providers
        .get("alpha")
        .expect("alpha provider should still exist");
    assert_eq!(alpha.alias.as_deref(), Some("Relay Alpha"));
    assert!(!alpha.enabled);
    assert_eq!(
        alpha.inline_auth.auth_token.as_deref(),
        Some("inline-alpha-token")
    );
    assert_eq!(
        alpha.inline_auth.auth_token_env.as_deref(),
        Some("ALPHA_NEXT_KEY")
    );
    assert_eq!(
        alpha.inline_auth.api_key_env.as_deref(),
        Some("ALPHA_API_KEY")
    );
    assert!(!alpha.tags.contains_key("provider_id"));
    assert_eq!(
        alpha.tags.get("region").map(|value| value.as_str()),
        Some("hk")
    );
    assert_eq!(alpha.supported_models.get("gpt-5.4").copied(), Some(true));
    assert_eq!(
        alpha.supported_models.get("gpt-5.4-mini").copied(),
        Some(true)
    );
    assert_eq!(
        alpha
            .model_mapping
            .get("gpt-5.4")
            .map(|value| value.as_str()),
        Some("gpt-5.4-fast")
    );
    assert_eq!(
        alpha
            .model_mapping
            .get("gpt-5.4-mini")
            .map(|value| value.as_str()),
        Some("gpt-5.4")
    );
    assert_eq!(
        alpha.base_url.as_deref(),
        Some("https://alpha2.example.com/v1")
    );
    assert!(alpha.endpoints.is_empty());
    let routing = persisted_cfg
        .codex
        .routing
        .expect("routing should remain after provider CRUD");
    assert!(
        routing
            .routes
            .values()
            .all(|node| !node.children.iter().any(|provider| provider == "beta"))
    );
    let reloaded_cfg = crate::config::load_config()
        .await
        .expect("reload config from disk after provider spec CRUD");
    assert!(reloaded_cfg.codex.configs.contains_key("routing"));
    assert!(!reloaded_cfg.codex.configs.contains_key("beta"));
    assert!(persisted_text.contains("[codex.providers.alpha]"));
    assert!(!persisted_text.contains("[codex.providers.beta]"));

    proxy_handle.abort();
}
