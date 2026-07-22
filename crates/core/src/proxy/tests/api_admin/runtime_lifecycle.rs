use super::*;
use crate::proxy::tests::harness::upstream_config;

fn spawn_axum_server_with_graceful_shutdown(
    app: axum::Router,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    listener.set_nonblocking(true).expect("nonblocking");
    let listener = tokio::net::TcpListener::from_std(listener).expect("to tokio listener");
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            if !*shutdown_rx.borrow() {
                let _ = shutdown_rx.changed().await;
            }
        })
        .await
        .expect("serve");
    });
    (addr, handle)
}

async fn signed_shutdown_fixture(
    policy: crate::local_operator::LocalRuntimeShutdownPolicy,
    request_service_name: &str,
    request_port: u16,
) -> (
    Result<
        crate::local_operator::LocalRuntimeShutdownResponse,
        crate::control_plane_client::ControlPlaneError,
    >,
    bool,
) {
    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    let token =
        crate::local_operator::ensure_local_operator_token().expect("create local operator token");
    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(make_helper_config(
            vec![upstream_config("http://127.0.0.1:9/v1")],
            RetryConfig::default(),
        )),
        "codex",
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let proxy = proxy.with_local_runtime_shutdown(3211, policy, shutdown_tx);
    let app = crate::proxy::admin_listener_router(proxy);
    let (addr, handle) = spawn_axum_server(app);
    let endpoint = crate::control_plane_client::ControlPlaneEndpoint::new(
        format!("http://{addr}"),
        None::<String>,
    )
    .expect("loopback endpoint");
    let operator = crate::control_plane_client::LocalOperatorClient::new(endpoint, &token)
        .expect("local operator client");
    let result = operator
        .shutdown_runtime(&crate::local_operator::LocalRuntimeShutdownRequest {
            service_name: request_service_name.to_string(),
            proxy_port: request_port,
        })
        .await;
    let shutdown_requested = *shutdown_rx.borrow();
    handle.abort();
    let _ = std::fs::remove_dir_all(home);
    (result, shutdown_requested)
}

#[tokio::test]
async fn runtime_shutdown_mutation_route_is_absent() {
    let cfg = make_helper_config(
        vec![upstream_config("http://127.0.0.1:9/v1")],
        RetryConfig::default(),
    );
    let proxy = ProxyService::new(Client::new(), Arc::new(cfg), "codex");
    let app = crate::proxy::router(proxy);
    let mut request = Request::builder()
        .method("POST")
        .uri("/__codex_helper/api/v1/runtime/shutdown")
        .body(Body::empty())
        .expect("build shutdown request");
    request
        .extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            42_114,
        ))));

    let response = app.oneshot(request).await.expect("shutdown response");

    assert!(matches!(
        response.status(),
        StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
    ));
}

#[tokio::test]
async fn signed_local_shutdown_stops_only_the_targeted_manual_resident_runtime() {
    let (result, shutdown_requested) = signed_shutdown_fixture(
        crate::local_operator::LocalRuntimeShutdownPolicy::ManualResident,
        "codex",
        3211,
    )
    .await;

    let response = result.expect("manual resident shutdown should be accepted");
    assert!(response.accepted);
    assert_eq!(response.service_name, "codex");
    assert_eq!(response.proxy_port, 3211);
    assert!(shutdown_requested);
}

