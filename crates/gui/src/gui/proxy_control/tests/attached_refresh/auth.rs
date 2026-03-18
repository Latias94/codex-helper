use super::*;

#[test]
fn refresh_attached_sends_admin_token_when_configured() {
    let _env_lock = env_lock();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set(crate::proxy::ADMIN_TOKEN_ENV_VAR, "gui-secret");
    }

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let observed_headers = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "shared_capabilities": {
            "session_observability": true,
            "request_history": true
        },
        "host_local_capabilities": {
            "session_history": false,
            "transcript_read": false,
            "cwd_enrichment": false
        },
        "remote_admin_access": {
            "loopback_without_token": true,
            "remote_requires_token": true,
            "remote_enabled": true,
            "token_header": crate::proxy::ADMIN_TOKEN_HEADER,
            "token_env_var": crate::proxy::ADMIN_TOKEN_ENV_VAR
        },
        "endpoints": [
            "/__codex_helper/api/v1/snapshot"
        ]
    });
    let snapshot = sample_snapshot(vec![sample_station("alpha")]);
    let app = Router::new()
        .route(
            "/__codex_helper/api/v1/capabilities",
            get({
                let caps = caps.clone();
                let observed_headers = observed_headers.clone();
                move |headers: HeaderMap| {
                    let caps = caps.clone();
                    let observed_headers = observed_headers.clone();
                    async move {
                        observed_headers.lock().expect("header lock").push(
                            headers
                                .get(crate::proxy::ADMIN_TOKEN_HEADER)
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_string),
                        );
                        Json(caps)
                    }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/snapshot",
            get({
                let snapshot = snapshot.clone();
                let observed_headers = observed_headers.clone();
                move |headers: HeaderMap| {
                    let snapshot = snapshot.clone();
                    let observed_headers = observed_headers.clone();
                    async move {
                        observed_headers.lock().expect("header lock").push(
                            headers
                                .get(crate::proxy::ADMIN_TOKEN_HEADER)
                                .and_then(|value| value.to_str().ok())
                                .map(str::to_string),
                        );
                        Json(snapshot)
                    }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4250, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4250, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let observed_headers = observed_headers.lock().expect("header lock").clone();
    assert!(!observed_headers.is_empty());
    assert!(
        observed_headers
            .iter()
            .all(|value| value.as_deref() == Some("gui-secret"))
    );
    assert!(
        controller
            .attached()
            .expect("attached status")
            .remote_admin_access
            .remote_enabled
    );

    handle.abort();
}
