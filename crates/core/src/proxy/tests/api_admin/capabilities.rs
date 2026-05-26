use super::*;

#[tokio::test]
async fn proxy_api_v1_capabilities_and_overrides_work() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

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
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/runtime/shutdown"))
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
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/routing"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/routing/explain"))
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
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/providers/balances/refresh"))
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
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/request-ledger/recent"))
    }));
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/request-ledger/summary"))
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
        caps["surface_capabilities"]["provider_balance_refresh"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["station_specs"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["routing"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["routing_explain"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["station_persisted_settings"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["session_override_reset"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["control_trace"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["request_ledger_recent"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["request_ledger_summary"].as_bool(),
        Some(true)
    );
    assert_eq!(
        caps["surface_capabilities"]["runtime_shutdown"].as_bool(),
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

    let refresh = client
        .post(format!(
            "http://{}/__codex_helper/api/v1/providers/balances/refresh?station_name=test",
            proxy_addr
        ))
        .send()
        .await
        .expect("provider balance refresh send")
        .error_for_status()
        .expect("provider balance refresh status")
        .json::<serde_json::Value>()
        .await
        .expect("provider balance refresh json");
    assert_eq!(
        refresh.get("service_name").and_then(|v| v.as_str()),
        Some("codex")
    );
    assert_eq!(
        refresh["refresh"]["attempted"].as_u64(),
        Some(0),
        "tests should not call real usage provider endpoints"
    );
    assert!(refresh["provider_balances"].as_object().is_some());

    let trace_dir = make_temp_test_dir();
    let trace_path = trace_dir.join("control_trace.jsonl");
    unsafe {
        scoped.set_path("CODEX_HELPER_CONTROL_TRACE_PATH", &trace_path);
    }
    write_text_file(
        &crate::request_ledger::request_log_path(),
        &[
            serde_json::json!({
                "timestamp_ms": 100,
                "request_id": 41,
                "trace_id": "codex-41",
                "service": "codex",
                "method": "POST",
                "path": "/v1/responses",
                "status_code": 200,
                "duration_ms": 900,
                "station_name": "primary",
                "provider_id": "relay",
                "session_id": "sid-a",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 4,
                    "total_tokens": 14
                },
                "retry": {
                    "attempts": 1,
                    "upstream_chain": [
                        "primary:https://relay.example/v1 (idx=0) status=200 class=- model=gpt-5.4-mini"
                    ]
                }
            })
            .to_string(),
            serde_json::json!({
                "timestamp_ms": 200,
                "request_id": 42,
                "trace_id": "codex-42",
                "service": "codex",
                "method": "POST",
                "path": "/responses/compact",
                "status_code": 429,
                "duration_ms": 1500,
                "ttfb_ms": 300,
                "station_name": "backup",
                "provider_id": "fallback",
                "session_id": "sid-b",
                "service_tier": { "actual": "priority" },
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": 50,
                    "total_tokens": 150
                },
                "retry": {
                    "attempts": 2,
                    "upstream_chain": [
                        "primary:https://relay.example/v1 (idx=0) status=429 class=rate_limit model=gpt-5.4",
                        "backup:https://fallback.example/v1 (idx=1) status=200 class=- model=gpt-5.4"
                    ]
                }
            })
            .to_string(),
        ]
        .join("\n"),
    );

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

    let request_ledger = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/request-ledger/recent?limit=40",
            proxy_addr
        ))
        .send()
        .await
        .expect("request ledger send")
        .error_for_status()
        .expect("request ledger status")
        .json::<serde_json::Value>()
        .await
        .expect("request ledger json");
    assert_eq!(request_ledger.as_array().map(|items| items.len()), Some(2));
    assert_eq!(
        request_ledger
            .as_array()
            .and_then(|items| items.first())
            .and_then(|value| value.get("id"))
            .and_then(|value| value.as_u64()),
        Some(42)
    );
    assert_eq!(request_ledger[0]["model"].as_str(), Some("gpt-5.4"));
    assert_eq!(request_ledger[0]["service_tier"].as_str(), Some("priority"));

    let filtered_request_ledger = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/request-ledger/recent?limit=40&station=backup&provider=fallback&model=5.4&path=responses%2Fcompact&fast=true&retried=true&status_min=400&status_max=499",
            proxy_addr
        ))
        .send()
        .await
        .expect("filtered request ledger send")
        .error_for_status()
        .expect("filtered request ledger status")
        .json::<serde_json::Value>()
        .await
        .expect("filtered request ledger json");
    assert_eq!(
        filtered_request_ledger.as_array().map(|items| items.len()),
        Some(1)
    );
    assert_eq!(filtered_request_ledger[0]["id"].as_u64(), Some(42));

    let request_summary = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/request-ledger/summary?limit=10&by=provider&station=backup&provider=fallback&model=5.4&path=responses%2Fcompact&fast=true&retried=true&status_min=400&status_max=499",
            proxy_addr
        ))
        .send()
        .await
        .expect("request summary send")
        .error_for_status()
        .expect("request summary status")
        .json::<serde_json::Value>()
        .await
        .expect("request summary json");
    assert_eq!(request_summary.as_array().map(|items| items.len()), Some(1));
    assert_eq!(request_summary[0]["group_value"].as_str(), Some("fallback"));
    assert_eq!(
        request_summary[0]["aggregate"]["requests"].as_u64(),
        Some(1)
    );
    assert_eq!(
        request_summary[0]["aggregate"]["total_tokens"].as_u64(),
        Some(150)
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

#[tokio::test]
async fn codex_capabilities_api_reports_expected_observed_and_mismatches() {
    let model_hits = Arc::new(AtomicUsize::new(0));
    let responses_hits = Arc::new(AtomicUsize::new(0));
    let compact_hits = Arc::new(AtomicUsize::new(0));

    let model_hits_for_route = model_hits.clone();
    let responses_hits_for_route = responses_hits.clone();
    let compact_hits_for_route = compact_hits.clone();
    let upstream = axum::Router::new()
        .route(
            "/v1/models",
            get(move || {
                let model_hits = model_hits_for_route.clone();
                async move {
                    model_hits.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({
                        "object": "list",
                        "data": [
                            { "id": "gpt-5.5", "object": "model", "display_name": "GPT-5.5" }
                        ]
                    }))
                }
            }),
        )
        .route(
            "/v1/responses",
            post(move || {
                let responses_hits = responses_hits_for_route.clone();
                async move {
                    responses_hits.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": {
                                "type": "invalid_request_error",
                                "message": "Missing required parameter: model"
                            }
                        })),
                    )
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let compact_hits = compact_hits_for_route.clone();
                async move {
                    compact_hits.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": {
                                "code": "compact_not_supported",
                                "message": "compact is not supported"
                            }
                        })),
                    )
                }
            }),
        );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
            tags: {
                let mut t = HashMap::new();
                t.insert("provider_id".to_string(), "relay-a".to_string());
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
            "http://{proxy_addr}/__codex_helper/api/v1/capabilities"
        ))
        .send()
        .await
        .expect("codex capabilities manifest send")
        .error_for_status()
        .expect("codex capabilities manifest status")
        .json::<serde_json::Value>()
        .await
        .expect("codex capabilities manifest json");
    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/codex/relay-capabilities"))
    }));
    assert_eq!(
        caps["surface_capabilities"]["codex_relay_capabilities"].as_bool(),
        Some(true)
    );

    let summary = client
        .get(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/operator/summary"
        ))
        .send()
        .await
        .expect("operator summary send")
        .error_for_status()
        .expect("operator summary status")
        .json::<serde_json::Value>()
        .await
        .expect("operator summary json");
    assert_eq!(
        summary["links"]["codex_relay_capabilities"].as_str(),
        Some("/__codex_helper/api/v1/codex/relay-capabilities")
    );

    let diagnostics = client
        .post(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/codex/relay-capabilities"
        ))
        .json(&serde_json::json!({
            "patch_preset": "official-imagegen",
            "model": "gpt-5.5"
        }))
        .send()
        .await
        .expect("codex capabilities send")
        .error_for_status()
        .expect("codex capabilities status")
        .json::<serde_json::Value>()
        .await
        .expect("codex capabilities json");

    assert_eq!(model_hits.load(Ordering::SeqCst), 1);
    assert_eq!(responses_hits.load(Ordering::SeqCst), 1);
    assert_eq!(compact_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        diagnostics.get("service_name").and_then(|v| v.as_str()),
        Some("codex")
    );
    assert_eq!(
        diagnostics.get("station_name").and_then(|v| v.as_str()),
        Some("test")
    );
    assert_eq!(
        diagnostics.get("upstream_index").and_then(|v| v.as_u64()),
        Some(0)
    );
    assert_eq!(
        diagnostics.get("patch_mode").and_then(|v| v.as_str()),
        Some("official-imagegen-bridge")
    );
    assert_eq!(
        diagnostics
            .get("responses_websocket")
            .and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        diagnostics["observed"]["models"]["response_shape"].as_str(),
        Some("openai_data_list")
    );
    assert_eq!(
        diagnostics["observed"]["models"]["translation_required"].as_bool(),
        Some(true)
    );
    assert_eq!(
        diagnostics["expected"]["remote_compaction_v1"]["support"].as_str(),
        Some("supported")
    );
    assert_eq!(
        diagnostics["expected"]["hosted_image_generation"]["support"].as_str(),
        Some("unknown")
    );
    assert_eq!(
        diagnostics["observed"]["responses"]["support"].as_str(),
        Some("supported")
    );
    assert_eq!(
        diagnostics["observed"]["responses_compact"]["support"].as_str(),
        Some("unsupported")
    );
    assert_eq!(
        diagnostics["recommendation"]["current_patch_mode"].as_str(),
        Some("official-imagegen-bridge")
    );
    assert_eq!(
        diagnostics["recommendation"]["recommended_patch_mode"].as_str(),
        Some("default")
    );
    assert_eq!(
        diagnostics["recommendation"]["changes_current_mode"].as_bool(),
        Some(true)
    );
    assert_eq!(
        diagnostics["recommendation"]["confidence"].as_str(),
        Some("medium")
    );
    assert!(diagnostics["mismatches"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("capability").and_then(|v| v.as_str()) == Some("remote_compaction_v1")
                && item
                    .get("observed")
                    .and_then(|v| v.as_str())
                    .is_some_and(|value| value.contains("unsupported"))
        })
    }));
    assert!(diagnostics["mismatches"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("capability").and_then(|v| v.as_str()) == Some("model_catalog")
                && item
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .is_some_and(|value| {
                        value.contains("helper model translation is disabled by default")
                    })
        })
    }));

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn codex_capabilities_api_uses_current_codex_switch_mode_when_payload_omits_patch_mode() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let codex_home = temp_dir.join(".codex");
    let helper_home = temp_dir.join(".codex-helper");
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", codex_home.as_path());
        scoped.set_path("CODEX_HELPER_HOME", helper_home.as_path());
    }
    write_text_file(
        &codex_home.join("config.toml"),
        r#"
model_provider = "codex_proxy"

[model_providers.codex_proxy]
name = "OpenAI"
base_url = "http://127.0.0.1:3211"
wire_api = "responses"
supports_websockets = true
request_max_retries = 0
"#
        .trim_start(),
    );
    write_text_file(&codex_home.join("auth.json"), "{}");

    let upstream = axum::Router::new()
        .route(
            "/v1/models",
            get(|| async {
                Json(serde_json::json!({
                    "models": [
                        {
                            "slug": "gpt-5.5",
                            "input_modalities": ["text", "image"],
                            "supports_search_tool": true,
                            "apply_patch_tool_type": "freeform",
                            "supports_reasoning_summaries": true
                        }
                    ]
                }))
            }),
        )
        .route(
            "/v1/responses",
            post(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "type": "invalid_request_error",
                            "message": "Missing required parameter: model"
                        }
                    })),
                )
            }),
        )
        .route(
            "/v1/responses/compact",
            post(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "code": "compact_not_supported",
                            "message": "compact is not supported"
                        }
                    })),
                )
            }),
        );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
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

    let diagnostics = reqwest::Client::new()
        .post(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/codex/relay-capabilities"
        ))
        .json(&serde_json::json!({ "model": "gpt-5.5" }))
        .send()
        .await
        .expect("codex capabilities send")
        .error_for_status()
        .expect("codex capabilities status")
        .json::<serde_json::Value>()
        .await
        .expect("codex capabilities json");

    assert_eq!(
        diagnostics.get("patch_mode").and_then(|v| v.as_str()),
        Some("official-imagegen-bridge")
    );
    assert_eq!(
        diagnostics
            .get("responses_websocket")
            .and_then(|v| v.as_bool()),
        Some(true)
    );
    assert_eq!(
        diagnostics["expected"]["responses_websocket"]["support"].as_str(),
        Some("supported")
    );
    assert_eq!(
        diagnostics["expected"]["remote_compaction_v1"]["support"].as_str(),
        Some("supported")
    );
    assert_eq!(
        diagnostics["recommendation"]["current_patch_mode"].as_str(),
        Some("official-imagegen-bridge")
    );
    assert_eq!(
        diagnostics["recommendation"]["recommended_patch_mode"].as_str(),
        Some("imagegen-bridge")
    );

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn codex_live_smoke_api_reports_manifest_and_summary_links() {
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: "http://127.0.0.1:9/v1".to_string(),
            auth: UpstreamAuth::default(),
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
    let client = reqwest::Client::new();

    let caps = client
        .get(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/capabilities"
        ))
        .send()
        .await
        .expect("caps send")
        .error_for_status()
        .expect("caps status")
        .json::<serde_json::Value>()
        .await
        .expect("caps json");

    assert!(caps["endpoints"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item.as_str() == Some("/__codex_helper/api/v1/codex/relay-live-smoke"))
    }));
    assert_eq!(
        caps["surface_capabilities"]["codex_relay_live_smoke"].as_bool(),
        Some(true)
    );

    let summary = client
        .get(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/operator/summary"
        ))
        .send()
        .await
        .expect("summary send")
        .error_for_status()
        .expect("summary status")
        .json::<serde_json::Value>()
        .await
        .expect("summary json");
    assert_eq!(
        summary["links"]["codex_relay_live_smoke"].as_str(),
        Some("/__codex_helper/api/v1/codex/relay-live-smoke")
    );

    proxy_handle.abort();
}

