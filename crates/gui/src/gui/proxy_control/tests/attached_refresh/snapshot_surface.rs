use super::*;

#[test]
fn refresh_attached_prefers_station_snapshot_payload() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "shared_capabilities": {
            "session_observability": true,
            "request_history": true
        },
        "host_local_capabilities": {
            "session_history": true,
            "transcript_read": true,
            "cwd_enrichment": true
        },
        "endpoints": [
            "/__codex_helper/api/v1/snapshot",
            "/__codex_helper/api/v1/stations",
            "/__codex_helper/api/v1/stations/runtime"
        ]
    });
    let snapshot = sample_snapshot(vec![sample_station("preferred-station")]);
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
            "/__codex_helper/api/v1/snapshot",
            get({
                let snapshot = snapshot.clone();
                move || {
                    let snapshot = snapshot.clone();
                    async move { Json(snapshot) }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4100, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4100, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("attached snapshot");
    assert_eq!(snapshot.stations.len(), 1);
    assert_eq!(snapshot.stations[0].name, "preferred-station");
    assert!(snapshot.shared_capabilities.session_observability);
    assert!(snapshot.shared_capabilities.request_history);
    assert!(snapshot.host_local_capabilities.session_history);
    assert!(snapshot.host_local_capabilities.transcript_read);
    assert!(snapshot.host_local_capabilities.cwd_enrichment);
    assert!(
        controller
            .attached()
            .expect("attached status")
            .supports_station_api
    );

    handle.abort();
}

#[test]
fn refresh_attached_loads_pricing_catalog_surface() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "surface_capabilities": {
            "snapshot": true,
            "pricing_catalog": true
        },
        "endpoints": []
    });
    let snapshot = sample_snapshot(Vec::new());
    let pricing_catalog = serde_json::json!({
        "source": "test-api",
        "model_count": 1,
        "models": [{
            "model_id": "test-model",
            "input_per_1m_usd": "1",
            "output_per_1m_usd": "2",
            "source": "test",
            "confidence": "estimated"
        }]
    });
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
            "/__codex_helper/api/v1/pricing/catalog",
            get({
                let pricing_catalog = pricing_catalog.clone();
                move || {
                    let pricing_catalog = pricing_catalog.clone();
                    async move { Json(pricing_catalog) }
                }
            }),
        )
        .route(
            "/__codex_helper/api/v1/snapshot",
            get({
                let snapshot = snapshot.clone();
                move || {
                    let snapshot = snapshot.clone();
                    async move { Json(snapshot) }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4101, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4101, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("attached snapshot");
    assert!(snapshot.supports_pricing_catalog_api);
    assert_eq!(snapshot.pricing_catalog.source, "test-api");
    assert_eq!(snapshot.pricing_catalog.model_count, 1);
    assert_eq!(snapshot.pricing_catalog.models[0].model_id, "test-model");

    handle.abort();
}
