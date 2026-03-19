use super::*;

#[tokio::test]
async fn proxy_api_v1_capabilities_and_overrides_work() {
    let _env_lock = env_lock().await;
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
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/operator/summary"))
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
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/stations/active"))
    }));
    assert!(!caps["endpoints"].as_array().is_some_and(|items| {
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
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/providers"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/providers/runtime"))
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
    assert_eq!(
        caps["surface_capabilities"]["snapshot"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["operator_summary"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["providers"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["provider_runtime"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["station_specs"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["station_persisted_settings"].as_bool(),
        Some(true)
    );
    assert!(caps["surface_capabilities"]["station_persisted_config"].is_null());
    assert_eq!(
        caps["surface_capabilities"]["session_override_reset"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["control_trace"].as_bool(),
        Some(true)
    );
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
    assert_eq!(
        profiles["profiles"][0]
            .get("fast_mode")
            .and_then(|v| v.as_bool()),
        Some(true)
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
async fn proxy_api_v1_capabilities_report_remote_enabled_when_admin_token_configured() {
    let _env_lock = env_lock().await;
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
