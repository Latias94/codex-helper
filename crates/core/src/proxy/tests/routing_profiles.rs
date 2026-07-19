use super::*;
use crate::dashboard_core::{OperatorReadModel, OperatorReadStatus};
use crate::state::{RouteValueSource, SessionContinuityMode};

fn proxy_from_helper_config(source: HelperConfig) -> ProxyService {
    ProxyService::new(Client::new(), Arc::new(source), "codex")
}

fn single_provider_config(provider_id: &str, provider: ProviderConfig) -> HelperConfig {
    HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([(provider_id.to_string(), provider)]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                provider_id.to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    }
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

    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "primary".to_string(),
                "backup".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        retry: RetryConfig {
            profile: Some(RetryProfileName::AggressiveFailover),
            ..Default::default()
        },
        ..HelperConfig::default()
    };

    let proxy = proxy_from_helper_config(source);
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

    let runtime = proxy_for_state
        .state
        .route_plan_runtime_state_for_provider_endpoints("codex")
        .await;
    let primary = runtime.provider_endpoint(&crate::runtime_identity::ProviderEndpointKey::new(
        "codex", "primary", "default",
    ));
    assert_eq!(primary.failure_count, 0);
    assert!(!primary.cooldown_active);

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_default_profile_binding_does_not_patch_request_fields() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|body: Bytes| async move {
            let json: serde_json::Value =
                serde_json::from_slice(&body).expect("echo upstream json");
            (StatusCode::OK, Json(json))
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let mut source = single_provider_config(
        "u-bind",
        ProviderConfig {
            base_url: Some(format!("http://{upstream_addr}/v1")),
            ..ProviderConfig::default()
        },
    );
    source.codex.default_profile = Some("daily".to_string());
    source.codex.profiles.insert(
        "daily".to_string(),
        ServiceControlProfile {
            extends: None,
            model: Some("gpt-5.4-fast".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: Some("priority".to_string()),
        },
    );

    let proxy = proxy_from_helper_config(source);
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("session_id", "sid-bind")
        .body(
            r#"{"input":"hi","model":"client-model","service_tier":"default","reasoning":{"effort":"medium"}}"#,
        )
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
        Some("client-model")
    );
    assert_eq!(
        resp.get("service_tier").and_then(|v| v.as_str()),
        Some("default")
    );
    assert_eq!(
        resp.get("reasoning")
            .and_then(|v| v.get("effort"))
            .and_then(|v| v.as_str()),
        Some("medium")
    );

    let model = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/operator/read-model",
            proxy_addr
        ))
        .send()
        .await
        .expect("binding operator read model send")
        .error_for_status()
        .expect("binding operator read model status")
        .json::<OperatorReadModel>()
        .await
        .expect("binding operator read model json");

    assert_eq!(model.status, OperatorReadStatus::Ready);
    let data = model.data.expect("ready operator read model data");
    let card = data
        .summary
        .sessions
        .first()
        .expect("default-profile session projection");
    assert_eq!(card.binding_profile_name.as_deref(), Some("daily"));
    assert_eq!(
        card.binding_continuity_mode,
        Some(SessionContinuityMode::DefaultProfile)
    );
    let effective_model = card.effective_model.as_ref().expect("effective model");
    assert_eq!(effective_model.value, "client-model");
    assert_eq!(effective_model.source, RouteValueSource::RequestPayload);
    let effective_service_tier = card
        .effective_service_tier
        .as_ref()
        .expect("effective service tier");
    assert_eq!(effective_service_tier.value, "default");
    assert_eq!(
        effective_service_tier.source,
        RouteValueSource::RequestPayload
    );
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("session_id", "sid-bind-fast")
        .body(
            r#"{"input":"hi","model":"codex-client-model","service_tier":"priority","reasoning":{"effort":"high"}}"#,
        )
        .send()
        .await
        .expect("send fast mode bind request")
        .error_for_status()
        .expect("fast mode bind request status")
        .json::<serde_json::Value>()
        .await
        .expect("fast mode bind request json");

    assert_eq!(
        resp.get("model").and_then(|v| v.as_str()),
        Some("codex-client-model")
    );
    assert_eq!(
        resp.get("service_tier").and_then(|v| v.as_str()),
        Some("priority")
    );
    assert_eq!(
        resp.get("reasoning")
            .and_then(|v| v.get("effort"))
            .and_then(|v| v.as_str()),
        Some("high")
    );

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn claude_settings_reader_observes_source_changes_without_retaining_json() {
    let _env_lock = env_lock().await;
    let mut scoped = ScopedEnv::default();
    let base = make_temp_test_dir();
    let claude_home = base.join("claude-home");
    let claude_settings = claude_home.join("settings.json");

    unsafe {
        scoped.set_path("CLAUDE_HOME", &claude_home);
    }

    write_text_file(
        &claude_settings,
        r#"{"env":{"ANTHROPIC_API_KEY":"claude-first"}}"#,
    );

    assert_eq!(
        super::claude_settings_env_value("ANTHROPIC_API_KEY"),
        Some("claude-first".to_string())
    );

    write_text_file(
        &claude_settings,
        r#"{"env":{"ANTHROPIC_API_KEY":"claude-second"}}"#,
    );
    assert_eq!(
        super::claude_settings_env_value("ANTHROPIC_API_KEY"),
        Some("claude-second".to_string())
    );
}

