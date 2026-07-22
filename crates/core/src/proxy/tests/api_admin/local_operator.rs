use super::*;
use crate::control_plane_client::{ControlPlaneEndpoint, LocalOperatorClient};
use crate::credentials::{CredentialName, CredentialSourceCapabilities, SecretValue};
use crate::dashboard_core::{OperatorReadModel, OperatorRoutingSummary};
use crate::local_operator::{
    LocalOperatorSessionRequest, LocalOperatorSessionResponse, local_operator_client_proof,
    local_operator_request_signature, new_local_operator_nonce, unix_time_ms,
    verify_local_operator_server_proof,
};
use crate::proxy::tests::harness::{
    TestProxyServer, post_responses_json, proxy_service, spawn_proxy_service, spawn_test_upstream,
};
use crate::proxy::{
    LOCAL_OPERATOR_NONCE_HEADER, LOCAL_OPERATOR_SESSION_HEADER, LOCAL_OPERATOR_SIGNATURE_HEADER,
    LOCAL_OPERATOR_TIMESTAMP_HEADER, LOCAL_V1_BALANCE_REFRESH, LOCAL_V1_CREDENTIAL_REFRESH,
    LOCAL_V1_OPERATOR_SESSION, OperatorEndpointMode, OperatorRoutingCommand,
    OperatorRoutingMutationRequest, OperatorRoutingMutationStatus, OperatorSessionAffinityCommand,
    OperatorSessionAffinityMutationRequest, OperatorSessionAffinityMutationStatus,
    OperatorSessionBindingCommand, OperatorSessionBindingMutationRequest,
    OperatorSessionBindingMutationStatus,
};
use crate::service_target::{
    LocalCredentialRefreshAction, LocalCredentialRefreshRequest, LocalCredentialRefreshStatus,
    LocalServiceRuntimeReadRequest, ServiceInstallGeneration, ServiceRuntimeIdentity,
};
use crate::state::{FinishRequestParams, RuntimeConfigState, SessionRouteAffinityTarget};

fn spawn_admin_listener(proxy: ProxyService) -> TestProxyServer {
    let app = crate::proxy::admin_listener_router(proxy);
    let (addr, handle) = spawn_axum_server(app);
    TestProxyServer { addr, handle }
}

fn empty_proxy_config() -> HelperConfig {
    make_helper_config(Vec::new(), RetryConfig::default())
}

