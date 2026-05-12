use super::*;

#[tokio::test]
async fn proxy_api_v1_routing_explain_returns_selected_route_and_structured_skip_reasons() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let old_url = "http://127.0.0.1:9/v1".to_string();
    let new_url = "http://127.0.0.1:10/v1".to_string();
    let cfg = make_proxy_config(
        vec![
            UpstreamConfig {
                base_url: old_url.clone(),
                auth: UpstreamAuth::default(),
                tags: HashMap::from([
                    ("provider_id".to_string(), "old".to_string()),
                    ("endpoint_id".to_string(), "legacy".to_string()),
                ]),
                supported_models: HashMap::from([("gpt-4.1".to_string(), true)]),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: new_url.clone(),
                auth: UpstreamAuth::default(),
                tags: HashMap::from([
                    ("provider_id".to_string(), "new".to_string()),
                    ("endpoint_id".to_string(), "modern".to_string()),
                ]),
                supported_models: HashMap::from([("gpt-5".to_string(), true)]),
                model_mapping: HashMap::new(),
            },
        ],
        RetryConfig::default(),
    );

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(cfg),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    {
        let mut guard = proxy.lb_states.lock().expect("lb state lock");
        guard.insert(
            "test".to_string(),
            crate::lb::LbState {
                failure_counts: vec![crate::lb::FAILURE_THRESHOLD, 0],
                cooldown_until: vec![None, None],
                usage_exhausted: vec![false, false],
                last_good_index: None,
                penalty_streak: vec![0, 0],
                upstream_signature: vec![old_url, new_url],
            },
        );
    }

    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let explain = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/routing/explain?model=gpt-5&session=sid-route&service_tier=priority&reasoning_effort=high&path=/v1/chat/completions&method=POST&header=X-Plan%3Dgold",
            proxy_addr
        ))
        .send()
        .await
        .expect("routing explain send")
        .error_for_status()
        .expect("routing explain status")
        .json::<serde_json::Value>()
        .await
        .expect("routing explain json");

    assert_eq!(explain["api_version"].as_u64(), Some(1));
    assert_eq!(explain["service_name"].as_str(), Some("codex"));
    assert_eq!(explain["request_model"].as_str(), Some("gpt-5"));
    assert_eq!(explain["session_id"].as_str(), Some("sid-route"));
    assert_eq!(
        explain["request_context"]["service_tier"].as_str(),
        Some("priority")
    );
    assert_eq!(
        explain["request_context"]["reasoning_effort"].as_str(),
        Some("high")
    );
    assert_eq!(
        explain["request_context"]["headers"]
            .as_array()
            .map(|items| items
                .iter()
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()),
        Some(vec!["X-Plan"])
    );
    assert!(
        !serde_json::to_string(&explain)
            .expect("serialize explain")
            .contains("gold")
    );
    assert_eq!(explain["candidates"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        explain["selected_route"]["provider_id"].as_str(),
        Some("new")
    );
    assert_eq!(
        explain["selected_route"]["endpoint_id"].as_str(),
        Some("modern")
    );
    assert_eq!(
        explain["selected_route"]["route_path"]
            .as_array()
            .map(|items| items
                .iter()
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()),
        Some(vec!["legacy", "test", "new"])
    );
    assert_eq!(
        explain["selected_route"]["compatibility"]["station_name"].as_str(),
        Some("test")
    );
    assert_eq!(
        explain["selected_route"]["compatibility"]["upstream_index"].as_u64(),
        Some(1)
    );
    assert_eq!(
        explain["selected_route"]["station_name"].as_str(),
        Some("test")
    );

    let first = &explain["candidates"][0];
    assert_eq!(first["provider_id"].as_str(), Some("old"));
    assert_eq!(first["selected"].as_bool(), Some(false));
    assert_eq!(
        first["compatibility"]["station_name"].as_str(),
        Some("test")
    );
    assert_eq!(
        first["skip_reasons"].as_array().map(|reasons| reasons
            .iter()
            .filter_map(|reason| reason.get("code").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()),
        Some(vec!["unsupported_model", "breaker_open"])
    );
    assert_eq!(
        first["skip_reasons"][0]["requested_model"].as_str(),
        Some("gpt-5")
    );
    assert_eq!(
        first["skip_reasons"][1]["failure_count"].as_u64(),
        Some(crate::lb::FAILURE_THRESHOLD as u64)
    );
    assert_eq!(explain["candidates"][1]["selected"].as_bool(), Some(true));
    assert_eq!(
        explain["candidates"][1]["skip_reasons"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    proxy_handle.abort();
}
