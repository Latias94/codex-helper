use super::*;

#[test]
fn refresh_attached_prefers_aggregate_session_override_api() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "endpoints": [
            "/__codex_helper/api/v1/status/active",
            "/__codex_helper/api/v1/status/recent",
            "/__codex_helper/api/v1/status/session-stats",
            "/__codex_helper/api/v1/status/health-checks",
            "/__codex_helper/api/v1/status/station-health",
            "/__codex_helper/api/v1/runtime/status",
            "/__codex_helper/api/v1/stations",
            "/__codex_helper/api/v1/overrides/global-station",
            "/__codex_helper/api/v1/overrides/session"
        ]
    });
    let stations = vec![sample_station("aggregate-only")];
    let app = Router::new()
        .route(
            "/__codex_helper/api/v1/capabilities",
            get({
                let caps = caps.clone();
                move || {
                    let caps = caps.clone();
                    async move { Json(caps) }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/status/active",
            get(|| async { Json(Vec::<ActiveRequest>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/recent",
            get(|| async { Json(Vec::<FinishedRequest>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/session-stats",
            get(|| async { Json(HashMap::<String, SessionStats>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/health-checks",
            get(|| async { Json(HashMap::<String, HealthCheckStatus>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/status/station-health",
            get(|| async { Json(HashMap::<String, StationHealth>::new()) }),
        )
        .route(
            "/__codex_helper/api/v1/runtime/status",
            get(|| async {
                Json(serde_json::json!({
                    "loaded_at_ms": 11,
                    "source_mtime_ms": 22,
                }))
            }),
        )
        .route(
            "/__codex_helper/api/v1/stations",
            get({
                let stations = stations.clone();
                move || {
                    let stations = stations.clone();
                    async move { Json(stations) }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/global-station",
            get(|| async { Json(Option::<String>::None) }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session",
            get(|| async {
                Json(serde_json::json!({
                    "sessions": {
                        "sid-a": {
                            "reasoning_effort": "high",
                            "station_name": "aggregate-only",
                            "model": "gpt-5.4",
                            "service_tier": "priority"
                        }
                    }
                }))
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4201, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4201, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("aggregate attached snapshot");
    assert_eq!(
        snapshot
            .session_station_overrides
            .get("sid-a")
            .map(String::as_str),
        Some("aggregate-only")
    );
    assert_eq!(
        snapshot
            .session_effort_overrides
            .get("sid-a")
            .map(String::as_str),
        Some("high")
    );
    assert_eq!(
        snapshot
            .session_model_overrides
            .get("sid-a")
            .map(String::as_str),
        Some("gpt-5.4")
    );
    assert_eq!(
        snapshot
            .session_service_tier_overrides
            .get("sid-a")
            .map(String::as_str),
        Some("priority")
    );

    handle.abort();
}
