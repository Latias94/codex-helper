use super::*;
use crate::control_plane_client::{ControlPlaneEndpoint, LocalOperatorClient};
use crate::dashboard_core::{OperatorReadModel, OperatorRoutingSummary};
use crate::local_operator::{
    LocalOperatorSessionRequest, LocalOperatorSessionResponse, local_operator_client_proof,
    local_operator_request_signature, new_local_operator_nonce, unix_time_ms,
    verify_local_operator_server_proof,
};
use crate::proxy::tests::harness::{TestProxyServer, proxy_service};
use crate::proxy::{
    LOCAL_OPERATOR_NONCE_HEADER, LOCAL_OPERATOR_SESSION_HEADER, LOCAL_OPERATOR_SIGNATURE_HEADER,
    LOCAL_OPERATOR_TIMESTAMP_HEADER, LOCAL_V1_BALANCE_REFRESH, LOCAL_V1_OPERATOR_SESSION,
    OperatorEndpointMode, OperatorRoutingCommand, OperatorRoutingMutationRequest,
    OperatorRoutingMutationStatus, OperatorSessionAffinityCommand,
    OperatorSessionAffinityMutationRequest, OperatorSessionAffinityMutationStatus,
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
    let request_nonce = new_local_operator_nonce();
    let timestamp_ms = unix_time_ms();
    let signature = local_operator_request_signature(
        context.token,
        context.client_nonce,
        &context.session.session_id,
        &request_nonce,
        timestamp_ms,
        LOCAL_V1_BALANCE_REFRESH,
        body_to_sign,
    )
    .expect("sign local operator request");
    let mut request = context
        .client
        .post(context.server.url(LOCAL_V1_BALANCE_REFRESH))
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
    let server = spawn_admin_listener(proxy.clone());

    let endpoint = ControlPlaneEndpoint::new(format!("http://{}", server.addr), None::<String>)
        .expect("loopback endpoint");
    let operator = LocalOperatorClient::new(endpoint, &token).expect("local operator client");
    let response = operator
        .refresh_provider_balances(true)
        .await
        .expect("signed balance refresh");
    assert_eq!(response.service_name, "codex");

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
