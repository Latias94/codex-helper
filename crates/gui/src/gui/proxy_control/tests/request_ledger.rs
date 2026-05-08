use super::helpers::spawn_test_server;
use super::*;

fn sample_request(id: u64) -> FinishedRequest {
    FinishedRequest {
        id,
        trace_id: Some(format!("codex-{id}")),
        session_id: Some("sid-ledger".to_string()),
        client_name: None,
        client_addr: None,
        cwd: None,
        model: Some("gpt-5.4".to_string()),
        reasoning_effort: Some("medium".to_string()),
        service_tier: Some("priority".to_string()),
        station_name: Some("primary".to_string()),
        provider_id: Some("relay".to_string()),
        upstream_base_url: Some("https://relay.example/v1".to_string()),
        route_decision: None,
        usage: None,
        cost: crate::pricing::CostBreakdown::default(),
        retry: None,
        observability: crate::state::RequestObservability::default(),
        service: "codex".to_string(),
        method: "POST".to_string(),
        path: "/v1/responses".to_string(),
        status_code: 200,
        duration_ms: 1_000,
        ttfb_ms: Some(200),
        streaming: false,
        ended_at_ms: 123,
    }
}

#[test]
fn read_request_ledger_records_prefers_attached_api_when_supported() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let app = Router::new().route(
        "/__codex_helper/api/v1/request-ledger/recent",
        get(|| async { Json(vec![sample_request(77)]) }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4293, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4293);
    attached.admin_base_url = base_url.clone();
    attached.supports_request_ledger_api = true;
    controller.mode = ProxyMode::Attached(attached);

    let result = controller
        .read_request_ledger_records(&rt, 100)
        .expect("read attached request ledger");

    assert_eq!(
        result.source,
        RequestLedgerDataSource::AttachedApi {
            admin_base_url: base_url
        }
    );
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].id, 77);
    assert_eq!(result.records[0].model.as_deref(), Some("gpt-5.4"));

    handle.abort();
}

#[test]
fn read_request_ledger_records_uses_operator_summary_link_when_present() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let app = Router::new().route(
        "/__alt/v1/request-ledger/recent",
        get(|| async { Json(vec![sample_request(78)]) }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4294, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4294);
    attached.admin_base_url = base_url.clone();
    attached.supports_request_ledger_api = true;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        request_ledger_recent: "/__alt/v1/request-ledger/recent".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    let result = controller
        .read_request_ledger_records(&rt, 100)
        .expect("read attached request ledger via operator summary link");

    assert_eq!(
        result.source,
        RequestLedgerDataSource::AttachedApi {
            admin_base_url: base_url
        }
    );
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].id, 78);

    handle.abort();
}