#[tokio::test]
async fn proxy_admin_routes_require_configured_token_even_from_loopback() {
    let _env_lock = env_lock().await;
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set(super::ADMIN_TOKEN_ENV_VAR, "remote-secret");
    }

    let source = single_provider_config(
        "admin-test",
        ProviderConfig {
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            ..ProviderConfig::default()
        },
    );
    let proxy = proxy_from_helper_config(source);
    let app = crate::proxy::router(proxy);
    let remote_addr = std::net::SocketAddr::from(([203, 0, 113, 7], 43123));

    let mut denied_req = Request::builder()
        .uri("/__codex_helper/api/v1/operator/read-model")
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

    let loopback_addr = std::net::SocketAddr::from(([127, 0, 0, 1], 43124));
    let mut loopback_without_token = Request::builder()
        .uri("/__codex_helper/api/v1/operator/read-model")
        .body(Body::empty())
        .expect("build loopback request without token");
    loopback_without_token
        .extensions_mut()
        .insert(ConnectInfo(loopback_addr));
    let loopback_denied = app
        .clone()
        .oneshot(loopback_without_token)
        .await
        .expect("loopback denial response");
    assert_eq!(loopback_denied.status(), StatusCode::FORBIDDEN);

    let mut allowed_req = Request::builder()
        .uri("/__codex_helper/api/v1/operator/read-model")
        .header(super::ADMIN_TOKEN_HEADER, "remote-secret")
        .body(Body::empty())
        .expect("build allowed request");
    allowed_req
        .extensions_mut()
        .insert(ConnectInfo(loopback_addr));
    let allowed = app.oneshot(allowed_req).await.expect("allowed response");
    assert_eq!(allowed.status(), StatusCode::OK);
    let allowed_body = to_bytes(allowed.into_body(), usize::MAX)
        .await
        .expect("allowed operator read model body");
    let allowed_model: OperatorReadModel =
        serde_json::from_slice(&allowed_body).expect("allowed operator read model json");
    assert_eq!(allowed_model.status, OperatorReadStatus::Ready);
    assert!(allowed_model.validate().is_ok());
}

#[tokio::test]
async fn proxy_split_listeners_isolate_admin_routes_from_proxy_traffic() {
    let source = single_provider_config(
        "listener-test",
        ProviderConfig {
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            ..ProviderConfig::default()
        },
    );

    let proxy = proxy_from_helper_config(source);
    let proxy_app = crate::proxy::proxy_only_router(proxy.clone());
    let admin_app = crate::proxy::admin_listener_router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(proxy_app);
    let (admin_addr, admin_handle) = spawn_axum_server(admin_app);

    let client = reqwest::Client::new();

    let proxy_admin = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/operator/read-model",
            proxy_addr
        ))
        .send()
        .await
        .expect("proxy admin send");
    assert_eq!(proxy_admin.status(), StatusCode::NOT_FOUND);

    let admin_model = client
        .get(format!(
            "http://{}/__codex_helper/api/v1/operator/read-model",
            admin_addr
        ))
        .send()
        .await
        .expect("admin operator read model send")
        .error_for_status()
        .expect("admin operator read model status")
        .json::<OperatorReadModel>()
        .await
        .expect("admin operator read model json");
    assert_eq!(admin_model.status, OperatorReadStatus::Ready);
    assert_eq!(admin_model.service_name, "codex");
    assert!(admin_model.validate().is_ok());

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