#[tokio::test]
async fn codex_live_smoke_api_rejects_missing_ack_before_upstream_io() {
    let compact_hits = Arc::new(AtomicUsize::new(0));
    let compact_hits_for_route = compact_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses/compact",
        post(move || {
            let compact_hits = compact_hits_for_route.clone();
            async move {
                compact_hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({ "output": [] }))
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
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

    let response = reqwest::Client::new()
        .post(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/codex/relay-live-smoke"
        ))
        .json(&serde_json::json!({ "model": "gpt-5.5" }))
        .send()
        .await
        .expect("live smoke send");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(compact_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn codex_live_smoke_api_runs_compact_live_smoke() {
    let compact_hits = Arc::new(AtomicUsize::new(0));
    let seen_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));

    let compact_hits_for_route = compact_hits.clone();
    let seen_body_for_route = seen_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses/compact",
        post(move |request: Request<Body>| {
            let compact_hits = compact_hits_for_route.clone();
            let seen_body = seen_body_for_route.clone();
            async move {
                compact_hits.fetch_add(1, Ordering::SeqCst);
                let body = to_bytes(request.into_body(), 16 * 1024)
                    .await
                    .expect("body");
                let body: serde_json::Value =
                    serde_json::from_slice(body.as_ref()).expect("json body");
                *seen_body.lock().expect("lock body") = Some(body);
                Json(serde_json::json!({
                    "output": [
                        { "type": "compaction", "encrypted_content": "summary" }
                    ]
                }))
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
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

    let result = reqwest::Client::new()
        .post(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/codex/relay-live-smoke"
        ))
        .json(&serde_json::json!({
            "acknowledgement": crate::proxy::CODEX_RELAY_LIVE_SMOKE_ACK,
            "model": "gpt-5.5"
        }))
        .send()
        .await
        .expect("live smoke send")
        .error_for_status()
        .expect("live smoke status")
        .json::<serde_json::Value>()
        .await
        .expect("live smoke json");

    assert_eq!(compact_hits.load(Ordering::SeqCst), 1);
    assert_eq!(result["api_version"].as_u64(), Some(1));
    assert_eq!(result["service_name"].as_str(), Some("codex"));
    assert_eq!(result["station_name"].as_str(), Some("test"));
    assert_eq!(result["requested_model"].as_str(), Some("gpt-5.5"));
    assert_eq!(result["upstream_model"].as_str(), Some("gpt-5.5"));
    assert_eq!(result["cases"][0].as_str(), Some("responses_compact"));
    assert_eq!(result["results"][0]["outcome"].as_str(), Some("passed"));
    assert_eq!(
        result["results"][0]["response_shape"].as_str(),
        Some("compact_output_compaction_item")
    );
    assert_eq!(
        seen_body
            .lock()
            .expect("lock body")
            .as_ref()
            .and_then(|body| body["model"].as_str()),
        Some("gpt-5.5")
    );

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn codex_live_smoke_api_runs_remote_compaction_v2_live_smoke() {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let seen_beta = Arc::new(std::sync::Mutex::new(None::<String>));

    let hits_for_route = hits.clone();
    let seen_body_for_route = seen_body.clone();
    let seen_beta_for_route = seen_beta.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |request: Request<Body>| {
            let hits = hits_for_route.clone();
            let seen_body = seen_body_for_route.clone();
            let seen_beta = seen_beta_for_route.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                *seen_beta.lock().expect("lock beta") = request
                    .headers()
                    .get("x-codex-beta-features")
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned);
                let body = to_bytes(request.into_body(), 16 * 1024)
                    .await
                    .expect("body");
                let body: serde_json::Value =
                    serde_json::from_slice(body.as_ref()).expect("json body");
                *seen_body.lock().expect("lock body") = Some(body);
                (
                    [(
                        axum::http::header::CONTENT_TYPE,
                        HeaderValue::from_static("text/event-stream"),
                    )],
                    concat!(
                        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"compaction\",\"encrypted_content\":\"summary\"}}\n\n",
                        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_compact_v2\",\"output\":[]}}\n\n",
                        "data: [DONE]\n\n",
                    ),
                )
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let cfg = make_proxy_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
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

    let result = reqwest::Client::new()
        .post(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/codex/relay-live-smoke"
        ))
        .json(&serde_json::json!({
            "acknowledgement": crate::proxy::CODEX_RELAY_LIVE_SMOKE_ACK,
            "model": "gpt-5.5",
            "cases": ["remote_compaction_v2"]
        }))
        .send()
        .await
        .expect("live smoke send")
        .error_for_status()
        .expect("live smoke status")
        .json::<serde_json::Value>()
        .await
        .expect("live smoke json");

    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        seen_beta.lock().expect("lock beta").as_deref(),
        Some("remote_compaction_v2")
    );
    let body = seen_body
        .lock()
        .expect("lock body")
        .clone()
        .expect("captured body");
    assert_eq!(body["stream"].as_bool(), Some(true));
    assert_eq!(
        body["input"]
            .as_array()
            .expect("input")
            .iter()
            .filter(|item| item["type"].as_str() == Some("compaction_trigger"))
            .count(),
        1
    );
    assert_eq!(result["cases"][0].as_str(), Some("remote_compaction_v2"));
    assert_eq!(result["results"][0]["outcome"].as_str(), Some("passed"));
    assert_eq!(
        result["results"][0]["response_shape"].as_str(),
        Some("remote_compaction_v2_compaction_stream")
    );
    assert_eq!(
        result["results"][0]["compaction_output_items_seen"].as_u64(),
        Some(1)
    );
    assert_eq!(
        result["results"][0]["response_completed_seen"].as_bool(),
        Some(true)
    );

    proxy_handle.abort();
    upstream_handle.abort();
}