fn native_credential_proxy_config() -> HelperConfig {
    HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([(
                "native".to_string(),
                ProviderConfig {
                    base_url: Some("https://native.example.test/v1".to_string()),
                    inline_auth: UpstreamAuth {
                        auth_token_ref: Some(crate::config::CredentialRef::Native {
                            name: "relay.primary".to_string(),
                        }),
                        ..UpstreamAuth::default()
                    },
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "native".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    }
}

fn native_credential_proxy(
    generation: Option<ServiceInstallGeneration>,
) -> (
    ProxyService,
    crate::credentials::TestNativeCredentialControl,
) {
    let initial =
        SecretValue::new(b"credential-generation-a".to_vec()).expect("valid initial credential");
    let (credential_sources, control) = CredentialSourceCapabilities::test_native(initial);
    let runtime_store = Arc::new(
        crate::runtime_store::RuntimeStore::open_in_memory()
            .expect("open local operator runtime store"),
    );
    let proxy = ProxyService::new_with_runtime_store_and_credential_sources(
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build test proxy client"),
        Arc::new(native_credential_proxy_config()),
        "codex",
        runtime_store,
        credential_sources,
    )
    .expect("build native credential proxy")
    .with_service_install_generation(generation);
    (proxy, control)
}

fn proxy_with_usage_provider_catalog(
    config: HelperConfig,
    usage_provider_catalog: crate::usage_providers::UsageProviderCredentialCatalog,
) -> ProxyService {
    let runtime_store = Arc::new(
        crate::runtime_store::RuntimeStore::open_in_memory()
            .expect("open isolated usage provider runtime store"),
    );
    let (runtime_config, state) =
        crate::proxy::RuntimeConfig::new_with_usage_provider_catalog_for_test(
            Arc::new(config),
            runtime_store,
            CredentialSourceCapabilities::server(),
            usage_provider_catalog,
        )
        .expect("build proxy with captured usage provider catalog");
    ProxyService {
        client: crate::proxy::upstream_http_client_builder()
            .build()
            .expect("build test proxy client"),
        config: Arc::new(runtime_config),
        service_name: "codex",
        concurrency_limiter: Arc::new(
            crate::proxy::concurrency_limits::ConcurrencyLimiter::default(),
        ),
        filter: crate::filter::RequestFilter::new(),
        state,
        service_install_generation: None,
        service_runtime_identity: None,
        local_runtime_shutdown: None,
    }
}

fn routing_proxy_config() -> HelperConfig {
    HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "input".to_string(),
                    ProviderConfig {
                        base_url: Some("https://input.example.test/v1".to_string()),
                        continuity_domain: Some("shared-relay-state".to_string()),
                        inline_auth: UpstreamAuth {
                            auth_token: Some("input-test-token".to_string().into()),
                            ..UpstreamAuth::default()
                        },
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "ciii".to_string(),
                    ProviderConfig {
                        base_url: Some("https://ciii.example.test/v1".to_string()),
                        continuity_domain: Some("shared-relay-state".to_string()),
                        inline_auth: UpstreamAuth {
                            auth_token: Some("ciii-test-token".to_string().into()),
                            ..UpstreamAuth::default()
                        },
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::round_robin(vec![
                "input".to_string(),
                "ciii".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    }
}

fn conditional_routing_proxy_config() -> HelperConfig {
    let mut config = routing_proxy_config();
    config.codex.routing = Some(RouteGraphConfig {
        entry: "root".to_string(),
        routes: std::collections::BTreeMap::from([(
            "root".to_string(),
            RouteNodeConfig {
                strategy: RouteStrategy::Conditional,
                when: Some(RouteCondition {
                    model: Some("gpt-5".to_string()),
                    ..RouteCondition::default()
                }),
                then: Some("input".to_string()),
                default_route: Some("ciii".to_string()),
                ..RouteNodeConfig::default()
            },
        )]),
        ..RouteGraphConfig::default()
    });
    config
}

async fn observe_idle_session(proxy: &ProxyService, session_id: &str) {
    let request_id = proxy
        .state
        .begin_request(
            "codex",
            "POST",
            "/v1/responses",
            Some(session_id.to_string()),
            None,
            None,
            None,
            None,
            Some("gpt-test".to_string()),
            None,
            None,
            10,
        )
        .await;
    assert!(
        proxy
            .state
            .finish_request(FinishRequestParams {
                id: request_id,
                winning_attempt: None,
                status_code: 200,
                duration_ms: 10,
                ended_at_ms: 20,
                observed_service_tier: None,
                reported_model: Some("gpt-test".to_string()),
                usage: None,
                retry: None,
                ttfb_ms: Some(2),
                streaming: false,
            })
            .await
    );
}

async fn seed_session_affinity(proxy: &ProxyService, session_id: &str, provider_id: &str) {
    let runtime_snapshot = proxy.config.capture().await;
    let graph = runtime_snapshot
        .route_graph(proxy.service_name)
        .expect("compiled route graph");
    let template = graph.handshake_plan();
    let candidate = template
        .candidates
        .iter()
        .find(|candidate| candidate.provider_id == provider_id)
        .expect("affinity provider candidate");
    proxy
        .state
        .record_session_route_affinity_success(
            None,
            session_id,
            SessionRouteAffinityTarget {
                route_graph_key: template.route_graph_key(),
                session_identity_source: None,
                provider_endpoint: template.candidate_provider_endpoint_key(candidate),
                upstream_base_url: candidate.base_url.clone(),
                route_path: candidate.route_path.clone(),
            },
            Some("initial_selection".to_string()),
            30,
        )
        .await
        .expect("seed session affinity");
}

fn routing_mutation_request(
    routing: &OperatorRoutingSummary,
    command: OperatorRoutingCommand,
) -> OperatorRoutingMutationRequest {
    OperatorRoutingMutationRequest {
        expected_route_graph_key: routing.route_graph_key.clone(),
        expected_control_revision: routing.control_revision,
        expected_policy_revision: routing.provider_policy_revision,
        command,
    }
}

async fn read_operator_model(
    client: &reqwest::Client,
    server: &TestProxyServer,
) -> OperatorReadModel {
    client
        .get(server.url("/__codex_helper/api/v1/operator/read-model"))
        .send()
        .await
        .expect("request operator read model")
        .error_for_status()
        .expect("operator read-model status")
        .json::<OperatorReadModel>()
        .await
        .expect("decode operator read model")
}

async fn begin_operator_session(
    client: &reqwest::Client,
    server: &TestProxyServer,
    token: &str,
    admin_token: Option<&str>,
) -> (String, LocalOperatorSessionResponse) {
    let client_nonce = new_local_operator_nonce();
    let timestamp_ms = unix_time_ms();
    let proof = local_operator_client_proof(token, &client_nonce, timestamp_ms)
        .expect("sign local operator session request");
    let mut request =
        client
            .post(server.url(LOCAL_V1_OPERATOR_SESSION))
            .json(&LocalOperatorSessionRequest {
                client_nonce: client_nonce.clone(),
                timestamp_ms,
                proof,
            });
    if let Some(admin_token) = admin_token {
        request = request.header(ADMIN_TOKEN_HEADER, admin_token);
    }
    let response = request
        .send()
        .await
        .expect("send local operator session request");
    assert_eq!(response.status(), StatusCode::OK);
    let session = response
        .json::<LocalOperatorSessionResponse>()
        .await
        .expect("decode local operator session");
    verify_local_operator_server_proof(token, &client_nonce, &session)
        .expect("verify daemon proof");
    (client_nonce, session)
}

struct SignedOperatorRequestContext<'a> {
    client: &'a reqwest::Client,
    server: &'a TestProxyServer,
    token: &'a str,
    client_nonce: &'a str,
    session: &'a LocalOperatorSessionResponse,
    admin_token: Option<&'a str>,
}

fn signed_balance_request(
    context: &SignedOperatorRequestContext<'_>,
    body_to_sign: &[u8],
    body_to_send: Vec<u8>,
) -> reqwest::RequestBuilder {
    signed_operator_request(
        context,
        LOCAL_V1_BALANCE_REFRESH,
        body_to_sign,
        body_to_send,
        unix_time_ms(),
    )
}

fn signed_operator_request(
    context: &SignedOperatorRequestContext<'_>,
    path: &str,
    body_to_sign: &[u8],
    body_to_send: Vec<u8>,
    timestamp_ms: u64,
) -> reqwest::RequestBuilder {
    let request_nonce = new_local_operator_nonce();
    let signature = local_operator_request_signature(
        context.token,
        context.client_nonce,
        &context.session.session_id,
        &request_nonce,
        timestamp_ms,
        path,
        body_to_sign,
    )
    .expect("sign local operator request");
    let mut request = context
        .client
        .post(context.server.url(path))
        .header(LOCAL_OPERATOR_SESSION_HEADER, &context.session.session_id)
        .header(LOCAL_OPERATOR_NONCE_HEADER, request_nonce)
        .header(LOCAL_OPERATOR_TIMESTAMP_HEADER, timestamp_ms.to_string())
        .header(LOCAL_OPERATOR_SIGNATURE_HEADER, signature)
        .header("content-type", "application/json")
        .body(body_to_send);
    if let Some(admin_token) = context.admin_token {
        request = request.header(ADMIN_TOKEN_HEADER, admin_token);
    }
    request
}

#[tokio::test]
async fn local_operator_http_protocol_enforces_proof_replay_and_admin_layers() {
    const ADMIN_TOKEN: &str = "operator-admin-test-token";

    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    let token =
        crate::local_operator::ensure_local_operator_token().expect("create operator token");
    let proxy = proxy_service(empty_proxy_config());
    let raw_session_id = "local-session-metadata-handle";
    let _request_id = proxy
        .state
        .begin_request(
            "codex",
            "POST",
            "/v1/responses",
            Some(raw_session_id.to_string()),
            None,
            Some("codex-cli".to_string()),
            Some("127.0.0.1:43123".to_string()),
            Some("/workspace/project".to_string()),
            Some("gpt-test".to_string()),
            None,
            None,
            10,
        )
        .await;
    let server = spawn_admin_listener(proxy.clone());

    let endpoint = ControlPlaneEndpoint::new(format!("http://{}", server.addr), None::<String>)
        .expect("loopback endpoint");
    let operator = LocalOperatorClient::new(endpoint, &token).expect("local operator client");
    let response = operator
        .refresh_provider_balances(true)
        .await
        .expect("signed balance refresh");
    assert_eq!(response.service_name, "codex");
    let session_key = crate::dashboard_core::operator_summary::operator_session_key(raw_session_id);
    let local_sessions = operator
        .read_operator_session_metadata(vec![
            session_key.clone(),
            "session:sha256:missing".to_string(),
        ])
        .await
        .expect("signed local session metadata read");
    assert_eq!(local_sessions.service_name, "codex");
    assert_eq!(local_sessions.sessions.len(), 1);
    let local_session = local_sessions
        .sessions
        .get(&session_key)
        .expect("requested local session metadata");
    assert_eq!(local_session.raw_session_id, raw_session_id);
    assert_eq!(local_session.cwd.as_deref(), Some("/workspace/project"));
    assert_eq!(local_session.last_client_name.as_deref(), Some("codex-cli"));
    assert_eq!(
        local_session.last_client_addr.as_deref(),
        Some("127.0.0.1:43123")
    );

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build local client");
    let body = br#"{"force":true}"#;
    let (client_nonce, session) = begin_operator_session(&client, &server, &token, None).await;
    let request = signed_balance_request(
        &SignedOperatorRequestContext {
            client: &client,
            server: &server,
            token: &token,
            client_nonce: &client_nonce,
            session: &session,
            admin_token: None,
        },
        body,
        body.to_vec(),
    )
    .build()
    .expect("build signed request");
    let replay = request.try_clone().expect("clone replay request");

    let first = client.execute(request).await.expect("send signed request");
    assert_eq!(first.status(), StatusCode::OK);
    let replay = client.execute(replay).await.expect("send replay request");
    assert_eq!(replay.status(), StatusCode::FORBIDDEN);

    let (client_nonce, session) = begin_operator_session(&client, &server, &token, None).await;
    let tampered = signed_balance_request(
        &SignedOperatorRequestContext {
            client: &client,
            server: &server,
            token: &token,
            client_nonce: &client_nonce,
            session: &session,
            admin_token: None,
        },
        body,
        br#"{"force":false}"#.to_vec(),
    )
    .send()
    .await
    .expect("send tampered request");
    assert_eq!(tampered.status(), StatusCode::FORBIDDEN);

    drop(server);
    unsafe {
        scoped.set(ADMIN_TOKEN_ENV_VAR, ADMIN_TOKEN);
    }
    let server = spawn_admin_listener(proxy);

    let client_nonce = new_local_operator_nonce();
    let timestamp_ms = unix_time_ms();
    let proof = local_operator_client_proof(&token, &client_nonce, timestamp_ms)
        .expect("sign unauthenticated admin-layer request");
    let no_admin = client
        .post(server.url(LOCAL_V1_OPERATOR_SESSION))
        .json(&LocalOperatorSessionRequest {
            client_nonce,
            timestamp_ms,
            proof,
        })
        .send()
        .await
        .expect("send session without admin token");
    assert_eq!(no_admin.status(), StatusCode::FORBIDDEN);

    let no_proof = client
        .post(server.url(LOCAL_V1_BALANCE_REFRESH))
        .header(ADMIN_TOKEN_HEADER, ADMIN_TOKEN)
        .header("content-type", "application/json")
        .body(r#"{"force":true}"#)
        .send()
        .await
        .expect("send action without local proof");
    assert_eq!(no_proof.status(), StatusCode::FORBIDDEN);

    let endpoint =
        ControlPlaneEndpoint::new(format!("http://{}", server.addr), Some(ADMIN_TOKEN_ENV_VAR))
            .expect("authenticated loopback endpoint");
    let authenticated_operator =
        LocalOperatorClient::new(endpoint, &token).expect("local operator client");
    let response = authenticated_operator
        .refresh_provider_balances(true)
        .await
        .expect("two-layer authenticated refresh");
    assert_eq!(response.service_name, "codex");

    drop(server);
    drop(scoped);
    std::fs::remove_dir_all(home).expect("remove helper home");
}

#[tokio::test]
async fn signed_balance_refresh_isolates_rejected_catalog_entries_and_publishes_balance() {
    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    let token =
        crate::local_operator::ensure_local_operator_token().expect("create operator token");

    let usage_hits = Arc::new(AtomicUsize::new(0));
    let handler_hits = Arc::clone(&usage_hits);
    let usage_app = axum::Router::new().route(
        "/usage",
        get(move || {
            let handler_hits = Arc::clone(&handler_hits);
            async move {
                handler_hits.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({ "balance": "7.25" }))
            }
        }),
    );
    let usage_server = spawn_test_upstream(usage_app);

    let usage_provider_catalog =
        crate::usage_providers::usage_provider_credential_catalog_from_value_for_test(
            serde_json::json!({
                "providers": [
                    {
                        "id": "loopback-usage",
                        "kind": "openai_balance_http_json",
                        "domains": ["127.0.0.1"],
                        "endpoint": "/usage"
                    },
                    {
                        "id": "rejected-template",
                        "kind": "openai_balance_http_json",
                        "domains": ["invalid.example"],
                        "endpoint": "https://invalid.example/usage?token={{token}}"
                    }
                ]
            }),
        )
        .expect("build partially valid usage provider catalog");
    let config = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([(
                "loopback".to_string(),
                ProviderConfig {
                    base_url: Some(usage_server.base_url()),
                    inline_auth: UpstreamAuth {
                        auth_token: Some("loopback-usage-token".to_string().into()),
                        ..UpstreamAuth::default()
                    },
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "loopback".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = proxy_with_usage_provider_catalog(config, usage_provider_catalog);
    let server = spawn_admin_listener(proxy);
    let endpoint = ControlPlaneEndpoint::new(format!("http://{}", server.addr), None::<String>)
        .expect("loopback endpoint");
    let operator = LocalOperatorClient::new(endpoint, &token).expect("local operator client");

    let response = operator
        .refresh_provider_balances(true)
        .await
        .expect("signed provider balance refresh");

    assert_eq!(response.service_name, "codex");
    assert_eq!(response.refresh.providers_configured, 1);
    assert_eq!(response.refresh.providers_rejected, 1);
    assert_eq!(response.refresh.providers_matched, 1);
    assert_eq!(response.refresh.upstreams_matched, 1);
    assert_eq!(response.refresh.attempted, 1);
    assert_eq!(response.refresh.refreshed, 1);
    assert_eq!(response.refresh.failed, 0);
    assert_eq!(response.refresh.missing_token, 0);
    assert_eq!(response.refresh.auto_attempted, 0);
    assert_eq!(usage_hits.load(Ordering::SeqCst), 1);
    assert_eq!(response.refresh.rejected_providers.len(), 1);
    assert_eq!(
        response.refresh.rejected_providers[0].provider_id,
        "rejected-template"
    );
    assert_eq!(
        response.refresh.rejected_providers[0].code,
        "invalid_config"
    );
    assert_eq!(
        response
            .provider_balances
            .first()
            .expect("refreshed provider balance")
            .total_balance_usd
            .as_deref(),
        Some("7.25")
    );

    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build local client");
    let operator_model = read_operator_model(&client, &server).await;
    let published = operator_model
        .data
        .as_ref()
        .expect("ready operator data")
        .provider_balances
        .iter()
        .find(|balance| {
            balance.observation_provider_id == "loopback-usage"
                && balance.provider_id == "loopback"
                && balance.endpoint_id == "default"
        })
        .expect("published loopback provider balance");
    assert_eq!(published.total_balance_usd.as_deref(), Some("7.25"));

    drop(server);
    drop(usage_server);
    drop(scoped);
    std::fs::remove_dir_all(home).expect("remove helper home");
}

#[tokio::test]
async fn local_operator_client_batches_session_metadata_above_the_wire_limit() {
    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    let token =
        crate::local_operator::ensure_local_operator_token().expect("create operator token");
    let proxy = proxy_service(empty_proxy_config());
    let request_count = crate::dashboard_core::LOCAL_OPERATOR_SESSION_METADATA_BATCH_MAX + 1;
    let mut session_keys = Vec::with_capacity(request_count);
    for index in 0..request_count {
        let raw_session_id = format!("batched-local-session-{index}");
        proxy
            .state
            .begin_request(
                "codex",
                "POST",
                "/v1/responses",
                Some(raw_session_id.clone()),
                None,
                Some("codex-cli".to_string()),
                Some("127.0.0.1:43123".to_string()),
                Some(format!("/workspace/project-{index}")),
                Some("gpt-test".to_string()),
                None,
                None,
                u64::try_from(index).expect("session index"),
            )
            .await;
        session_keys
            .push(crate::dashboard_core::operator_summary::operator_session_key(&raw_session_id));
    }
    let server = spawn_admin_listener(proxy);
    let endpoint = ControlPlaneEndpoint::new(format!("http://{}", server.addr), None::<String>)
        .expect("loopback endpoint");
    let operator = LocalOperatorClient::new(endpoint, &token).expect("local operator client");

    let response = operator
        .read_operator_session_metadata(session_keys.clone())
        .await
        .expect("read all session metadata in bounded batches");

    assert_eq!(response.service_name, "codex");
    assert_eq!(response.sessions.len(), request_count);
    assert!(
        session_keys
            .iter()
            .all(|session_key| response.sessions.contains_key(session_key))
    );

    drop(server);
    drop(scoped);
    std::fs::remove_dir_all(home).expect("remove helper home");
}

#[tokio::test]
async fn signed_credential_refresh_publishes_only_for_matching_service_generation() {
    const ROTATED_CANARY: &str = "credential-canary-49173bf8d4ec4d5389281b13d09fd52c";

    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    crate::local_operator::ensure_local_operator_token().expect("create operator token");
    let install_generation = ServiceInstallGeneration::generate();
    let (proxy, control) = native_credential_proxy(Some(install_generation.clone()));
    assert_eq!(control.read_count(), 1);
    let server = spawn_admin_listener(proxy);
    let endpoint = ControlPlaneEndpoint::new(format!("http://{}", server.addr), None::<String>)
        .expect("loopback endpoint");
    let operator = LocalOperatorClient::from_helper_home(endpoint, &home)
        .expect("local operator client from selected helper home");
    let credential_name = CredentialName::parse("relay.primary").expect("credential name");

    control.set_value(SecretValue::new(ROTATED_CANARY.as_bytes().to_vec()).expect("canary"));
    let published = operator
        .refresh_native_credential(&LocalCredentialRefreshRequest {
            service: crate::config::ServiceKind::Codex,
            install_generation: install_generation.clone(),
            credential_name: credential_name.clone(),
            action: LocalCredentialRefreshAction::Upsert,
        })
        .await
        .expect("publish rotated credential");
    assert_eq!(published.status, LocalCredentialRefreshStatus::Published);
    assert_eq!(control.read_count(), 2);
    let rendered = format!(
        "{:?} {}",
        published,
        serde_json::to_string(&published).expect("serialize response")
    );
    assert!(!rendered.contains(ROTATED_CANARY));
    assert!(!rendered.contains("fingerprint"));

    let unchanged = operator
        .refresh_native_credential(&LocalCredentialRefreshRequest {
            service: crate::config::ServiceKind::Codex,
            install_generation: install_generation.clone(),
            credential_name: credential_name.clone(),
            action: LocalCredentialRefreshAction::Upsert,
        })
        .await
        .expect("refresh unchanged credential");
    assert_eq!(unchanged.status, LocalCredentialRefreshStatus::Unchanged);
    assert_eq!(unchanged.runtime_revision, published.runtime_revision);
    assert_eq!(control.read_count(), 3);

    control.set_missing();
    let degraded = operator
        .refresh_native_credential(&LocalCredentialRefreshRequest {
            service: crate::config::ServiceKind::Codex,
            install_generation: install_generation.clone(),
            credential_name: credential_name.clone(),
            action: LocalCredentialRefreshAction::Upsert,
        })
        .await
        .expect_err("stale last-known-good publication is not a successful rotation");
    assert!(matches!(
        &degraded,
        crate::control_plane_client::ControlPlaneError::HttpStatus { status: 503, .. }
    ));
    assert!(!degraded.to_string().contains(ROTATED_CANARY));
    assert_eq!(control.read_count(), 4);

    let not_referenced = operator
        .refresh_native_credential(&LocalCredentialRefreshRequest {
            service: crate::config::ServiceKind::Codex,
            install_generation: install_generation.clone(),
            credential_name: CredentialName::parse("relay.unused").expect("unused name"),
            action: LocalCredentialRefreshAction::Upsert,
        })
        .await
        .expect("report unreferenced credential");
    assert_eq!(
        not_referenced.status,
        LocalCredentialRefreshStatus::NotReferenced
    );
    assert_eq!(control.read_count(), 4);

    let deleted = operator
        .refresh_native_credential(&LocalCredentialRefreshRequest {
            service: crate::config::ServiceKind::Codex,
            install_generation,
            credential_name,
            action: LocalCredentialRefreshAction::Delete,
        })
        .await
        .expect("publish explicit delete");
    assert_eq!(deleted.status, LocalCredentialRefreshStatus::Published);
    assert_eq!(control.read_count(), 4);

    drop(server);
    drop(scoped);
    std::fs::remove_dir_all(home).expect("remove helper home");
}

#[tokio::test]
async fn signed_service_runtime_read_binds_identity_and_operator_model_atomically() {
    const CREDENTIAL_CANARY: &str = "service-runtime-canary-fb41804311c849a086438241d33b71fd";

    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let client_home = home.join("client");
    std::fs::create_dir_all(&client_home).expect("create client home");
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    crate::local_operator::ensure_local_operator_token().expect("create operator token");
    let install_generation = ServiceInstallGeneration::generate();
    let identity = ServiceRuntimeIdentity {
        service: crate::config::ServiceKind::Codex,
        helper_home: home.clone(),
        client_home: client_home.clone(),
        install_generation: install_generation.clone(),
    };
    let (proxy, control) = native_credential_proxy(Some(install_generation.clone()));
    control.set_value(SecretValue::new(CREDENTIAL_CANARY.as_bytes().to_vec()).expect("canary"));
    let server = spawn_admin_listener(proxy.with_service_runtime_identity(Some(identity.clone())));
    let endpoint = ControlPlaneEndpoint::new(format!("http://{}", server.addr), None::<String>)
        .expect("loopback endpoint");
    let operator = LocalOperatorClient::from_helper_home(endpoint, &home)
        .expect("local operator client from selected helper home");

    let response = operator
        .read_service_runtime(&LocalServiceRuntimeReadRequest {
            service: crate::config::ServiceKind::Codex,
            install_generation: install_generation.clone(),
        })
        .await
        .expect("read bound service runtime");
    assert_eq!(response.identity, identity);
    assert_eq!(response.operator.service_name, "codex");
    assert_eq!(
        response.credential_readiness,
        crate::credentials::CredentialAggregateReadiness::Ready
    );
    let rendered = format!(
        "{:?} {}",
        response,
        serde_json::to_string(&response).expect("serialize service runtime response")
    );
    assert!(!rendered.contains(CREDENTIAL_CANARY));
    assert!(!rendered.contains("fingerprint"));

    let stale = operator
        .read_service_runtime(&LocalServiceRuntimeReadRequest {
            service: crate::config::ServiceKind::Codex,
            install_generation: ServiceInstallGeneration::generate(),
        })
        .await
        .expect_err("reject stale receipt generation");
    assert!(matches!(
        stale,
        crate::control_plane_client::ControlPlaneError::HttpStatus { status: 409, .. }
    ));

    drop(server);
    drop(scoped);
    std::fs::remove_dir_all(home).expect("remove helper home");
}

#[tokio::test]
async fn credential_refresh_rejects_unbound_stale_or_foreign_target_before_native_read() {
    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    crate::local_operator::ensure_local_operator_token().expect("create operator token");
    let active_generation = ServiceInstallGeneration::generate();
    let (proxy, control) = native_credential_proxy(Some(active_generation.clone()));
    let server = spawn_admin_listener(proxy);
    let endpoint = ControlPlaneEndpoint::new(format!("http://{}", server.addr), None::<String>)
        .expect("loopback endpoint");
    let operator = LocalOperatorClient::from_helper_home(endpoint, &home)
        .expect("local operator client from selected helper home");
    control.set_value(
        SecretValue::new(b"credential-generation-b".to_vec()).expect("rotated credential"),
    );

    for request in [
        LocalCredentialRefreshRequest {
            service: crate::config::ServiceKind::Codex,
            install_generation: ServiceInstallGeneration::generate(),
            credential_name: CredentialName::parse("relay.primary").expect("credential name"),
            action: LocalCredentialRefreshAction::Upsert,
        },
        LocalCredentialRefreshRequest {
            service: crate::config::ServiceKind::Claude,
            install_generation: active_generation,
            credential_name: CredentialName::parse("relay.primary").expect("credential name"),
            action: LocalCredentialRefreshAction::Upsert,
        },
    ] {
        let error = operator
            .refresh_native_credential(&request)
            .await
            .expect_err("foreign target must be rejected");
        assert!(matches!(
            error,
            crate::control_plane_client::ControlPlaneError::HttpStatus { status: 409, .. }
        ));
    }
    assert_eq!(
        control.read_count(),
        1,
        "target mismatch must be rejected before native-store I/O"
    );

    drop(server);

    let (unbound_proxy, unbound_control) = native_credential_proxy(None);
    let unbound_server = spawn_admin_listener(unbound_proxy);
    let unbound_endpoint =
        ControlPlaneEndpoint::new(format!("http://{}", unbound_server.addr), None::<String>)
            .expect("unbound loopback endpoint");
    let unbound_operator = LocalOperatorClient::from_helper_home(unbound_endpoint, &home)
        .expect("unbound local operator client");
    let error = unbound_operator
        .refresh_native_credential(&LocalCredentialRefreshRequest {
            service: crate::config::ServiceKind::Codex,
            install_generation: ServiceInstallGeneration::generate(),
            credential_name: CredentialName::parse("relay.primary").expect("credential name"),
            action: LocalCredentialRefreshAction::Upsert,
        })
        .await
        .expect_err("a runtime without an install generation must be rejected");
    assert!(matches!(
        error,
        crate::control_plane_client::ControlPlaneError::HttpStatus { status: 409, .. }
    ));
    assert_eq!(
        unbound_control.read_count(),
        1,
        "an unbound runtime must be rejected before native-store I/O"
    );

    drop(unbound_server);
    drop(scoped);
    std::fs::remove_dir_all(home).expect("remove helper home");
}

#[tokio::test]
async fn credential_refresh_signature_is_single_use_and_rejects_expired_timestamp() {
    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    let token =
        crate::local_operator::ensure_local_operator_token().expect("create operator token");
    let generation = ServiceInstallGeneration::generate();
    let (proxy, control) = native_credential_proxy(Some(generation.clone()));
    control.set_value(
        SecretValue::new(b"credential-generation-b".to_vec()).expect("rotated credential"),
    );
    let server = spawn_admin_listener(proxy);
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build local client");
    let body = serde_json::to_vec(&LocalCredentialRefreshRequest {
        service: crate::config::ServiceKind::Codex,
        install_generation: generation,
        credential_name: CredentialName::parse("relay.primary").expect("credential name"),
        action: LocalCredentialRefreshAction::Upsert,
    })
    .expect("serialize credential refresh");

    let (client_nonce, session) = begin_operator_session(&client, &server, &token, None).await;
    let request = signed_operator_request(
        &SignedOperatorRequestContext {
            client: &client,
            server: &server,
            token: &token,
            client_nonce: &client_nonce,
            session: &session,
            admin_token: None,
        },
        LOCAL_V1_CREDENTIAL_REFRESH,
        &body,
        body.clone(),
        unix_time_ms(),
    )
    .build()
    .expect("build signed credential request");
    let replay = request.try_clone().expect("clone credential replay");
    assert_eq!(
        client
            .execute(request)
            .await
            .expect("send credential refresh")
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .execute(replay)
            .await
            .expect("send credential replay")
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(control.read_count(), 2);

    let (client_nonce, session) = begin_operator_session(&client, &server, &token, None).await;
    let expired = signed_operator_request(
        &SignedOperatorRequestContext {
            client: &client,
            server: &server,
            token: &token,
            client_nonce: &client_nonce,
            session: &session,
            admin_token: None,
        },
        LOCAL_V1_CREDENTIAL_REFRESH,
        &body,
        body.clone(),
        unix_time_ms().saturating_sub(60_000),
    )
    .send()
    .await
    .expect("send expired credential refresh");
    assert_eq!(expired.status(), StatusCode::FORBIDDEN);
    assert_eq!(control.read_count(), 2);

    let (client_nonce, session) = begin_operator_session(&client, &server, &token, None).await;
    let tampered_body = serde_json::to_vec(&LocalCredentialRefreshRequest {
        service: crate::config::ServiceKind::Codex,
        install_generation: ServiceInstallGeneration::generate(),
        credential_name: CredentialName::parse("relay.primary").expect("credential name"),
        action: LocalCredentialRefreshAction::Delete,
    })
    .expect("serialize tampered credential refresh");
    let invalid_signature = signed_operator_request(
        &SignedOperatorRequestContext {
            client: &client,
            server: &server,
            token: &token,
            client_nonce: &client_nonce,
            session: &session,
            admin_token: None,
        },
        LOCAL_V1_CREDENTIAL_REFRESH,
        &body,
        tampered_body,
        unix_time_ms(),
    )
    .send()
    .await
    .expect("send tampered credential refresh");
    assert_eq!(invalid_signature.status(), StatusCode::FORBIDDEN);
    assert_eq!(control.read_count(), 2);

    drop(server);
    drop(scoped);
    std::fs::remove_dir_all(home).expect("remove helper home");
}

#[tokio::test]
async fn signed_local_operator_routing_mutations_enforce_cas_and_update_read_model() {
    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    let token =
        crate::local_operator::ensure_local_operator_token().expect("create operator token");
    let server = spawn_admin_listener(proxy_service(routing_proxy_config()));
    let endpoint = ControlPlaneEndpoint::new(format!("http://{}", server.addr), None::<String>)
        .expect("loopback endpoint");
    let operator = LocalOperatorClient::new(endpoint, &token).expect("local operator client");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build local client");
    let initial_model = read_operator_model(&client, &server).await;
    let initial = initial_model
        .data
        .as_ref()
        .and_then(|data| data.routing.as_ref())
        .expect("initial routing summary")
        .clone();

    let preferred = operator
        .mutate_operator_routing(&routing_mutation_request(
            &initial,
            OperatorRoutingCommand::SetNewSessionPreference {
                provider_id: "input".to_string(),
                endpoint_id: "default".to_string(),
            },
        ))
        .await
        .expect("set new-session preference");
    assert_eq!(preferred.status, OperatorRoutingMutationStatus::Applied);
    assert_eq!(
        preferred
            .routing
            .new_session_preference
            .as_ref()
            .map(|target| (target.provider_id.as_str(), target.endpoint_id.as_str())),
        Some(("input", "default"))
    );

    let stale_control = operator
        .mutate_operator_routing(&routing_mutation_request(
            &initial,
            OperatorRoutingCommand::ClearNewSessionPreference,
        ))
        .await
        .expect("stale control request");
    assert_eq!(
        stale_control.status,
        OperatorRoutingMutationStatus::Conflict
    );
    assert!(stale_control.routing.new_session_preference.is_some());

    let cleared = operator
        .mutate_operator_routing(&routing_mutation_request(
            &preferred.routing,
            OperatorRoutingCommand::ClearNewSessionPreference,
        ))
        .await
        .expect("clear new-session preference");
    assert_eq!(cleared.status, OperatorRoutingMutationStatus::Applied);
    assert!(cleared.routing.new_session_preference.is_none());

    let drained = operator
        .mutate_operator_routing(&routing_mutation_request(
            &cleared.routing,
            OperatorRoutingCommand::SetEndpointMode {
                provider_id: "ciii".to_string(),
                endpoint_id: "default".to_string(),
                mode: OperatorEndpointMode::Draining,
            },
        ))
        .await
        .expect("drain endpoint");
    assert_eq!(drained.status, OperatorRoutingMutationStatus::Applied);
    assert!(drained.routing.provider_policy_revision > cleared.routing.provider_policy_revision);

    let stale_policy = operator
        .mutate_operator_routing(&routing_mutation_request(
            &cleared.routing,
            OperatorRoutingCommand::SetEndpointMode {
                provider_id: "ciii".to_string(),
                endpoint_id: "default".to_string(),
                mode: OperatorEndpointMode::Disabled,
            },
        ))
        .await
        .expect("stale policy request");
    assert_eq!(stale_policy.status, OperatorRoutingMutationStatus::Conflict);

    let invalid = operator
        .mutate_operator_routing(&routing_mutation_request(
            &drained.routing,
            OperatorRoutingCommand::SetNewSessionPreference {
                provider_id: "missing".to_string(),
                endpoint_id: "default".to_string(),
            },
        ))
        .await
        .expect_err("invalid candidate must be rejected");
    assert!(
        matches!(
            invalid,
            crate::control_plane_client::ControlPlaneError::HttpStatus { status: 400, .. }
        ),
        "unexpected invalid-candidate error: {invalid}"
    );

    let refreshed = read_operator_model(&client, &server).await;
    let data = refreshed.data.as_ref().expect("refreshed operator data");
    let routing = data.routing.as_ref().expect("refreshed routing summary");
    assert_eq!(
        routing.provider_policy_revision,
        drained.routing.provider_policy_revision
    );
    assert!(routing.new_session_preference.is_none());
    let ciii = data
        .summary
        .providers
        .iter()
        .find(|provider| provider.name == "ciii")
        .and_then(|provider| {
            provider
                .endpoints
                .iter()
                .find(|endpoint| endpoint.name == "default")
        })
        .expect("ciii endpoint summary");
    assert_eq!(ciii.runtime_state, RuntimeConfigState::Draining);
    assert_eq!(
        ciii.runtime_state_override,
        Some(RuntimeConfigState::Draining)
    );

    drop(server);
    drop(scoped);
    std::fs::remove_dir_all(home).expect("remove helper home");
}

#[tokio::test]
async fn signed_local_operator_session_affinity_mutations_enforce_cas_and_update_read_model() {
    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    let token =
        crate::local_operator::ensure_local_operator_token().expect("create operator token");
    let proxy = proxy_service(routing_proxy_config());
    let raw_session_id = "session-affinity-control-secret";
    observe_idle_session(&proxy, raw_session_id).await;
    seed_session_affinity(&proxy, raw_session_id, "input").await;

    let server = spawn_admin_listener(proxy);
    let endpoint = ControlPlaneEndpoint::new(format!("http://{}", server.addr), None::<String>)
        .expect("loopback endpoint");
    let operator = LocalOperatorClient::new(endpoint, &token).expect("local operator client");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build local client");
    let initial_model = read_operator_model(&client, &server).await;
    let initial_data = initial_model.data.as_ref().expect("initial operator data");
    let routing = initial_data
        .routing
        .as_ref()
        .expect("initial routing summary");
    let session = initial_data
        .summary
        .sessions
        .iter()
        .find(|session| session.route_affinity.is_some())
        .expect("session with route affinity");
    let initial_affinity = session
        .route_affinity
        .as_ref()
        .expect("initial route affinity");
    assert!(!routing.route_graph_key.is_empty());
    assert_eq!(initial_affinity.provider_id, "input");
    assert!(
        !serde_json::to_string(&initial_model)
            .expect("serialize operator model")
            .contains(raw_session_id)
    );

    let rebound = operator
        .mutate_operator_session_affinity(&OperatorSessionAffinityMutationRequest {
            session_key: session.session_key.clone(),
            expected_affinity_revision: Some(initial_affinity.revision.clone()),
            command: OperatorSessionAffinityCommand::Rebind {
                provider_id: "ciii".to_string(),
                endpoint_id: "default".to_string(),
            },
        })
        .await
        .expect("rebind idle session affinity");
    assert_eq!(
        rebound.status,
        OperatorSessionAffinityMutationStatus::Applied
    );
    let rebound_affinity = rebound.route_affinity.as_ref().expect("rebound affinity");
    assert_eq!(rebound_affinity.provider_id, "ciii");
    assert_eq!(rebound_affinity.change_reason, "operator_rebind");

    let stale_clear = operator
        .mutate_operator_session_affinity(&OperatorSessionAffinityMutationRequest {
            session_key: session.session_key.clone(),
            expected_affinity_revision: Some(initial_affinity.revision.clone()),
            command: OperatorSessionAffinityCommand::Clear,
        })
        .await
        .expect("stale clear request");
    assert_eq!(
        stale_clear.status,
        OperatorSessionAffinityMutationStatus::Conflict
    );
    assert_eq!(
        stale_clear
            .route_affinity
            .as_ref()
            .map(|affinity| affinity.provider_id.as_str()),
        Some("ciii")
    );

    let cleared = operator
        .mutate_operator_session_affinity(&OperatorSessionAffinityMutationRequest {
            session_key: session.session_key.clone(),
            expected_affinity_revision: Some(rebound_affinity.revision.clone()),
            command: OperatorSessionAffinityCommand::Clear,
        })
        .await
        .expect("clear rebound affinity");
    assert_eq!(
        cleared.status,
        OperatorSessionAffinityMutationStatus::Applied
    );
    assert!(cleared.route_affinity.is_none());

    let refreshed = read_operator_model(&client, &server).await;
    let refreshed_session = refreshed
        .data
        .as_ref()
        .expect("refreshed operator data")
        .summary
        .sessions
        .iter()
        .find(|candidate| candidate.session_key == session.session_key)
        .expect("refreshed session");
    assert!(refreshed_session.route_affinity.is_none());

    drop(server);
    drop(scoped);
    std::fs::remove_dir_all(home).expect("remove helper home");
}

#[tokio::test]
async fn signed_session_binding_mutation_is_cas_guarded_and_applies_to_the_next_request() {
    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    let token =
        crate::local_operator::ensure_local_operator_token().expect("create local operator token");
    let (upstream_body_tx, mut upstream_body_rx) = tokio::sync::mpsc::unbounded_channel();
    let upstream = spawn_test_upstream(axum::Router::new().route(
        "/v1/responses",
        post(move |Json(body): Json<serde_json::Value>| {
            let upstream_body_tx = upstream_body_tx.clone();
            async move {
                upstream_body_tx.send(body).expect("capture upstream body");
                Json(serde_json::json!({"id":"resp_test","status":"completed"}))
            }
        }),
    ));
    let mut config = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([(
                "input".to_string(),
                ProviderConfig {
                    base_url: Some(upstream.base_url()),
                    ..ProviderConfig::default()
                },
            )]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "input".to_string(),
            ])),
            profiles: std::collections::BTreeMap::from([(
                "daily".to_string(),
                ServiceControlProfile {
                    model: Some("gpt-5.4".to_string()),
                    reasoning_effort: Some("high".to_string()),
                    service_tier: Some("fast".to_string()),
                    ..ServiceControlProfile::default()
                },
            )]),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    config.codex.default_profile = None;
    let proxy = proxy_service(config);
    let public_server = spawn_proxy_service(proxy.clone());
    let admin_server = spawn_admin_listener(proxy);
    let endpoint =
        ControlPlaneEndpoint::new(format!("http://{}", admin_server.addr), None::<String>)
            .expect("loopback endpoint");
    let operator = LocalOperatorClient::new(endpoint, &token).expect("local operator client");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build local client");
    let raw_session_id = "session-binding-next-request-secret";

    let initial = post_responses_json(
        &client,
        &public_server,
        serde_json::json!({
            "model": "gpt-5",
            "prompt_cache_key": raw_session_id,
            "stream": false
        })
        .to_string(),
    )
    .await;
    assert_eq!(initial.status(), StatusCode::OK);
    let initial_body = upstream_body_rx
        .recv()
        .await
        .expect("initial upstream body");
    assert_eq!(initial_body["model"].as_str(), Some("gpt-5"));

    let initial_model = read_operator_model(&client, &admin_server).await;
    let session = initial_model
        .data
        .as_ref()
        .expect("operator data")
        .summary
        .sessions
        .iter()
        .find(|session| session.last_model.as_deref() == Some("gpt-5"))
        .expect("observed session")
        .clone();
    assert!(!session.binding.revision.is_empty());
    assert!(
        !serde_json::to_string(&initial_model)
            .expect("serialize read model")
            .contains(raw_session_id)
    );

    let applied = operator
        .mutate_operator_session_binding(&OperatorSessionBindingMutationRequest {
            session_key: session.session_key.clone(),
            expected_binding_revision: session.binding.revision.clone(),
            command: OperatorSessionBindingCommand::SetProfile {
                profile_name: Some("daily".to_string()),
            },
        })
        .await
        .expect("apply session profile");
    assert_eq!(
        applied.status,
        OperatorSessionBindingMutationStatus::Applied
    );
    assert_eq!(applied.binding.profile_name.as_deref(), Some("daily"));
    assert_eq!(applied.binding.service_tier.as_deref(), Some("priority"));

    let stale = operator
        .mutate_operator_session_binding(&OperatorSessionBindingMutationRequest {
            session_key: session.session_key.clone(),
            expected_binding_revision: session.binding.revision.clone(),
            command: OperatorSessionBindingCommand::ResetManualOverrides,
        })
        .await
        .expect("stale binding mutation response");
    assert_eq!(stale.status, OperatorSessionBindingMutationStatus::Conflict);

    let next = post_responses_json(
        &client,
        &public_server,
        serde_json::json!({
            "model": "gpt-5",
            "prompt_cache_key": raw_session_id,
            "reasoning": {"effort":"low"},
            "service_tier": "default",
            "stream": false
        })
        .to_string(),
    )
    .await;
    assert_eq!(next.status(), StatusCode::OK);
    let next_body = upstream_body_rx.recv().await.expect("next upstream body");
    assert_eq!(next_body["model"].as_str(), Some("gpt-5.4"));
    assert_eq!(next_body["reasoning"]["effort"].as_str(), Some("high"));
    assert_eq!(next_body["service_tier"].as_str(), Some("priority"));

    drop(admin_server);
    drop(public_server);
    drop(upstream);
    drop(scoped);
    std::fs::remove_dir_all(home).expect("remove helper home");
}

#[tokio::test]
async fn conditional_route_affinity_can_be_cleared_but_not_rebound() {
    let proxy = proxy_service(conditional_routing_proxy_config());
    let raw_session_id = "conditional-affinity-control";
    observe_idle_session(&proxy, raw_session_id).await;
    seed_session_affinity(&proxy, raw_session_id, "input").await;

    let capture = proxy
        .operator_read_capture()
        .await
        .expect("operator read capture");
    let session = capture
        .model
        .data
        .as_ref()
        .expect("operator data")
        .summary
        .sessions
        .iter()
        .find(|session| session.route_affinity.is_some())
        .expect("session affinity summary");
    let affinity = session.route_affinity.as_ref().expect("route affinity");

    let rebind_error = proxy
        .mutate_operator_session_affinity(OperatorSessionAffinityMutationRequest {
            session_key: session.session_key.clone(),
            expected_affinity_revision: Some(affinity.revision.clone()),
            command: OperatorSessionAffinityCommand::Rebind {
                provider_id: "ciii".to_string(),
                endpoint_id: "default".to_string(),
            },
        })
        .await
        .expect_err("conditional rebind must be rejected");
    assert_eq!(rebind_error.status(), StatusCode::CONFLICT);
    assert!(rebind_error.message().contains("conditional"));

    let cleared = proxy
        .mutate_operator_session_affinity(OperatorSessionAffinityMutationRequest {
            session_key: session.session_key.clone(),
            expected_affinity_revision: Some(affinity.revision.clone()),
            command: OperatorSessionAffinityCommand::Clear,
        })
        .await
        .expect("conditional affinity clear");
    assert_eq!(
        cleared.status,
        OperatorSessionAffinityMutationStatus::Applied
    );
    assert!(cleared.route_affinity.is_none());
}