#[tokio::test]
async fn signed_local_shutdown_returns_acceptance_before_graceful_server_exit() {
    let _env_guard = env_lock().await;
    let home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", &home);
        scoped.set(ADMIN_TOKEN_ENV_VAR, "");
    }
    let token =
        crate::local_operator::ensure_local_operator_token().expect("create local operator token");
    let proxy = ProxyService::new(
        Client::new(),
        Arc::new(make_helper_config(
            vec![upstream_config("http://127.0.0.1:9/v1")],
            RetryConfig::default(),
        )),
        "codex",
    );
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let app = crate::proxy::admin_listener_router(proxy.with_local_runtime_shutdown(
        3211,
        crate::local_operator::LocalRuntimeShutdownPolicy::ManualResident,
        shutdown_tx,
    ));
    let (addr, handle) = spawn_axum_server_with_graceful_shutdown(app, shutdown_rx.clone());
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build local client");
    let client_nonce = crate::local_operator::new_local_operator_nonce();
    let session_timestamp_ms = crate::local_operator::unix_time_ms();
    let proof = crate::local_operator::local_operator_client_proof(
        &token,
        &client_nonce,
        session_timestamp_ms,
    )
    .expect("sign local operator session");
    let session = client
        .post(format!(
            "http://{addr}{}",
            crate::proxy::LOCAL_V1_OPERATOR_SESSION
        ))
        .json(&crate::local_operator::LocalOperatorSessionRequest {
            client_nonce: client_nonce.clone(),
            timestamp_ms: session_timestamp_ms,
            proof,
        })
        .send()
        .await
        .expect("begin local operator session")
        .error_for_status()
        .expect("local operator session status")
        .json::<crate::local_operator::LocalOperatorSessionResponse>()
        .await
        .expect("decode local operator session");
    crate::local_operator::verify_local_operator_server_proof(&token, &client_nonce, &session)
        .expect("verify local operator server proof");
    let body = serde_json::to_vec(&crate::local_operator::LocalRuntimeShutdownRequest {
        service_name: "codex".to_string(),
        proxy_port: 3211,
    })
    .expect("serialize shutdown request");
    let request_nonce = crate::local_operator::new_local_operator_nonce();
    let request_timestamp_ms = crate::local_operator::unix_time_ms();
    let signature = crate::local_operator::local_operator_request_signature(
        &token,
        &client_nonce,
        &session.session_id,
        &request_nonce,
        request_timestamp_ms,
        crate::proxy::LOCAL_V1_RUNTIME_SHUTDOWN,
        &body,
    )
    .expect("sign shutdown request");
    let request = client
        .post(format!(
            "http://{addr}{}",
            crate::proxy::LOCAL_V1_RUNTIME_SHUTDOWN
        ))
        .header(
            crate::proxy::LOCAL_OPERATOR_SESSION_HEADER,
            &session.session_id,
        )
        .header(crate::proxy::LOCAL_OPERATOR_NONCE_HEADER, request_nonce)
        .header(
            crate::proxy::LOCAL_OPERATOR_TIMESTAMP_HEADER,
            request_timestamp_ms.to_string(),
        )
        .header(crate::proxy::LOCAL_OPERATOR_SIGNATURE_HEADER, signature)
        .header("content-type", "application/json")
        .body(body)
        .build()
        .expect("build signed shutdown request");
    let accepted = client
        .execute(request)
        .await
        .expect("receive shutdown acceptance")
        .error_for_status()
        .expect("shutdown acceptance status")
        .json::<crate::local_operator::LocalRuntimeShutdownResponse>()
        .await
        .expect("decode shutdown acceptance before observing graceful exit");
    assert!(accepted.accepted);
    if !*shutdown_rx.borrow() {
        shutdown_rx
            .changed()
            .await
            .expect("observe graceful shutdown after acceptance");
    }
    assert!(*shutdown_rx.borrow());

    tokio::time::timeout(std::time::Duration::from_secs(2), handle)
        .await
        .expect("graceful server must exit after returning shutdown acceptance")
        .expect("join graceful server");
    let _ = std::fs::remove_dir_all(home);
}

#[tokio::test]
async fn signed_local_shutdown_rejects_non_resident_runtime_ownership() {
    for (policy, guidance) in [
        (
            crate::local_operator::LocalRuntimeShutdownPolicy::ForegroundProcess,
            "foreground process",
        ),
        (
            crate::local_operator::LocalRuntimeShutdownPolicy::SupervisorManaged,
            "daemon supervise",
        ),
        (
            crate::local_operator::LocalRuntimeShutdownPolicy::SystemService,
            "service stop",
        ),
        (
            crate::local_operator::LocalRuntimeShutdownPolicy::DesktopManaged,
            "Stop Proxy",
        ),
    ] {
        let (result, shutdown_requested) = signed_shutdown_fixture(policy, "codex", 3211).await;
        let error = result.expect_err("managed runtime shutdown must be rejected");
        assert!(
            error.to_string().contains(guidance),
            "missing {guidance:?} guidance: {error}"
        );
        assert!(!shutdown_requested);
    }
}

#[tokio::test]
async fn signed_local_shutdown_rejects_target_identity_mismatch() {
    for (service_name, proxy_port) in [("claude", 3211), ("codex", 3210)] {
        let (result, shutdown_requested) = signed_shutdown_fixture(
            crate::local_operator::LocalRuntimeShutdownPolicy::ManualResident,
            service_name,
            proxy_port,
        )
        .await;
        let error = result.expect_err("target mismatch must be rejected");
        assert!(
            error.to_string().contains("target does not match"),
            "{error}"
        );
        assert!(!shutdown_requested);
    }
}
