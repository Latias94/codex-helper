use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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

fn sample_summary_row(group_value: &str) -> crate::request_ledger::RequestUsageSummaryRow {
    crate::request_ledger::RequestUsageSummaryRow {
        group_value: group_value.to_string(),
        aggregate: crate::request_ledger::RequestUsageAggregate {
            requests: 2,
            input_tokens: 100,
            output_tokens: 50,
            total_tokens: 150,
            duration_ms_total: 800,
            ..Default::default()
        },
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

#[test]
fn read_request_ledger_summary_prefers_attached_api_when_supported() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let captured_query = Arc::new(Mutex::new(None::<HashMap<String, String>>));
    let captured_query_handler = captured_query.clone();
    let app = Router::new().route(
        "/__codex_helper/api/v1/request-ledger/summary",
        get(
            move |axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>| {
                let captured_query = captured_query_handler.clone();
                async move {
                    *captured_query.lock().expect("capture query lock") = Some(query);
                    Json(vec![sample_summary_row("relay")])
                }
            },
        ),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4295, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4295);
    attached.admin_base_url = base_url.clone();
    attached.supports_request_ledger_summary_api = true;
    controller.mode = ProxyMode::Attached(attached);
    let filters = crate::request_ledger::RequestLogFilters {
        session: Some("sid-123".to_string()),
        model: Some("gpt-5.4".to_string()),
        station: Some("backup".to_string()),
        provider: Some("relay".to_string()),
        status_min: Some(400),
        status_max: Some(499),
        fast: true,
        retried: true,
    };

    let result = controller
        .read_request_ledger_summary(
            &rt,
            crate::request_ledger::RequestUsageSummaryGroup::Provider,
            10,
            &filters,
        )
        .expect("read attached request ledger summary");

    assert_eq!(
        result.source,
        RequestLedgerDataSource::AttachedApi {
            admin_base_url: base_url
        }
    );
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].group_value, "relay");
    assert_eq!(result.rows[0].aggregate.total_tokens, 150);

    let captured = captured_query.lock().expect("captured query").clone();
    assert_eq!(
        captured
            .as_ref()
            .and_then(|query| query.get("limit"))
            .map(String::as_str),
        Some("10")
    );
    assert_eq!(
        captured
            .as_ref()
            .and_then(|query| query.get("by"))
            .map(String::as_str),
        Some("provider")
    );
    assert_eq!(
        captured
            .as_ref()
            .and_then(|query| query.get("session"))
            .map(String::as_str),
        Some("sid-123")
    );
    assert_eq!(
        captured
            .as_ref()
            .and_then(|query| query.get("model"))
            .map(String::as_str),
        Some("gpt-5.4")
    );
    assert_eq!(
        captured
            .as_ref()
            .and_then(|query| query.get("station"))
            .map(String::as_str),
        Some("backup")
    );
    assert_eq!(
        captured
            .as_ref()
            .and_then(|query| query.get("provider"))
            .map(String::as_str),
        Some("relay")
    );
    assert_eq!(
        captured
            .as_ref()
            .and_then(|query| query.get("status_min"))
            .map(String::as_str),
        Some("400")
    );
    assert_eq!(
        captured
            .as_ref()
            .and_then(|query| query.get("status_max"))
            .map(String::as_str),
        Some("499")
    );
    assert_eq!(
        captured
            .as_ref()
            .and_then(|query| query.get("fast"))
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        captured
            .as_ref()
            .and_then(|query| query.get("retried"))
            .map(String::as_str),
        Some("true")
    );

    handle.abort();
}
