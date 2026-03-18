use super::helpers::spawn_test_server;
use super::*;

#[test]
fn attached_runtime_reload_uses_operator_summary_link() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let reload_hits = Arc::new(Mutex::new(0usize));
    let app = Router::new().route(
        "/__alt/v1/runtime/reload",
        post({
            let reload_hits = reload_hits.clone();
            move || {
                let reload_hits = reload_hits.clone();
                async move {
                    *reload_hits.lock().expect("reload hits lock") += 1;
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4298, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4298);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        runtime_reload: "/__alt/v1/runtime/reload".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    controller
        .reload_runtime_config(&rt)
        .expect("runtime reload via operator summary link");

    assert_eq!(*reload_hits.lock().expect("reload hits lock"), 1);
    handle.abort();
}

#[test]
fn attached_runtime_meta_uses_station_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let station_payload = Arc::new(Mutex::new(None::<Value>));
    let station_app = Router::new().route(
        "/__codex_helper/api/v1/stations/runtime",
        post({
            let station_payload = station_payload.clone();
            move |Json(payload): Json<Value>| {
                let station_payload = station_payload.clone();
                async move {
                    *station_payload.lock().expect("station payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (station_base_url, station_handle) = spawn_test_server(&rt, station_app);

    let mut station_controller = ProxyController::new(4300, ServiceKind::Codex);
    let mut station_attached = AttachedStatus::new(4300);
    station_attached.admin_base_url = station_base_url;
    station_attached.supports_station_runtime_override = true;
    station_attached.supports_station_api = true;
    station_controller.mode = ProxyMode::Attached(station_attached);
    station_controller
        .set_runtime_station_meta(
            &rt,
            "alpha".to_string(),
            Some(Some(false)),
            Some(Some(7)),
            Some(Some(RuntimeConfigState::Draining)),
        )
        .expect("station runtime meta update");

    let station_payload = station_payload
        .lock()
        .expect("station payload lock")
        .clone()
        .expect("station payload");
    assert_eq!(
        station_payload.get("station_name"),
        Some(&Value::String("alpha".to_string()))
    );
    assert_eq!(
        station_payload.get("runtime_state"),
        Some(&Value::String("draining".to_string()))
    );
    station_handle.abort();
}

#[test]
fn attached_runtime_meta_uses_operator_summary_station_link() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let station_payload = Arc::new(Mutex::new(None::<Value>));
    let station_app = Router::new().route(
        "/__alt/v1/stations/runtime",
        post({
            let station_payload = station_payload.clone();
            move |Json(payload): Json<Value>| {
                let station_payload = station_payload.clone();
                async move {
                    *station_payload.lock().expect("station payload lock") = Some(payload);
                    StatusCode::NO_CONTENT
                }
            }
        }),
    );
    let (station_base_url, station_handle) = spawn_test_server(&rt, station_app);

    let mut station_controller = ProxyController::new(4309, ServiceKind::Codex);
    let mut station_attached = AttachedStatus::new(4309);
    station_attached.admin_base_url = station_base_url;
    station_attached.supports_station_runtime_override = true;
    station_attached.supports_station_api = true;
    station_attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        stations: "/__alt/v1/stations".to_string(),
        ..Default::default()
    });
    station_controller.mode = ProxyMode::Attached(station_attached);
    station_controller
        .set_runtime_station_meta(
            &rt,
            "alpha".to_string(),
            Some(Some(true)),
            Some(Some(3)),
            Some(Some(RuntimeConfigState::HalfOpen)),
        )
        .expect("station runtime meta update via operator summary link");

    let station_payload = station_payload
        .lock()
        .expect("station payload lock")
        .clone()
        .expect("station payload");
    assert_eq!(
        station_payload.get("station_name"),
        Some(&Value::String("alpha".to_string()))
    );
    assert_eq!(station_payload.get("enabled"), Some(&Value::Bool(true)));
    assert_eq!(station_payload.get("level"), Some(&Value::from(3)));
    assert_eq!(
        station_payload.get("runtime_state"),
        Some(&Value::String("half_open".to_string()))
    );
    station_handle.abort();
}

#[test]
fn attached_probe_station_uses_station_probe_and_legacy_healthcheck_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let station_payload = Arc::new(Mutex::new(None::<Value>));
    let station_app = Router::new().route(
        "/__codex_helper/api/v1/stations/probe",
        post({
            let station_payload = station_payload.clone();
            move |Json(payload): Json<Value>| {
                let station_payload = station_payload.clone();
                async move {
                    *station_payload.lock().expect("station payload lock") = Some(payload);
                    StatusCode::OK
                }
            }
        }),
    );
    let (station_base_url, station_handle) = spawn_test_server(&rt, station_app);

    let mut station_controller = ProxyController::new(4306, ServiceKind::Codex);
    let mut station_attached = AttachedStatus::new(4306);
    station_attached.admin_base_url = station_base_url;
    station_attached.api_version = Some(1);
    station_attached.supports_station_api = true;
    station_controller.mode = ProxyMode::Attached(station_attached);
    station_controller
        .probe_station(&rt, "alpha".to_string())
        .expect("station probe");

    let station_payload = station_payload
        .lock()
        .expect("station payload lock")
        .clone()
        .expect("station payload");
    assert_eq!(
        station_payload.get("station_name"),
        Some(&Value::String("alpha".to_string()))
    );
    station_handle.abort();

    let operator_payload = Arc::new(Mutex::new(None::<Value>));
    let operator_app = Router::new().route(
        "/__alt/v1/stations/probe",
        post({
            let operator_payload = operator_payload.clone();
            move |Json(payload): Json<Value>| {
                let operator_payload = operator_payload.clone();
                async move {
                    *operator_payload.lock().expect("operator payload lock") = Some(payload);
                    StatusCode::OK
                }
            }
        }),
    );
    let (operator_base_url, operator_handle) = spawn_test_server(&rt, operator_app);

    let mut operator_controller = ProxyController::new(4318, ServiceKind::Codex);
    let mut operator_attached = AttachedStatus::new(4318);
    operator_attached.admin_base_url = operator_base_url;
    operator_attached.api_version = Some(1);
    operator_attached.supports_station_api = true;
    operator_attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        station_probe: "/__alt/v1/stations/probe".to_string(),
        ..Default::default()
    });
    operator_controller.mode = ProxyMode::Attached(operator_attached);
    operator_controller
        .probe_station(&rt, "gamma".to_string())
        .expect("station probe via operator summary link");

    let operator_payload = operator_payload
        .lock()
        .expect("operator payload lock")
        .clone()
        .expect("operator payload");
    assert_eq!(
        operator_payload.get("station_name"),
        Some(&Value::String("gamma".to_string()))
    );
    operator_handle.abort();

    let legacy_payload = Arc::new(Mutex::new(None::<Value>));
    let legacy_app = Router::new().route(
        "/__codex_helper/api/v1/healthcheck/start",
        post({
            let legacy_payload = legacy_payload.clone();
            move |Json(payload): Json<Value>| {
                let legacy_payload = legacy_payload.clone();
                async move {
                    *legacy_payload.lock().expect("legacy payload lock") = Some(payload);
                    StatusCode::OK
                }
            }
        }),
    );
    let (legacy_base_url, legacy_handle) = spawn_test_server(&rt, legacy_app);

    let mut legacy_controller = ProxyController::new(4307, ServiceKind::Codex);
    let mut legacy_attached = AttachedStatus::new(4307);
    legacy_attached.admin_base_url = legacy_base_url;
    legacy_attached.api_version = Some(1);
    legacy_attached.supports_station_api = false;
    legacy_controller.mode = ProxyMode::Attached(legacy_attached);
    legacy_controller
        .probe_station(&rt, "beta".to_string())
        .expect("legacy probe fallback");

    let legacy_payload = legacy_payload
        .lock()
        .expect("legacy payload lock")
        .clone()
        .expect("legacy payload");
    assert_eq!(legacy_payload.get("all"), Some(&Value::Bool(false)));
    assert_eq!(
        legacy_payload
            .get("station_names")
            .and_then(|value| value.as_array())
            .and_then(|items| items.first())
            .and_then(|value| value.as_str()),
        Some("beta")
    );
    legacy_handle.abort();
}

#[test]
fn attached_healthcheck_control_uses_v1_endpoints() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let start_payload = Arc::new(Mutex::new(None::<Value>));
    let cancel_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new()
        .route(
            "/__codex_helper/api/v1/healthcheck/start",
            post({
                let start_payload = start_payload.clone();
                move |Json(payload): Json<Value>| {
                    let start_payload = start_payload.clone();
                    async move {
                        *start_payload.lock().expect("start payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/healthcheck/cancel",
            post({
                let cancel_payload = cancel_payload.clone();
                move |Json(payload): Json<Value>| {
                    let cancel_payload = cancel_payload.clone();
                    async move {
                        *cancel_payload.lock().expect("cancel payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4319, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4319);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    controller.mode = ProxyMode::Attached(attached);

    controller
        .start_health_checks(&rt, false, vec!["alpha".to_string()])
        .expect("start health checks");
    controller
        .cancel_health_checks(&rt, true, vec!["alpha".to_string(), "beta".to_string()])
        .expect("cancel health checks");

    let start_payload = start_payload
        .lock()
        .expect("start payload lock")
        .clone()
        .expect("start payload");
    assert_eq!(start_payload.get("all"), Some(&Value::Bool(false)));
    assert_eq!(
        start_payload
            .get("station_names")
            .and_then(|value| value.as_array())
            .map(|items| items.len()),
        Some(1)
    );

    let cancel_payload = cancel_payload
        .lock()
        .expect("cancel payload lock")
        .clone()
        .expect("cancel payload");
    assert_eq!(cancel_payload.get("all"), Some(&Value::Bool(true)));
    assert_eq!(
        cancel_payload
            .get("station_names")
            .and_then(|value| value.as_array())
            .map(|items| items.len()),
        Some(2)
    );

    handle.abort();
}

#[test]
fn attached_healthcheck_control_uses_operator_summary_links() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    let start_payload = Arc::new(Mutex::new(None::<Value>));
    let cancel_payload = Arc::new(Mutex::new(None::<Value>));
    let app = Router::new()
        .route(
            "/__alt/v1/healthcheck/start",
            post({
                let start_payload = start_payload.clone();
                move |Json(payload): Json<Value>| {
                    let start_payload = start_payload.clone();
                    async move {
                        *start_payload.lock().expect("start payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        )
        .route(
            "/__alt/v1/healthcheck/cancel",
            post({
                let cancel_payload = cancel_payload.clone();
                move |Json(payload): Json<Value>| {
                    let cancel_payload = cancel_payload.clone();
                    async move {
                        *cancel_payload.lock().expect("cancel payload lock") = Some(payload);
                        StatusCode::NO_CONTENT
                    }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4320, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4320);
    attached.api_version = Some(1);
    attached.admin_base_url = base_url;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        healthcheck_start: "/__alt/v1/healthcheck/start".to_string(),
        healthcheck_cancel: "/__alt/v1/healthcheck/cancel".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    controller
        .start_health_checks(&rt, true, vec!["alpha".to_string()])
        .expect("start health checks via operator summary link");
    controller
        .cancel_health_checks(&rt, false, vec!["beta".to_string()])
        .expect("cancel health checks via operator summary link");

    let start_payload = start_payload
        .lock()
        .expect("start payload lock")
        .clone()
        .expect("start payload");
    assert_eq!(start_payload.get("all"), Some(&Value::Bool(true)));
    assert_eq!(
        start_payload
            .get("station_names")
            .and_then(|value| value.as_array())
            .map(|items| items.len()),
        Some(1)
    );

    let cancel_payload = cancel_payload
        .lock()
        .expect("cancel payload lock")
        .clone()
        .expect("cancel payload");
    assert_eq!(cancel_payload.get("all"), Some(&Value::Bool(false)));
    assert_eq!(
        cancel_payload
            .get("station_names")
            .and_then(|value| value.as_array())
            .map(|items| items.len()),
        Some(1)
    );

    handle.abort();
}
