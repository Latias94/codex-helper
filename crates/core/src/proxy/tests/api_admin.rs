use super::*;

#[tokio::test]
async fn proxy_api_v1_capabilities_and_overrides_work() {
    let _env_lock = env_lock();
    let mut cfg = make_proxy_config(
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
    cfg.codex.profiles.insert(
        "steady".to_string(),
        ServiceControlProfile {
            extends: None,
            station: Some("test".to_string()),
            model: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: Some("default".to_string()),
        },
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let proxy_state = proxy.state.clone();
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
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/profiles"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/stations/runtime"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/retry/config"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/stations"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/stations/runtime"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/stations/config-active"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/stations/{name}"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/stations/specs"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/stations/specs/{name}"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/providers/specs"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/providers/specs/{name}"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/profiles/default"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/profiles/default/persisted"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/profiles/{name}"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/overrides/session/reset"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/control-trace"))
    }));
    let host_local_history = crate::config::codex_sessions_dir().is_dir();
    assert_eq!(
        caps["shared_capabilities"]["session_observability"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["shared_capabilities"]["request_history"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["host_local_capabilities"]["session_history"].as_bool(),
        Some(host_local_history)
    );
    assert_eq!(
        caps["host_local_capabilities"]["transcript_read"].as_bool(),
        Some(host_local_history)
    );
    assert_eq!(
        caps["host_local_capabilities"]["cwd_enrichment"].as_bool(),
        Some(host_local_history)
    );
    assert_eq!(
        caps["remote_admin_access"]["loopback_without_token"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["remote_admin_access"]["remote_requires_token"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["remote_admin_access"]["remote_enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(
        caps["remote_admin_access"]["token_header"].as_str(),
        Some(crate::proxy::ADMIN_TOKEN_HEADER)
    );
    assert_eq!(
        caps["remote_admin_access"]["token_env_var"].as_str(),
        Some(crate::proxy::ADMIN_TOKEN_ENV_VAR)
    );

    let trace_dir = make_temp_test_dir();
    let trace_path = trace_dir.join("control_trace.jsonl");
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_CONTROL_TRACE_PATH", &trace_path);
    }
    std::fs::write(
        &trace_path,
        [
            serde_json::json!({
                "ts_ms": 100,
                "kind": "retry_trace",
                "service": "codex",
                "request_id": 5,
                "event": "attempt_select",
                "payload": {
                    "event": "attempt_select",
                    "station_name": "right"
                }
            })
            .to_string(),
            serde_json::json!({
                "ts_ms": 200,
                "kind": "request_completed",
                "service": "codex",
                "request_id": 5,
                "event": "request_completed",
                "payload": {
                    "method": "POST",
                    "path": "/v1/responses"
                }
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .expect("write control trace");

    let control_trace = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/control-trace?limit=40",
            proxy_addr
        ))
        .send()
        .await
        .expect("control trace send")
        .error_for_status()
        .expect("control trace status")
        .json::<serde_json::Value>()
        .await
        .expect("control trace json");
    assert_eq!(
        control_trace
            .as_array()
            .and_then(|items| items.first())
            .and_then(|value| value.get("ts_ms"))
            .and_then(|value| value.as_u64()),
        Some(200)
    );
    assert_eq!(
        control_trace
            .as_array()
            .and_then(|items| items.first())
            .and_then(|value| value.get("kind"))
            .and_then(|value| value.as_str()),
        Some("request_completed")
    );

    let set_global = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/global-station",
            proxy_addr
        ))
        .json(&serde_json::json!({ "station_name": "test" }))
        .send()
        .await
        .expect("set global send");
    assert_eq!(set_global.status(), StatusCode::NO_CONTENT);

    let global = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/global-station",
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
            "http://{}/__codex_helper/api/v1/overrides/session/station",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s1", "station_name": "test" }))
        .send()
        .await
        .expect("set session config send");
    assert_eq!(set_session_cfg.status(), StatusCode::NO_CONTENT);

    let set_model = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/model",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s1", "model": "gpt-5.4-fast" }))
        .send()
        .await
        .expect("set model send");
    assert_eq!(set_model.status(), StatusCode::NO_CONTENT);

    let set_session_tier = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/service-tier",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s1", "service_tier": "priority" }))
        .send()
        .await
        .expect("set session tier send");
    assert_eq!(set_session_tier.status(), StatusCode::NO_CONTENT);

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
            "http://{}/__codex_helper/api/v1/overrides/session/station",
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

    let model_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/model",
            proxy_addr
        ))
        .send()
        .await
        .expect("get model send")
        .error_for_status()
        .expect("get model status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get model json");
    assert_eq!(
        model_map.get("s1").map(String::as_str),
        Some("gpt-5.4-fast")
    );

    let tier_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/service-tier",
            proxy_addr
        ))
        .send()
        .await
        .expect("get tier send")
        .error_for_status()
        .expect("get tier status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get tier json");
    assert_eq!(tier_map.get("s1").map(String::as_str), Some("priority"));

    let reset_session_overrides = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/reset",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s1" }))
        .send()
        .await
        .expect("reset session overrides send");
    assert_eq!(reset_session_overrides.status(), StatusCode::NO_CONTENT);

    let effort_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/effort",
            proxy_addr
        ))
        .send()
        .await
        .expect("get effort after reset send")
        .error_for_status()
        .expect("get effort after reset status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get effort after reset json");
    assert!(!effort_map.contains_key("s1"));

    let session_cfg_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/station",
            proxy_addr
        ))
        .send()
        .await
        .expect("get session config after reset send")
        .error_for_status()
        .expect("get session config after reset status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get session config after reset json");
    assert!(!session_cfg_map.contains_key("s1"));

    let model_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/model",
            proxy_addr
        ))
        .send()
        .await
        .expect("get model after reset send")
        .error_for_status()
        .expect("get model after reset status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get model after reset json");
    assert!(!model_map.contains_key("s1"));

    let tier_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/service-tier",
            proxy_addr
        ))
        .send()
        .await
        .expect("get tier after reset send")
        .error_for_status()
        .expect("get tier after reset status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get tier after reset json");
    assert!(!tier_map.contains_key("s1"));

    let profiles = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/profiles",
            proxy_addr
        ))
        .send()
        .await
        .expect("get profiles send")
        .error_for_status()
        .expect("get profiles status")
        .json::<serde_json::Value>()
        .await
        .expect("get profiles json");
    assert_eq!(
        profiles.get("default_profile").and_then(|v| v.as_str()),
        Some("fast")
    );
    assert_eq!(
        profiles["profiles"][0]
            .get("service_tier")
            .and_then(|v| v.as_str()),
        Some("priority")
    );

    let set_default = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": "steady" }))
        .send()
        .await
        .expect("set default profile send");
    assert_eq!(set_default.status(), StatusCode::NO_CONTENT);

    let profiles = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/profiles",
            proxy_addr
        ))
        .send()
        .await
        .expect("get profiles after override send")
        .error_for_status()
        .expect("get profiles after override status")
        .json::<serde_json::Value>()
        .await
        .expect("get profiles after override json");
    assert_eq!(
        profiles.get("default_profile").and_then(|v| v.as_str()),
        Some("steady")
    );

    let clear_default = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/profiles/default",
            proxy_addr
        ))
        .json(&serde_json::json!({ "profile_name": null }))
        .send()
        .await
        .expect("clear default profile send");
    assert_eq!(clear_default.status(), StatusCode::NO_CONTENT);

    let profiles = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/profiles",
            proxy_addr
        ))
        .send()
        .await
        .expect("get profiles after clear send")
        .error_for_status()
        .expect("get profiles after clear status")
        .json::<serde_json::Value>()
        .await
        .expect("get profiles after clear json");
    assert_eq!(
        profiles.get("default_profile").and_then(|v| v.as_str()),
        Some("fast")
    );

    let apply_profile = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/profile",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s2", "profile_name": "fast" }))
        .send()
        .await
        .expect("apply profile send");
    assert_eq!(apply_profile.status(), StatusCode::NO_CONTENT);
    let binding = proxy_state
        .get_session_binding("s2")
        .await
        .expect("s2 binding");
    assert_eq!(binding.profile_name.as_deref(), Some("fast"));

    let clear_profile = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/profile",
            proxy_addr
        ))
        .json(&serde_json::json!({ "session_id": "s2", "profile_name": null }))
        .send()
        .await
        .expect("clear profile send");
    assert_eq!(clear_profile.status(), StatusCode::NO_CONTENT);
    assert!(proxy_state.get_session_binding("s2").await.is_none());

    let model_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/model",
            proxy_addr
        ))
        .send()
        .await
        .expect("get model send")
        .error_for_status()
        .expect("get model status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get model json");
    assert!(!model_map.contains_key("s2"));

    let tier_map = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/overrides/session/service-tier",
            proxy_addr
        ))
        .send()
        .await
        .expect("get tier send")
        .error_for_status()
        .expect("get tier status")
        .json::<HashMap<String, String>>()
        .await
        .expect("get tier json");
    assert!(!tier_map.contains_key("s2"));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_profile_config_crud_persists_and_clears_stale_runtime_override() {
    let _env_lock = env_lock();
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
            "station": "test",
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
async fn proxy_api_v1_station_config_crud_persists_active_and_meta() {
    let _env_lock = env_lock();
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
    assert_eq!(update_station.status(), StatusCode::NO_CONTENT);

    let set_active = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/config-active",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station_name": "zeta",
        }))
        .send()
        .await
        .expect("set persisted active station send");
    assert_eq!(set_active.status(), StatusCode::NO_CONTENT);

    let snapshot = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/snapshot",
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
    assert_eq!(
        snapshot
            .get("configured_active_station")
            .and_then(|value| value.as_str()),
        Some("zeta")
    );
    assert_eq!(
        snapshot
            .get("effective_active_station")
            .and_then(|value| value.as_str()),
        Some("zeta")
    );

    let stations = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations",
            proxy_addr
        ))
        .send()
        .await
        .expect("stations send")
        .error_for_status()
        .expect("stations status")
        .json::<Vec<crate::dashboard_core::StationOption>>()
        .await
        .expect("stations json");
    let zeta = stations
        .iter()
        .find(|station| station.name == "zeta")
        .expect("zeta station");
    assert!(!zeta.enabled);
    assert_eq!(zeta.level, 7);
    assert_eq!(zeta.configured_enabled, false);
    assert_eq!(zeta.configured_level, 7);

    let clear_active = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/stations/config-active",
            proxy_addr
        ))
        .json(&serde_json::json!({
            "station_name": serde_json::Value::Null,
        }))
        .send()
        .await
        .expect("clear persisted active station send");
    assert_eq!(clear_active.status(), StatusCode::NO_CONTENT);

    let snapshot = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/snapshot",
            proxy_addr
        ))
        .send()
        .await
        .expect("snapshot after clear send")
        .error_for_status()
        .expect("snapshot after clear status")
        .json::<serde_json::Value>()
        .await
        .expect("snapshot after clear json");
    assert_eq!(
        snapshot
            .get("configured_active_station")
            .and_then(|value| value.as_str()),
        None
    );
    assert_eq!(
        snapshot
            .get("effective_active_station")
            .and_then(|value| value.as_str()),
        Some("test")
    );

    let reloaded_cfg = crate::config::load_config()
        .await
        .expect("reload config from disk after station CRUD");
    assert_eq!(reloaded_cfg.codex.active.as_deref(), None);
    let zeta = reloaded_cfg
        .codex
        .configs
        .get("zeta")
        .expect("zeta config from disk");
    assert!(!zeta.enabled);
    assert_eq!(zeta.level, 7);

    let config_text =
        std::fs::read_to_string(temp_dir.join("config.toml")).expect("read persisted config.toml");
    assert!(config_text.contains("[codex.stations.zeta]"));
    assert!(config_text.contains("enabled = false"));
    assert!(config_text.contains("level = 7"));
    assert!(!config_text.contains("active_station = \"zeta\""));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_retry_config_crud_persists_profile_and_cooldowns() {
    let _env_lock = env_lock();
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
async fn proxy_api_v1_station_specs_crud_persists_members_and_providers() {
    let _env_lock = env_lock();
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
        .expect("get station specs send")
        .error_for_status()
        .expect("get station specs status")
        .json::<serde_json::Value>()
        .await
        .expect("get station specs json");
    assert_eq!(
        initial["stations"]
            .as_array()
            .map(|stations| stations.len()),
        Some(1)
    );
    assert_eq!(
        initial["providers"]
            .as_array()
            .map(|providers| providers.len()),
        Some(1)
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
    assert_eq!(update.status(), StatusCode::NO_CONTENT);

    let after_update = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/stations/specs",
            proxy_addr
        ))
        .send()
        .await
        .expect("get station specs after update send")
        .error_for_status()
        .expect("get station specs after update status")
        .json::<serde_json::Value>()
        .await
        .expect("get station specs after update json");
    let beta = after_update["stations"]
        .as_array()
        .and_then(|stations| {
            stations.iter().find(|station| {
                station.get("name").and_then(|value| value.as_str()) == Some("beta")
            })
        })
        .expect("beta station");
    assert_eq!(
        beta.get("enabled").and_then(|value| value.as_bool()),
        Some(false)
    );
    assert_eq!(beta.get("level").and_then(|value| value.as_u64()), Some(7));
    assert_eq!(
        beta["members"][0]
            .get("provider")
            .and_then(|value| value.as_str()),
        Some("right")
    );
    assert_eq!(
        beta["members"][0]
            .get("endpoint_names")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
            .and_then(|value| value.as_str()),
        Some("hk")
    );

    let delete = client
        .delete(format!(
            "http://{}/__codex_helper/api/v1/stations/specs/beta",
            proxy_addr
        ))
        .send()
        .await
        .expect("delete station spec send");
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);

    let reloaded_cfg = crate::config::load_config()
        .await
        .expect("reload config from disk after station spec CRUD");
    assert!(reloaded_cfg.codex.configs.contains_key("alpha"));
    assert!(!reloaded_cfg.codex.configs.contains_key("beta"));

    let config_text =
        std::fs::read_to_string(temp_dir.join("config.toml")).expect("read persisted config.toml");
    assert!(config_text.contains("[codex.providers.right]"));
    assert!(config_text.contains("[codex.stations.alpha]"));
    assert!(!config_text.contains("[codex.stations.beta]"));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_provider_specs_crud_persists_endpoints_and_env_refs() {
    let _env_lock = env_lock();
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
    let persisted_cfg: ProxyConfigV2 =
        toml::from_str(&persisted_text).expect("parse persisted provider v2 config");
    let alpha = persisted_cfg
        .codex
        .providers
        .get("alpha")
        .expect("alpha provider should still exist");
    assert_eq!(alpha.alias.as_deref(), Some("Relay Alpha"));
    assert!(!alpha.enabled);
    assert_eq!(alpha.auth.auth_token.as_deref(), Some("inline-alpha-token"));
    assert_eq!(alpha.auth.auth_token_env.as_deref(), Some("ALPHA_NEXT_KEY"));
    assert_eq!(alpha.auth.api_key_env.as_deref(), Some("ALPHA_API_KEY"));
    assert_eq!(
        alpha.tags.get("provider_id").map(|value| value.as_str()),
        Some("alpha")
    );
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
    let alpha_default = alpha
        .endpoints
        .get("default")
        .expect("alpha default endpoint should exist");
    assert_eq!(alpha_default.base_url, "https://alpha2.example.com/v1");
    assert!(!alpha_default.enabled);
    let reloaded_cfg = crate::config::load_config()
        .await
        .expect("reload config from disk after provider spec CRUD");
    assert!(reloaded_cfg.codex.configs.contains_key("main"));
    assert!(!reloaded_cfg.codex.configs.contains_key("beta"));
    assert!(persisted_text.contains("[codex.providers.alpha]"));
    assert!(!persisted_text.contains("[codex.providers.beta]"));

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_capabilities_report_remote_enabled_when_admin_token_configured() {
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
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let caps = Client::new()
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
    assert_eq!(
        caps["remote_admin_access"]["loopback_without_token"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["remote_admin_access"]["remote_requires_token"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["remote_admin_access"]["remote_enabled"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["remote_admin_access"]["token_header"].as_str(),
        Some(super::ADMIN_TOKEN_HEADER)
    );
    assert_eq!(
        caps["remote_admin_access"]["token_env_var"].as_str(),
        Some(super::ADMIN_TOKEN_ENV_VAR)
    );

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_sessions_report_client_identity_from_request_context() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": "resp_test",
                    "output": [],
                })),
            )
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
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

    let client = Client::new();
    let response = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session_id", "sid-client")
        .header(super::CLIENT_NAME_HEADER, "Frank-Desk")
        .header("user-agent", "Codex CLI/0.1")
        .body(r#"{"input":"hi"}"#)
        .send()
        .await
        .expect("responses send");
    assert_eq!(response.status(), StatusCode::OK);

    let sessions = client
        .get(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/sessions"
        ))
        .send()
        .await
        .expect("sessions send")
        .error_for_status()
        .expect("sessions status")
        .json::<serde_json::Value>()
        .await
        .expect("sessions json");
    let sessions = sessions.as_array().expect("sessions array");
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0]
            .get("session_id")
            .and_then(|value| value.as_str()),
        Some("sid-client")
    );
    assert_eq!(
        sessions[0]
            .get("last_client_name")
            .and_then(|value| value.as_str()),
        Some("Frank-Desk")
    );
    assert_eq!(
        sessions[0]
            .get("last_client_addr")
            .and_then(|value| value.as_str()),
        Some("127.0.0.1")
    );
    assert_eq!(
        sessions[0]
            .get("observation_scope")
            .and_then(|value| value.as_str()),
        Some("observed_only")
    );

    proxy_handle.abort();
    upstream_handle.abort();
}
