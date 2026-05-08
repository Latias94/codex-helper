use super::helpers::spawn_test_server;
use super::*;

#[test]
fn attached_persisted_station_settings_use_v1_station_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let active_payload = Arc::new(Mutex::new(None::<Value>));
    let update_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new()
        .route(
            "/__codex_helper/api/v1/stations/active",
            post({
                let active_payload = active_payload.clone();
                move |Json(payload): Json<Value>| {
                    let active_payload = active_payload.clone();
                    async move {
                        *active_payload.lock().expect("active payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/stations/alpha",
            put({
                let update_payload = update_payload.clone();
                move |Json(payload): Json<Value>| {
                    let update_payload = update_payload.clone();
                    async move {
                        *update_payload.lock().expect("update payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4302, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4302);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_persisted_station_settings = true;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .set_persisted_active_station(&rt, Some("alpha".to_string()))
        .expect("set persisted active station");
    controller
        .update_persisted_station(&rt, "alpha".to_string(), false, 7)
        .expect("update persisted station");

    let active_payload = active_payload
        .lock()
        .expect("active payload lock")
        .clone()
        .expect("active payload");
    assert_eq!(
        active_payload.get("station_name"),
        Some(&Value::String("alpha".to_string()))
    );

    let update_payload = update_payload
        .lock()
        .expect("update payload lock")
        .clone()
        .expect("update payload");
    assert_eq!(update_payload.get("enabled"), Some(&Value::Bool(false)));
    assert_eq!(update_payload.get("level"), Some(&Value::from(7)));

    handle.abort();
}

#[test]
fn attached_persisted_retry_config_uses_v1_retry_endpoint() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new().route(
        "/__codex_helper/api/v1/retry/config",
        post({
            let observed_payload = observed_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_payload = observed_payload.clone();
                async move {
                    *observed_payload.lock().expect("retry payload lock") = Some(payload.clone());
                    Json(serde_json::json!({
                        "configured": payload,
                        "resolved": {
                            "upstream": {
                                "max_attempts": 2,
                                "backoff_ms": 200,
                                "backoff_max_ms": 2000,
                                "jitter_ms": 100,
                                "on_status": "429,500-599,524",
                                "on_class": ["upstream_transport_error"],
                                "strategy": "same_upstream"
                            },
                            "route": {
                                "max_attempts": 2,
                                "backoff_ms": 0,
                                "backoff_max_ms": 0,
                                "jitter_ms": 0,
                                "on_status": "401,403,404,408,429,500-599,524",
                                "on_class": ["upstream_transport_error"],
                                "strategy": "failover"
                            },
                            "allow_cross_station_before_first_output": true,
                            "never_on_status": "413,415,422",
                            "never_on_class": ["client_error_non_retryable"],
                            "cloudflare_challenge_cooldown_secs": 300,
                            "cloudflare_timeout_cooldown_secs": 12,
                            "transport_cooldown_secs": 45,
                            "cooldown_backoff_factor": 3,
                            "cooldown_backoff_max_secs": 180
                        }
                    }))
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4303, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4303);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_retry_config_api = true;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .set_persisted_retry_config(
            &rt,
            RetryConfig {
                profile: Some(crate::config::RetryProfileName::CostPrimary),
                transport_cooldown_secs: Some(45),
                cloudflare_timeout_cooldown_secs: Some(12),
                cooldown_backoff_factor: Some(3),
                cooldown_backoff_max_secs: Some(180),
                ..Default::default()
            },
        )
        .expect("set persisted retry config");

    let observed_payload = observed_payload
        .lock()
        .expect("retry payload lock")
        .clone()
        .expect("retry payload");
    assert_eq!(
        observed_payload.get("profile"),
        Some(&Value::String("cost-primary".to_string()))
    );
    assert_eq!(
        observed_payload.get("transport_cooldown_secs"),
        Some(&Value::from(45))
    );
    assert_eq!(
        observed_payload.get("cooldown_backoff_factor"),
        Some(&Value::from(3))
    );

    let snapshot = controller.snapshot().expect("snapshot");
    assert_eq!(
        snapshot
            .configured_retry
            .as_ref()
            .and_then(|retry| retry.profile),
        Some(crate::config::RetryProfileName::CostPrimary)
    );
    assert_eq!(
        snapshot
            .resolved_retry
            .as_ref()
            .map(|retry| retry.transport_cooldown_secs),
        Some(45)
    );
    assert_eq!(
        snapshot
            .resolved_retry
            .as_ref()
            .map(|retry| retry.cooldown_backoff_factor),
        Some(3)
    );
    assert_eq!(
        snapshot
            .resolved_retry
            .as_ref()
            .map(|retry| retry.allow_cross_station_before_first_output),
        Some(true)
    );

    handle.abort();
}

#[test]
fn attached_persisted_retry_config_uses_operator_summary_link() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new().route(
        "/__alt/v1/retry/config",
        post({
            let observed_payload = observed_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_payload = observed_payload.clone();
                async move {
                    *observed_payload.lock().expect("retry payload lock") = Some(payload.clone());
                    Json(serde_json::json!({
                        "configured": payload,
                        "resolved": {
                            "upstream": {
                                "max_attempts": 2,
                                "backoff_ms": 100,
                                "backoff_max_ms": 1000,
                                "jitter_ms": 50,
                                "on_status": "429,500-599,524",
                                "on_class": ["upstream_transport_error"],
                                "strategy": "same_upstream"
                            },
                            "route": {
                                "max_attempts": 2,
                                "backoff_ms": 0,
                                "backoff_max_ms": 0,
                                "jitter_ms": 0,
                                "on_status": "401,403,404,408,429,500-599,524",
                                "on_class": ["upstream_transport_error"],
                                "strategy": "failover"
                            },
                            "allow_cross_station_before_first_output": false,
                            "never_on_status": "413,415,422",
                            "never_on_class": ["client_error_non_retryable"],
                            "cloudflare_challenge_cooldown_secs": 300,
                            "cloudflare_timeout_cooldown_secs": 12,
                            "transport_cooldown_secs": 45,
                            "cooldown_backoff_factor": 2,
                            "cooldown_backoff_max_secs": 120
                        }
                    }))
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4310, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4310);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_retry_config_api = true;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        retry_config: "/__alt/v1/retry/config".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    controller
        .set_persisted_retry_config(
            &rt,
            RetryConfig {
                profile: Some(crate::config::RetryProfileName::Balanced),
                transport_cooldown_secs: Some(45),
                ..Default::default()
            },
        )
        .expect("set persisted retry config via operator summary link");

    let observed_payload = observed_payload
        .lock()
        .expect("retry payload lock")
        .clone()
        .expect("retry payload");
    assert_eq!(
        observed_payload.get("profile"),
        Some(&Value::String("balanced".to_string()))
    );
    assert_eq!(
        observed_payload.get("transport_cooldown_secs"),
        Some(&Value::from(45))
    );

    handle.abort();
}

#[test]
fn attached_runtime_default_profile_uses_operator_summary_link() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new().route(
        "/__alt/v1/profiles/default",
        post({
            let observed_payload = observed_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_payload = observed_payload.clone();
                async move {
                    *observed_payload
                        .lock()
                        .expect("default profile payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4311, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4311);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_default_profile_override = true;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        default_profile: "/__alt/v1/profiles/default".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    controller
        .set_default_profile(&rt, Some("fast".to_string()))
        .expect("set runtime default profile via operator summary link");

    let observed_payload = observed_payload
        .lock()
        .expect("default profile payload lock")
        .clone()
        .expect("default profile payload");
    assert_eq!(
        observed_payload.get("profile_name"),
        Some(&Value::String("fast".to_string()))
    );

    handle.abort();
}

#[test]
fn attached_persisted_profile_crud_uses_operator_summary_links() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let default_payload = Arc::new(Mutex::new(None::<Value>));
    let upsert_payload = Arc::new(Mutex::new(None::<Value>));
    let delete_hits = Arc::new(Mutex::new(0usize));
    let app = Router::new()
        .route(
            "/__alt/v1/profiles/default/persisted",
            post({
                let default_payload = default_payload.clone();
                move |Json(payload): Json<Value>| {
                    let default_payload = default_payload.clone();
                    async move {
                        *default_payload
                            .lock()
                            .expect("persisted default payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        )
        .route(
            "/__alt/v1/profiles/fast",
            put({
                let upsert_payload = upsert_payload.clone();
                move |Json(payload): Json<Value>| {
                    let upsert_payload = upsert_payload.clone();
                    async move {
                        *upsert_payload.lock().expect("profile payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            })
            .delete({
                let delete_hits = delete_hits.clone();
                move || {
                    let delete_hits = delete_hits.clone();
                    async move {
                        *delete_hits.lock().expect("profile delete hits lock") += 1;
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4312, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4312);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        persisted_default_profile: "/__alt/v1/profiles/default/persisted".to_string(),
        profile_by_name_template: "/__alt/v1/profiles/{name}".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    controller
        .set_persisted_default_profile(&rt, Some("fast".to_string()))
        .expect("set persisted default profile via operator summary link");
    controller
        .upsert_persisted_profile(
            &rt,
            "fast".to_string(),
            crate::config::ServiceControlProfile {
                station: Some("alpha".to_string()),
                model: Some("gpt-5.4".to_string()),
                reasoning_effort: Some("medium".to_string()),
                service_tier: Some("priority".to_string()),
                ..Default::default()
            },
        )
        .expect("upsert persisted profile via operator summary link");
    controller
        .delete_persisted_profile(&rt, "fast".to_string())
        .expect("delete persisted profile via operator summary link");

    let default_payload = default_payload
        .lock()
        .expect("persisted default payload lock")
        .clone()
        .expect("persisted default payload");
    assert_eq!(
        default_payload.get("profile_name"),
        Some(&Value::String("fast".to_string()))
    );

    let upsert_payload = upsert_payload
        .lock()
        .expect("profile payload lock")
        .clone()
        .expect("profile payload");
    assert_eq!(
        upsert_payload.get("station"),
        Some(&Value::String("alpha".to_string()))
    );
    assert_eq!(
        upsert_payload.get("model"),
        Some(&Value::String("gpt-5.4".to_string()))
    );
    assert_eq!(
        upsert_payload.get("reasoning_effort"),
        Some(&Value::String("medium".to_string()))
    );
    assert_eq!(*delete_hits.lock().expect("profile delete hits lock"), 1);

    handle.abort();
}

#[test]
fn attached_persisted_station_spec_uses_v1_specs_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_put_payload = Arc::new(Mutex::new(None::<Value>));
    let delete_hits = Arc::new(Mutex::new(0usize));
    let app = Router::new().route(
        "/__codex_helper/api/v1/stations/specs/alpha",
        put({
            let observed_put_payload = observed_put_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_put_payload = observed_put_payload.clone();
                async move {
                    *observed_put_payload
                        .lock()
                        .expect("station spec payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        })
        .delete({
            let delete_hits = delete_hits.clone();
            move || {
                let delete_hits = delete_hits.clone();
                async move {
                    *delete_hits.lock().expect("delete hits lock") += 1;
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4304, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4304);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_station_spec_api = true;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .upsert_persisted_station_spec(
            &rt,
            "alpha".to_string(),
            PersistedStationSpec {
                name: "alpha".to_string(),
                alias: Some("Alpha".to_string()),
                enabled: false,
                level: 7,
                members: vec![crate::config::GroupMemberRefV2 {
                    provider: "right".to_string(),
                    endpoint_names: vec!["hk".to_string()],
                    preferred: true,
                }],
            },
        )
        .expect("upsert persisted station spec");
    controller
        .delete_persisted_station_spec(&rt, "alpha".to_string())
        .expect("delete persisted station spec");

    let observed_put_payload = observed_put_payload
        .lock()
        .expect("station spec payload lock")
        .clone()
        .expect("station spec payload");
    assert_eq!(
        observed_put_payload.get("alias"),
        Some(&Value::String("Alpha".to_string()))
    );
    assert_eq!(
        observed_put_payload.get("enabled"),
        Some(&Value::Bool(false))
    );
    assert_eq!(observed_put_payload.get("level"), Some(&Value::from(7)));
    assert_eq!(
        observed_put_payload["members"][0]
            .get("provider")
            .and_then(|value| value.as_str()),
        Some("right")
    );
    assert_eq!(*delete_hits.lock().expect("delete hits lock"), 1);

    handle.abort();
}

#[test]
fn attached_persisted_station_settings_use_operator_summary_links() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let active_payload = Arc::new(Mutex::new(None::<Value>));
    let update_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new()
        .route(
            "/__alt/v1/stations/active",
            post({
                let active_payload = active_payload.clone();
                move |Json(payload): Json<Value>| {
                    let active_payload = active_payload.clone();
                    async move {
                        *active_payload.lock().expect("active payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        )
        .route(
            "/__alt/v1/stations/alpha",
            put({
                let update_payload = update_payload.clone();
                move |Json(payload): Json<Value>| {
                    let update_payload = update_payload.clone();
                    async move {
                        *update_payload.lock().expect("update payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4313, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4313);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_persisted_station_settings = true;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        stations: "/__alt/v1/stations".to_string(),
        station_by_name_template: "/__alt/v1/stations/{name}".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    controller
        .set_persisted_active_station(&rt, Some("alpha".to_string()))
        .expect("set persisted active station via operator summary link");
    controller
        .update_persisted_station(&rt, "alpha".to_string(), false, 9)
        .expect("update persisted station via operator summary link");

    let active_payload = active_payload
        .lock()
        .expect("active payload lock")
        .clone()
        .expect("active payload");
    assert_eq!(
        active_payload.get("station_name"),
        Some(&Value::String("alpha".to_string()))
    );

    let update_payload = update_payload
        .lock()
        .expect("update payload lock")
        .clone()
        .expect("update payload");
    assert_eq!(update_payload.get("enabled"), Some(&Value::Bool(false)));
    assert_eq!(update_payload.get("level"), Some(&Value::from(9)));

    handle.abort();
}

#[test]
fn attached_persisted_station_spec_uses_operator_summary_links() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_put_payload = Arc::new(Mutex::new(None::<Value>));
    let delete_hits = Arc::new(Mutex::new(0usize));
    let app = Router::new().route(
        "/__alt/v1/stations/specs/alpha",
        put({
            let observed_put_payload = observed_put_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_put_payload = observed_put_payload.clone();
                async move {
                    *observed_put_payload
                        .lock()
                        .expect("station spec payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        })
        .delete({
            let delete_hits = delete_hits.clone();
            move || {
                let delete_hits = delete_hits.clone();
                async move {
                    *delete_hits.lock().expect("delete hits lock") += 1;
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4314, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4314);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_station_spec_api = true;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        station_spec_by_name_template: "/__alt/v1/stations/specs/{name}".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    controller
        .upsert_persisted_station_spec(
            &rt,
            "alpha".to_string(),
            PersistedStationSpec {
                name: "alpha".to_string(),
                alias: Some("Alpha".to_string()),
                enabled: true,
                level: 5,
                members: vec![crate::config::GroupMemberRefV2 {
                    provider: "right".to_string(),
                    endpoint_names: vec!["hk".to_string()],
                    preferred: true,
                }],
            },
        )
        .expect("upsert persisted station spec via operator summary link");
    controller
        .delete_persisted_station_spec(&rt, "alpha".to_string())
        .expect("delete persisted station spec via operator summary link");

    let observed_put_payload = observed_put_payload
        .lock()
        .expect("station spec payload lock")
        .clone()
        .expect("station spec payload");
    assert_eq!(
        observed_put_payload.get("enabled"),
        Some(&Value::Bool(true))
    );
    assert_eq!(observed_put_payload.get("level"), Some(&Value::from(5)));
    assert_eq!(*delete_hits.lock().expect("delete hits lock"), 1);

    handle.abort();
}

#[test]
fn attached_session_override_reset_uses_v1_reset_endpoint() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new().route(
        "/__codex_helper/api/v1/overrides/session/reset",
        post({
            let observed_payload = observed_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_payload = observed_payload.clone();
                async move {
                    *observed_payload.lock().expect("reset payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4305, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4305);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_session_override_reset = true;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .clear_session_manual_overrides(&rt, "sid-reset".to_string())
        .expect("reset session manual overrides");

    let observed_payload = observed_payload
        .lock()
        .expect("reset payload lock")
        .clone()
        .expect("reset payload");
    assert_eq!(
        observed_payload.get("session_id"),
        Some(&Value::String("sid-reset".to_string()))
    );

    handle.abort();
}

#[test]
fn attached_session_override_reset_uses_operator_summary_link() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new().route(
        "/__alt/v1/overrides/session/reset",
        post({
            let observed_payload = observed_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_payload = observed_payload.clone();
                async move {
                    *observed_payload.lock().expect("reset payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4315, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4315);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_session_override_reset = true;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        session_overrides: "/__alt/v1/overrides/session".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    controller
        .clear_session_manual_overrides(&rt, "sid-reset-alt".to_string())
        .expect("reset session manual overrides via operator summary link");

    let observed_payload = observed_payload
        .lock()
        .expect("reset payload lock")
        .clone()
        .expect("reset payload");
    assert_eq!(
        observed_payload.get("session_id"),
        Some(&Value::String("sid-reset-alt".to_string()))
    );

    handle.abort();
}

#[test]
fn attached_session_effort_override_uses_v1_effort_endpoint() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new().route(
        "/__codex_helper/api/v1/overrides/session/effort",
        post({
            let observed_payload = observed_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_payload = observed_payload.clone();
                async move {
                    *observed_payload.lock().expect("effort payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4308, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4308);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .apply_session_effort_override(&rt, "sid-effort".to_string(), Some("high".to_string()))
        .expect("set effort via canonical endpoint");

    let observed_payload = observed_payload
        .lock()
        .expect("effort payload lock")
        .clone()
        .expect("effort payload");
    assert_eq!(
        observed_payload.get("session_id"),
        Some(&Value::String("sid-effort".to_string()))
    );
    assert_eq!(
        observed_payload.get("effort"),
        Some(&Value::String("high".to_string()))
    );

    handle.abort();
}

#[test]
fn attached_session_effort_override_uses_operator_summary_link() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new().route(
        "/__alt/v1/overrides/session/effort",
        post({
            let observed_payload = observed_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_payload = observed_payload.clone();
                async move {
                    *observed_payload.lock().expect("effort payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4316, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4316);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        session_overrides: "/__alt/v1/overrides/session".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    controller
        .apply_session_effort_override(&rt, "sid-effort-alt".to_string(), Some("low".to_string()))
        .expect("set effort via operator summary link");

    let observed_payload = observed_payload
        .lock()
        .expect("effort payload lock")
        .clone()
        .expect("effort payload");
    assert_eq!(
        observed_payload.get("session_id"),
        Some(&Value::String("sid-effort-alt".to_string()))
    );
    assert_eq!(
        observed_payload.get("effort"),
        Some(&Value::String("low".to_string()))
    );

    handle.abort();
}

#[test]
fn attached_persisted_provider_spec_uses_v1_specs_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_put_payload = Arc::new(Mutex::new(None::<Value>));
    let delete_hits = Arc::new(Mutex::new(0usize));
    let app = Router::new().route(
        "/__codex_helper/api/v1/providers/specs/right",
        put({
            let observed_put_payload = observed_put_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_put_payload = observed_put_payload.clone();
                async move {
                    *observed_put_payload
                        .lock()
                        .expect("provider spec payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        })
        .delete({
            let delete_hits = delete_hits.clone();
            move || {
                let delete_hits = delete_hits.clone();
                async move {
                    *delete_hits.lock().expect("delete hits lock") += 1;
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4305, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4305);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_provider_spec_api = true;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .upsert_persisted_provider_spec(
            &rt,
            "right".to_string(),
            PersistedProviderSpec {
                name: "right".to_string(),
                alias: Some("Right".to_string()),
                enabled: false,
                auth_token_env: Some("RIGHTCODE_API_KEY".to_string()),
                api_key_env: Some("RIGHTCODE_HEADER_KEY".to_string()),
                endpoints: vec![
                    crate::config::PersistedProviderEndpointSpec {
                        name: "hk".to_string(),
                        base_url: "https://right-hk.example.com/v1".to_string(),
                        enabled: true,
                        priority: 0,
                    },
                    crate::config::PersistedProviderEndpointSpec {
                        name: "us".to_string(),
                        base_url: "https://right-us.example.com/v1".to_string(),
                        enabled: false,
                        priority: 1,
                    },
                ],
            },
        )
        .expect("upsert persisted provider spec");
    controller
        .delete_persisted_provider_spec(&rt, "right".to_string())
        .expect("delete persisted provider spec");

    let observed_put_payload = observed_put_payload
        .lock()
        .expect("provider spec payload lock")
        .clone()
        .expect("provider spec payload");
    assert_eq!(
        observed_put_payload.get("alias"),
        Some(&Value::String("Right".to_string()))
    );
    assert_eq!(
        observed_put_payload.get("enabled"),
        Some(&Value::Bool(false))
    );
    assert_eq!(
        observed_put_payload.get("auth_token_env"),
        Some(&Value::String("RIGHTCODE_API_KEY".to_string()))
    );
    assert_eq!(
        observed_put_payload.get("api_key_env"),
        Some(&Value::String("RIGHTCODE_HEADER_KEY".to_string()))
    );
    assert_eq!(
        observed_put_payload["endpoints"][0]
            .get("name")
            .and_then(|value| value.as_str()),
        Some("hk")
    );
    assert_eq!(*delete_hits.lock().expect("delete hits lock"), 1);

    handle.abort();
}

#[test]
fn attached_persisted_provider_spec_uses_operator_summary_links() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let observed_put_payload = Arc::new(Mutex::new(None::<Value>));
    let delete_hits = Arc::new(Mutex::new(0usize));
    let app = Router::new().route(
        "/__alt/v1/providers/specs/right",
        put({
            let observed_put_payload = observed_put_payload.clone();
            move |Json(payload): Json<Value>| {
                let observed_put_payload = observed_put_payload.clone();
                async move {
                    *observed_put_payload
                        .lock()
                        .expect("provider spec payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        })
        .delete({
            let delete_hits = delete_hits.clone();
            move || {
                let delete_hits = delete_hits.clone();
                async move {
                    *delete_hits.lock().expect("delete hits lock") += 1;
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4317, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4317);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.supports_provider_spec_api = true;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        provider_spec_by_name_template: "/__alt/v1/providers/specs/{name}".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    controller
        .upsert_persisted_provider_spec(
            &rt,
            "right".to_string(),
            PersistedProviderSpec {
                name: "right".to_string(),
                alias: Some("Right".to_string()),
                enabled: true,
                auth_token_env: Some("RIGHTCODE_API_KEY".to_string()),
                api_key_env: None,
                endpoints: vec![crate::config::PersistedProviderEndpointSpec {
                    name: "hk".to_string(),
                    base_url: "https://right-hk.example.com/v1".to_string(),
                    enabled: true,
                    priority: 0,
                }],
            },
        )
        .expect("upsert persisted provider spec via operator summary link");
    controller
        .delete_persisted_provider_spec(&rt, "right".to_string())
        .expect("delete persisted provider spec via operator summary link");

    let observed_put_payload = observed_put_payload
        .lock()
        .expect("provider spec payload lock")
        .clone()
        .expect("provider spec payload");
    assert_eq!(
        observed_put_payload.get("enabled"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        observed_put_payload.get("auth_token_env"),
        Some(&Value::String("RIGHTCODE_API_KEY".to_string()))
    );
    assert_eq!(*delete_hits.lock().expect("delete hits lock"), 1);

    handle.abort();
}
