use super::*;
use crate::config::ProviderEndpointConfig;

fn provider_endpoint(base_url: String, supported_model: &str) -> ProviderEndpointConfig {
    ProviderEndpointConfig {
        base_url,
        continuity_domain: None,
        enabled: true,
        priority: 0,
        tags: std::collections::BTreeMap::new(),
        supported_models: std::collections::BTreeMap::from([(supported_model.to_string(), true)]),
        model_mapping: std::collections::BTreeMap::new(),
        limits: ProviderConcurrencyLimits::default(),
    }
}

#[tokio::test]
async fn proxy_routing_explain_returns_selected_route_and_structured_skip_reasons() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let old_url = "http://127.0.0.1:9/v1".to_string();
    let new_url = "http://127.0.0.1:10/v1".to_string();
    let cfg = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "old".to_string(),
                    ProviderConfig {
                        endpoints: std::collections::BTreeMap::from([(
                            "legacy".to_string(),
                            provider_endpoint(old_url.clone(), "gpt-4.1"),
                        )]),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "new".to_string(),
                    ProviderConfig {
                        endpoints: std::collections::BTreeMap::from([(
                            "modern".to_string(),
                            provider_endpoint(new_url.clone(), "gpt-5"),
                        )]),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "old".to_string(),
                "new".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };

    let proxy = ProxyService::new(Client::new(), Arc::new(cfg), "codex");
    proxy
        .state
        .penalize_provider_endpoint_attempt(
            "codex",
            crate::runtime_identity::ProviderEndpointKey::new("codex", "old", "legacy"),
            30,
            crate::endpoint_health::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;

    let explain = proxy
        .routing_explain(
            crate::routing_ir::RouteRequestContext {
                model: Some("gpt-5".to_string()),
                service_tier: Some("priority".to_string()),
                reasoning_effort: Some("high".to_string()),
                path: Some("/v1/chat/completions".to_string()),
                method: Some("POST".to_string()),
                headers: std::collections::BTreeMap::from([(
                    "X-Plan".to_string(),
                    "gold".to_string(),
                )]),
            },
            Some("sid-route".to_string()),
        )
        .await
        .expect("build routing explain");
    let explain = serde_json::to_value(explain).expect("serialize routing explain");

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
        explain["selected_route"]["provider_endpoint_key"].as_str(),
        Some("codex/new/modern")
    );
    assert_eq!(explain["affinity_policy"].as_str(), Some("fallback_sticky"));
    assert_eq!(
        explain["selected_route"]["preference_group"].as_u64(),
        Some(1)
    );
    assert_eq!(
        explain["selected_route"]["route_path"]
            .as_array()
            .map(|items| items
                .iter()
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()),
        Some(vec!["main", "new"])
    );

    let first = &explain["candidates"][0];
    assert_eq!(first["provider_id"].as_str(), Some("old"));
    assert_eq!(
        first["provider_endpoint_key"].as_str(),
        Some("codex/old/legacy")
    );
    assert_eq!(first["preference_group"].as_u64(), Some(0));
    assert_eq!(first["selected"].as_bool(), Some(false));
    assert_eq!(
        first["skip_reasons"].as_array().map(|reasons| reasons
            .iter()
            .filter_map(|reason| reason.get("code").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()),
        Some(vec!["unsupported_model", "cooldown"])
    );
    assert_eq!(
        first["skip_reasons"][0]["requested_model"].as_str(),
        Some("gpt-5")
    );
    assert_eq!(
        first["availability"]["failure_count"].as_u64(),
        Some(crate::endpoint_health::FAILURE_THRESHOLD as u64)
    );
    assert_eq!(
        first["availability"]["cooldown_active"].as_bool(),
        Some(true)
    );
    assert_eq!(explain["candidates"][1]["selected"].as_bool(), Some(true));
    assert_eq!(
        explain["candidates"][1]["skip_reasons"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
}

#[tokio::test]
async fn proxy_routing_explain_uses_session_affinity() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let old_url = "http://127.0.0.1:9/v1".to_string();
    let new_url = "http://127.0.0.1:10/v1".to_string();
    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "old".to_string(),
                    ProviderConfig {
                        endpoints: std::collections::BTreeMap::from([(
                            "legacy".to_string(),
                            provider_endpoint(old_url.clone(), "gpt-5"),
                        )]),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "new".to_string(),
                    ProviderConfig {
                        endpoints: std::collections::BTreeMap::from([(
                            "modern".to_string(),
                            provider_endpoint(new_url.clone(), "gpt-5"),
                        )]),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "old".to_string(),
                "new".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source.clone()), "codex");
    let request = crate::routing_ir::RouteRequestContext {
        model: Some("gpt-5".to_string()),
        service_tier: None,
        reasoning_effort: None,
        path: Some("/v1/chat/completions".to_string()),
        method: Some("POST".to_string()),
        headers: Default::default(),
    };
    let template = crate::routing_ir::compile_route_plan_template_with_request(
        "codex",
        &source.codex,
        &request,
    )
    .expect("compile route plan");
    let target_candidate = template
        .candidates
        .iter()
        .find(|candidate| candidate.provider_id == "new")
        .expect("candidate for new provider");
    proxy
        .state
        .record_session_route_affinity_success(
            "sid-route",
            crate::state::SessionRouteAffinityTarget {
                route_graph_key: template.route_graph_key(),
                session_identity_source: Some(crate::state::SessionIdentitySource::Header),
                provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                    "codex", "new", "modern",
                ),
                upstream_base_url: new_url.clone(),
                route_path: target_candidate.route_path.clone(),
            },
            Some("test".to_string()),
            crate::logging::now_ms(),
        )
        .await
        .expect("persist route affinity");

    let explain = proxy
        .routing_explain(request, Some("sid-route".to_string()))
        .await
        .expect("build routing explain");
    let explain = serde_json::to_value(explain).expect("serialize routing explain");

    assert_eq!(
        explain["selected_route"]["provider_endpoint_key"].as_str(),
        Some("codex/new/modern")
    );
    assert_eq!(
        explain["selected_route"]["provider_id"].as_str(),
        Some("new")
    );
    assert_eq!(
        explain["selected_route"]["endpoint_id"].as_str(),
        Some("modern")
    );
}

#[tokio::test]
async fn proxy_routing_explain_uses_provider_endpoint_runtime_health_for_routes() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "monthly".to_string(),
                    ProviderConfig {
                        base_url: Some("http://127.0.0.1:9/v1".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "chili".to_string(),
                    ProviderConfig {
                        base_url: Some("http://127.0.0.1:10/v1".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "monthly_first".to_string(),
                routes: std::collections::BTreeMap::from([(
                    "monthly_first".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::OrderedFailover,
                        children: vec!["monthly".to_string(), "chili".to_string()],
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    proxy
        .set_provider_automatic_block_for_test(
            crate::runtime_identity::ProviderEndpointKey::new("codex", "monthly", "default"),
            true,
            1,
        )
        .await;
    proxy
        .config
        .publish_provider_policy(proxy.state.capture_provider_policy_snapshot().await)
        .await
        .expect("publish provider policy snapshot");
    let explain = proxy
        .routing_explain(
            crate::routing_ir::RouteRequestContext::default(),
            Some("sid-route".to_string()),
        )
        .await
        .expect("build routing explain");
    let explain = serde_json::to_value(explain).expect("serialize routing explain");

    assert_eq!(
        explain["selected_route"]["provider_endpoint_key"].as_str(),
        Some("codex/chili/default")
    );
    assert_eq!(
        explain["candidates"][0]["provider_endpoint_key"].as_str(),
        Some("codex/monthly/default")
    );
    assert!(
        explain["selected_route"]["compatibility"].is_null(),
        "route graph explain should not synthesize legacy station compatibility"
    );
    assert!(
        explain["candidates"]
            .as_array()
            .expect("candidates")
            .iter()
            .all(|candidate| candidate["compatibility"].is_null()),
        "route graph candidates should use provider_endpoint_key as primary identity"
    );
    assert_eq!(
        explain["candidates"][0]["skip_reasons"]
            .as_array()
            .map(|reasons| reasons
                .iter()
                .filter_map(|reason| reason.get("code").and_then(|value| value.as_str()))
                .collect::<Vec<_>>()),
        Some(vec!["cooldown", "usage_exhausted"])
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["available"].as_bool(),
        Some(false)
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["runtime_available"].as_bool(),
        Some(false)
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["routable_except_usage"].as_bool(),
        Some(false)
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["usage_exhausted"].as_bool(),
        Some(true)
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["dominant_reason"]["code"].as_str(),
        Some("cooldown")
    );
}
