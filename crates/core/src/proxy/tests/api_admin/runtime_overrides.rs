use super::*;
use std::collections::BTreeMap;

use crate::config::ProviderEndpointV4;
use crate::state::FinishRequestParams;

#[tokio::test]
async fn proxy_api_v1_operator_summary_reports_runtime_target_and_retry() {
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
        RetryConfig {
            profile: Some(RetryProfileName::Balanced),
            allow_cross_station_before_first_output: Some(true),
            ..Default::default()
        },
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
    let v2 = crate::config::migrate_legacy_to_v2(&cfg);
    crate::config::save_config_v2(&v2)
        .await
        .expect("write operator summary v2 config");
    let loaded = crate::config::load_config()
        .await
        .expect("load operator summary runtime config");

    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(loaded),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    proxy
        .state
        .set_global_station_override("test".to_string(), 1)
        .await;
    proxy
        .state
        .set_station_runtime_state_override(
            "codex",
            "test".to_string(),
            RuntimeConfigState::Draining,
            2,
        )
        .await;
    proxy
        .state
        .record_station_health(
            "codex",
            "test".to_string(),
            crate::state::StationHealth {
                checked_at_ms: 20,
                upstreams: vec![crate::state::UpstreamHealth {
                    base_url: "http://127.0.0.1:9/v1".to_string(),
                    ok: Some(false),
                    status_code: Some(503),
                    latency_ms: None,
                    error: Some("upstream timed out".to_string()),
                    passive: None,
                }],
            },
        )
        .await;
    proxy
        .state
        .record_passive_upstream_failure(crate::state::PassiveUpstreamFailureRecord {
            service_name: "codex".to_string(),
            station_name: "test".to_string(),
            base_url: "http://127.0.0.1:9/v1".to_string(),
            status_code: Some(503),
            error_class: Some("upstream_transport_error".to_string()),
            error: Some("upstream timed out".to_string()),
            now_ms: 21,
        })
        .await;
    proxy
        .state
        .record_passive_upstream_failure(crate::state::PassiveUpstreamFailureRecord {
            service_name: "codex".to_string(),
            station_name: "test".to_string(),
            base_url: "http://127.0.0.1:9/v1".to_string(),
            status_code: Some(503),
            error_class: Some("upstream_transport_error".to_string()),
            error: Some("upstream timed out".to_string()),
            now_ms: 22,
        })
        .await;
    assert!(
        proxy
            .state
            .try_begin_station_health_check("codex", "test", 1, 23)
            .await
    );
    {
        let mut guard = proxy.lb_states.lock().expect("lb state lock");
        guard.insert(
            "test".to_string(),
            crate::lb::LbState {
                failure_counts: vec![2],
                cooldown_until: vec![Some(
                    std::time::Instant::now() + std::time::Duration::from_secs(30),
                )],
                usage_exhausted: vec![true],
                last_good_index: Some(0),
                penalty_streak: vec![0],
                upstream_signature: vec!["http://127.0.0.1:9/v1".to_string()],
            },
        );
    }
    let _req_id = proxy
        .state
        .begin_request(
            "codex",
            "POST",
            "/v1/responses",
            Some("sid-summary".to_string()),
            Some("Frank-Desk".to_string()),
            Some("100.64.0.12".to_string()),
            Some("G:/codes/demo".to_string()),
            Some("gpt-5.4-mini".to_string()),
            Some("low".to_string()),
            Some("priority".to_string()),
            1,
        )
        .await;
    let recent_same_station = proxy
        .state
        .begin_request(
            "codex",
            "POST",
            "/v1/responses",
            Some("sid-summary".to_string()),
            None,
            None,
            None,
            Some("gpt-5.4-mini".to_string()),
            Some("low".to_string()),
            Some("priority".to_string()),
            10,
        )
        .await;
    proxy
        .state
        .update_request_route(
            recent_same_station,
            "test".to_string(),
            Some("u1".to_string()),
            "http://127.0.0.1:9/v1".to_string(),
            None,
        )
        .await;
    proxy
        .state
        .finish_request(FinishRequestParams {
            id: recent_same_station,
            status_code: 200,
            duration_ms: 1200,
            ended_at_ms: 11,
            observed_service_tier: Some("priority".to_string()),
            usage: None,
            retry: Some(crate::logging::RetryInfo {
                attempts: 2,
                upstream_chain: vec!["test:http://127.0.0.1:9/v1".to_string()],
                route_attempts: Vec::new(),
            }),
            ttfb_ms: Some(180),
            streaming: false,
        })
        .await;
    let recent_cross_station = proxy
        .state
        .begin_request(
            "codex",
            "POST",
            "/v1/responses",
            Some("sid-summary".to_string()),
            None,
            None,
            None,
            Some("gpt-5.4".to_string()),
            Some("medium".to_string()),
            Some("default".to_string()),
            12,
        )
        .await;
    proxy
        .state
        .update_request_route(
            recent_cross_station,
            "test".to_string(),
            Some("u1".to_string()),
            "http://127.0.0.1:9/v1".to_string(),
            None,
        )
        .await;
    proxy
        .state
        .finish_request(FinishRequestParams {
            id: recent_cross_station,
            status_code: 200,
            duration_ms: 1400,
            ended_at_ms: 13,
            observed_service_tier: Some("default".to_string()),
            usage: None,
            retry: Some(crate::logging::RetryInfo {
                attempts: 3,
                upstream_chain: vec![
                    "backup:http://127.0.0.2:9/v1".to_string(),
                    "test:http://127.0.0.1:9/v1".to_string(),
                ],
                route_attempts: Vec::new(),
            }),
            ttfb_ms: Some(200),
            streaming: false,
        })
        .await;
    let recent_fast_mode_only = proxy
        .state
        .begin_request(
            "codex",
            "POST",
            "/v1/responses",
            Some("sid-summary".to_string()),
            None,
            None,
            None,
            Some("gpt-5.4-mini".to_string()),
            Some("low".to_string()),
            Some("priority".to_string()),
            14,
        )
        .await;
    proxy
        .state
        .update_request_route(
            recent_fast_mode_only,
            "test".to_string(),
            Some("u1".to_string()),
            "http://127.0.0.1:9/v1".to_string(),
            None,
        )
        .await;
    proxy
        .state
        .finish_request(FinishRequestParams {
            id: recent_fast_mode_only,
            status_code: 200,
            duration_ms: 800,
            ended_at_ms: 15,
            observed_service_tier: Some("priority".to_string()),
            usage: None,
            retry: None,
            ttfb_ms: Some(120),
            streaming: false,
        })
        .await;

    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let summary = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/operator/summary",
            proxy_addr
        ))
        .send()
        .await
        .expect("summary send")
        .error_for_status()
        .expect("summary status")
        .json::<serde_json::Value>()
        .await
        .expect("summary json");

    assert_eq!(summary["api_version"].as_u64(), Some(1));
    assert_eq!(summary["service_name"].as_str(), Some("codex"));
    assert_eq!(
        summary["runtime"]["configured_active_station"].as_str(),
        Some("test")
    );
    assert_eq!(
        summary["runtime"]["effective_active_station"].as_str(),
        Some("test")
    );
    assert_eq!(
        summary["runtime"]["global_station_override"].as_str(),
        Some("test")
    );
    assert_eq!(
        summary["runtime"]["configured_default_profile"].as_str(),
        Some("fast")
    );
    assert_eq!(summary["runtime"]["default_profile"].as_str(), Some("fast"));
    assert_eq!(
        summary["runtime"]["default_profile_summary"]["name"].as_str(),
        Some("fast")
    );
    assert_eq!(
        summary["runtime"]["default_profile_summary"]["model"].as_str(),
        Some("gpt-5.4-mini")
    );
    assert_eq!(
        summary["runtime"]["default_profile_summary"]["fast_mode"].as_bool(),
        Some(true)
    );
    assert_eq!(
        summary["profiles"]
            .as_array()
            .map(|profiles| profiles.len()),
        Some(1)
    );
    assert_eq!(summary["profiles"][0]["name"].as_str(), Some("fast"));
    assert_eq!(summary["profiles"][0]["fast_mode"].as_bool(), Some(true));
    assert_eq!(
        summary["stations"]
            .as_array()
            .map(|stations| stations.len()),
        Some(1)
    );
    assert_eq!(summary["stations"][0]["name"].as_str(), Some("test"));
    assert_eq!(
        summary["providers"]
            .as_array()
            .map(|providers| providers.len()),
        Some(1)
    );
    assert_eq!(summary["providers"][0]["name"].as_str(), Some("u1"));
    assert_eq!(
        summary["providers"][0]["endpoints"]
            .as_array()
            .map(|endpoints| endpoints.len()),
        Some(1)
    );
    assert_eq!(
        summary["providers"][0]["endpoints"][0]["base_url"].as_str(),
        Some("http://127.0.0.1:9/v1")
    );
    assert_eq!(
        summary["session_cards"].as_array().map(|cards| cards.len()),
        Some(1)
    );
    assert_eq!(
        summary["session_cards"][0]["session_id"].as_str(),
        Some("sid-summary")
    );
    assert_eq!(
        summary["session_cards"][0]["last_client_name"].as_str(),
        Some("Frank-Desk")
    );
    assert_eq!(
        summary["session_cards"][0]["effective_station"]["value"].as_str(),
        Some("test")
    );
    assert_eq!(
        summary["session_cards"][0]["effective_station"]["source"].as_str(),
        Some("global_override")
    );
    assert_eq!(
        summary["session_cards"][0]["effective_model"]["value"].as_str(),
        Some("gpt-5.4-mini")
    );
    assert_eq!(summary["counts"]["active_requests"].as_u64(), Some(1));
    assert_eq!(summary["counts"]["recent_requests"].as_u64(), Some(3));
    assert_eq!(summary["counts"]["sessions"].as_u64(), Some(1));
    assert_eq!(summary["counts"]["stations"].as_u64(), Some(1));
    assert_eq!(summary["counts"]["profiles"].as_u64(), Some(1));
    assert_eq!(summary["counts"]["providers"].as_u64(), Some(1));
    assert_eq!(
        summary["retry"]["configured_profile"].as_str(),
        Some("balanced")
    );
    assert_eq!(summary["retry"]["supports_write"].as_bool(), Some(true));
    assert_eq!(
        summary["retry"]["allow_cross_station_before_first_output"].as_bool(),
        Some(true)
    );
    assert_eq!(
        summary["retry"]["recent_retried_requests"].as_u64(),
        Some(2)
    );
    assert_eq!(
        summary["retry"]["recent_cross_station_failovers"].as_u64(),
        Some(1)
    );
    assert_eq!(
        summary["retry"]["recent_fast_mode_requests"].as_u64(),
        Some(2)
    );
    assert_eq!(summary["health"]["stations_draining"].as_u64(), Some(1));
    assert_eq!(
        summary["health"]["stations_with_active_health_checks"].as_u64(),
        Some(1)
    );
    assert_eq!(
        summary["health"]["stations_with_probe_failures"].as_u64(),
        Some(1)
    );
    assert_eq!(
        summary["health"]["stations_with_failing_passive_health"].as_u64(),
        Some(1)
    );
    assert_eq!(
        summary["health"]["stations_with_cooldown"].as_u64(),
        Some(1)
    );
    assert_eq!(
        summary["health"]["stations_with_usage_exhaustion"].as_u64(),
        Some(1)
    );
    assert_eq!(
        summary["surface_capabilities"]["operator_summary"].as_bool(),
        Some(true)
    );
    assert_eq!(
        summary["surface_capabilities"]["request_ledger_recent"].as_bool(),
        Some(true)
    );
    assert_eq!(
        summary["surface_capabilities"]["request_ledger_summary"].as_bool(),
        Some(true)
    );
    assert_eq!(
        summary["surface_capabilities"]["station_persisted_settings"].as_bool(),
        Some(false)
    );
    assert_eq!(
        summary["remote_admin_access"]["remote_requires_token"].as_bool(),
        Some(true)
    );
    let summary_obj = summary.as_object().expect("summary object");
    for key in [
        "api_version",
        "service_name",
        "runtime",
        "counts",
        "retry",
        "health",
        "session_cards",
        "stations",
        "profiles",
        "providers",
        "links",
        "surface_capabilities",
        "shared_capabilities",
        "host_local_capabilities",
        "remote_admin_access",
    ] {
        assert!(
            summary_obj.contains_key(key),
            "operator summary missing top-level field: {key}"
        );
    }
    for legacy_key in ["configs", "active_config", "config_health"] {
        assert!(
            !summary_obj.contains_key(legacy_key),
            "operator summary should not expose legacy field: {legacy_key}"
        );
    }
    let runtime_obj = summary["runtime"].as_object().expect("runtime object");
    for key in [
        "runtime_loaded_at_ms",
        "runtime_source_mtime_ms",
        "configured_active_station",
        "effective_active_station",
        "global_station_override",
        "configured_default_profile",
        "default_profile",
        "default_profile_summary",
    ] {
        assert!(
            runtime_obj.contains_key(key),
            "operator summary runtime missing field: {key}"
        );
    }
    for legacy_key in [
        "active",
        "active_config",
        "configured_active_config",
        "effective_active_config",
    ] {
        assert!(
            !runtime_obj.contains_key(legacy_key),
            "operator summary runtime should not expose legacy field: {legacy_key}"
        );
    }
    assert!(summary["session_cards"][0]["effective_config"].is_null());
    assert_eq!(
        summary["links"]["snapshot"].as_str(),
        Some("/__codex_helper/api/v1/snapshot")
    );
    assert_eq!(
        summary["links"]["status_active"].as_str(),
        Some("/__codex_helper/api/v1/status/active")
    );
    assert_eq!(
        summary["links"]["sessions"].as_str(),
        Some("/__codex_helper/api/v1/sessions")
    );
    assert_eq!(
        summary["links"]["session_by_id_template"].as_str(),
        Some("/__codex_helper/api/v1/sessions/{session_id}")
    );
    assert_eq!(
        summary["links"]["runtime_status"].as_str(),
        Some("/__codex_helper/api/v1/runtime/status")
    );
    assert_eq!(
        summary["links"]["retry_config"].as_str(),
        Some("/__codex_helper/api/v1/retry/config")
    );
    assert_eq!(
        summary["links"]["request_ledger_recent"].as_str(),
        Some("/__codex_helper/api/v1/request-ledger/recent")
    );
    assert_eq!(
        summary["links"]["request_ledger_summary"].as_str(),
        Some("/__codex_helper/api/v1/request-ledger/summary")
    );
    assert_eq!(
        summary["links"]["station_probe"].as_str(),
        Some("/__codex_helper/api/v1/stations/probe")
    );
    assert_eq!(
        summary["links"]["healthcheck_start"].as_str(),
        Some("/__codex_helper/api/v1/healthcheck/start")
    );
    assert_eq!(
        summary["links"]["healthcheck_cancel"].as_str(),
        Some("/__codex_helper/api/v1/healthcheck/cancel")
    );
    assert_eq!(
        summary["links"]["provider_balance_refresh"].as_str(),
        Some("/__codex_helper/api/v1/providers/balances/refresh")
    );
    assert_eq!(
        summary["links"]["global_station_override"].as_str(),
        Some("/__codex_helper/api/v1/overrides/global-station")
    );
    assert_eq!(
        summary["links"]["persisted_default_profile"].as_str(),
        Some("/__codex_helper/api/v1/profiles/default/persisted")
    );
    assert!(summary["links"]["config_active"].is_null());

    proxy_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_provider_runtime_override_filters_real_routing() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let upstream_default = axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": "resp-default",
                    "output": [],
                })),
            )
        }),
    );
    let upstream_backup = axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": "resp-backup",
                    "output": [],
                })),
            )
        }),
    );
    let (default_addr, default_handle) = spawn_axum_server(upstream_default);
    let (backup_addr, backup_handle) = spawn_axum_server(upstream_backup);

    let mut cfg = ProxyConfigV2 {
        version: 2,
        codex: ServiceViewV2::default(),
        claude: ServiceViewV2::default(),
        retry: RetryConfig::default(),
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };
    cfg.codex.active_group = Some("main".to_string());
    cfg.codex.providers.insert(
        "alpha".to_string(),
        ProviderConfigV2 {
            alias: Some("Alpha".to_string()),
            enabled: true,
            auth: UpstreamAuth::default(),
            tags: [("provider_id".to_string(), "alpha".to_string())]
                .into_iter()
                .collect(),
            supported_models: Default::default(),
            model_mapping: Default::default(),
            endpoints: [
                (
                    "default".to_string(),
                    ProviderEndpointV2 {
                        base_url: format!("http://{default_addr}/v1"),
                        enabled: true,
                        priority: 0,
                        tags: Default::default(),
                        supported_models: Default::default(),
                        model_mapping: Default::default(),
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderEndpointV2 {
                        base_url: format!("http://{backup_addr}/v1"),
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
        "main".to_string(),
        GroupConfigV2 {
            alias: Some("Main".to_string()),
            enabled: true,
            level: 1,
            members: vec![GroupMemberRefV2 {
                provider: "alpha".to_string(),
                endpoint_names: Vec::new(),
                preferred: true,
            }],
        },
    );

    crate::config::save_config_v2(&cfg)
        .await
        .expect("write provider runtime v2 config");
    let loaded = crate::config::load_config()
        .await
        .expect("load provider runtime config");

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
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"input":"hello"}"#)
        .send()
        .await
        .expect("initial routed request")
        .error_for_status()
        .expect("initial routed request status")
        .json::<serde_json::Value>()
        .await
        .expect("initial routed request json");
    assert_eq!(
        initial.get("id").and_then(|value| value.as_str()),
        Some("resp-default")
    );

    let initial_provider_surface = client
        .get(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/providers"
        ))
        .send()
        .await
        .expect("get providers send")
        .error_for_status()
        .expect("get providers status")
        .json::<serde_json::Value>()
        .await
        .expect("get providers json");
    assert_eq!(
        initial_provider_surface
            .as_array()
            .map(|providers| providers.len()),
        Some(1)
    );
    assert_eq!(
        initial_provider_surface[0]["endpoints"]
            .as_array()
            .map(|endpoints| endpoints.len()),
        Some(2)
    );
    let provider_name = initial_provider_surface[0]
        .get("name")
        .and_then(|value| value.as_str())
        .expect("provider name")
        .to_string();
    let default_base_url = format!("http://{default_addr}/v1");
    let default_endpoint_name = initial_provider_surface[0]["endpoints"]
        .as_array()
        .and_then(|endpoints| {
            endpoints.iter().find_map(|endpoint| {
                (endpoint.get("base_url").and_then(|value| value.as_str())
                    == Some(default_base_url.as_str()))
                .then(|| endpoint.get("name").and_then(|value| value.as_str()))
                .flatten()
            })
        })
        .expect("default endpoint name")
        .to_string();

    let update = client
        .post(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/providers/runtime"
        ))
        .json(&serde_json::json!({
            "provider_name": provider_name,
            "endpoint_name": default_endpoint_name,
            "enabled": false,
            "runtime_state": "breaker_open"
        }))
        .send()
        .await
        .expect("apply provider runtime override send");
    assert_eq!(update.status(), StatusCode::NO_CONTENT);

    let after_update = client
        .get(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/providers"
        ))
        .send()
        .await
        .expect("get providers after update send")
        .error_for_status()
        .expect("get providers after update status")
        .json::<serde_json::Value>()
        .await
        .expect("get providers after update json");
    let default_endpoint = after_update[0]["endpoints"]
        .as_array()
        .and_then(|endpoints| {
            endpoints.iter().find(|endpoint| {
                endpoint.get("base_url").and_then(|value| value.as_str())
                    == Some(default_base_url.as_str())
            })
        })
        .expect("default endpoint");
    assert_eq!(
        default_endpoint
            .get("runtime_enabled_override")
            .and_then(|value| value.as_bool()),
        Some(false)
    );
    assert_eq!(
        default_endpoint
            .get("runtime_state_override")
            .and_then(|value| value.as_str()),
        Some("breaker_open")
    );
    assert_eq!(
        default_endpoint
            .get("routable")
            .and_then(|value| value.as_bool()),
        Some(false)
    );

    let after = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"input":"hello again"}"#)
        .send()
        .await
        .expect("routed request after runtime override")
        .error_for_status()
        .expect("routed request after runtime override status")
        .json::<serde_json::Value>()
        .await
        .expect("routed request after runtime override json");
    assert_eq!(
        after.get("id").and_then(|value| value.as_str()),
        Some("resp-backup")
    );

    proxy_handle.abort();
    default_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_api_v1_provider_runtime_override_filters_v4_route_plan_routing() {
    let _env_lock = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let upstream_default = axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": "resp-default",
                    "output": [],
                })),
            )
        }),
    );
    let upstream_backup = axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": "resp-backup",
                    "output": [],
                })),
            )
        }),
    );
    let (default_addr, default_handle) = spawn_axum_server(upstream_default);
    let (backup_addr, backup_handle) = spawn_axum_server(upstream_backup);

    let cfg = ProxyConfigV4 {
        version: 4,
        codex: ServiceViewV4 {
            providers: BTreeMap::from([(
                "alpha".to_string(),
                ProviderConfigV4 {
                    endpoints: BTreeMap::from([
                        (
                            "default".to_string(),
                            ProviderEndpointV4 {
                                base_url: format!("http://{default_addr}/v1"),
                                enabled: true,
                                priority: 0,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                            },
                        ),
                        (
                            "backup".to_string(),
                            ProviderEndpointV4 {
                                base_url: format!("http://{backup_addr}/v1"),
                                enabled: true,
                                priority: 1,
                                tags: BTreeMap::new(),
                                supported_models: BTreeMap::new(),
                                model_mapping: BTreeMap::new(),
                            },
                        ),
                    ]),
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(RoutingConfigV4::ordered_failover(vec!["alpha".to_string()])),
            ..ServiceViewV4::default()
        },
        claude: ServiceViewV4::default(),
        retry: RetryConfig::default(),
        notify: Default::default(),
        default_service: None,
        ui: UiConfig::default(),
    };

    crate::config::save_config_v4(&cfg)
        .await
        .expect("write provider runtime v4 config");
    let loaded = crate::config::load_config_with_v4_source()
        .await
        .expect("load provider runtime v4 config");

    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(loaded.runtime),
        loaded.v4.map(Arc::new),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let initial = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"input":"hello"}"#)
        .send()
        .await
        .expect("initial v4 routed request")
        .error_for_status()
        .expect("initial v4 routed request status")
        .json::<serde_json::Value>()
        .await
        .expect("initial v4 routed request json");
    assert_eq!(
        initial.get("id").and_then(|value| value.as_str()),
        Some("resp-default")
    );

    let initial_provider_surface = client
        .get(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/providers"
        ))
        .send()
        .await
        .expect("get v4 providers send")
        .error_for_status()
        .expect("get v4 providers status")
        .json::<serde_json::Value>()
        .await
        .expect("get v4 providers json");
    let provider_name = initial_provider_surface[0]
        .get("name")
        .and_then(|value| value.as_str())
        .expect("provider name")
        .to_string();
    let default_base_url = format!("http://{default_addr}/v1");
    let default_endpoint_name = initial_provider_surface[0]["endpoints"]
        .as_array()
        .and_then(|endpoints| {
            endpoints.iter().find_map(|endpoint| {
                (endpoint.get("base_url").and_then(|value| value.as_str())
                    == Some(default_base_url.as_str()))
                .then(|| endpoint.get("name").and_then(|value| value.as_str()))
                .flatten()
            })
        })
        .expect("default endpoint name")
        .to_string();

    let update = client
        .post(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/providers/runtime"
        ))
        .json(&serde_json::json!({
            "provider_name": provider_name,
            "endpoint_name": default_endpoint_name,
            "enabled": false,
            "runtime_state": "breaker_open"
        }))
        .send()
        .await
        .expect("apply v4 provider runtime override send");
    assert_eq!(update.status(), StatusCode::NO_CONTENT);

    let explain = client
        .get(format!(
            "http://{proxy_addr}/__codex_helper/api/v1/routing/explain"
        ))
        .send()
        .await
        .expect("v4 routing explain send")
        .error_for_status()
        .expect("v4 routing explain status")
        .json::<serde_json::Value>()
        .await
        .expect("v4 routing explain json");
    assert_eq!(
        explain["selected_route"]["endpoint_id"].as_str(),
        Some("backup")
    );
    assert_eq!(
        explain["candidates"][0]["skip_reasons"][0]["code"].as_str(),
        Some("runtime_disabled")
    );

    let after = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"input":"hello again"}"#)
        .send()
        .await
        .expect("v4 routed request after runtime override")
        .error_for_status()
        .expect("v4 routed request after runtime override status")
        .json::<serde_json::Value>()
        .await
        .expect("v4 routed request after runtime override json");
    assert_eq!(
        after.get("id").and_then(|value| value.as_str()),
        Some("resp-backup")
    );

    proxy_handle.abort();
    default_handle.abort();
    backup_handle.abort();
}
