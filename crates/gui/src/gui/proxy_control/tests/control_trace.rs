use super::helpers::{ScopedEnv, env_lock, spawn_test_server};
use super::*;

#[test]
fn read_control_trace_entries_prefers_attached_api_when_supported() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let app = Router::new().route(
        "/__codex_helper/api/v1/control-trace",
        get(|| async {
            Json(vec![ControlTraceLogEntry {
                ts_ms: 300,
                kind: "request_completed".to_string(),
                service: Some("codex".to_string()),
                request_id: Some(9),
                trace_id: Some("codex-9".to_string()),
                event: Some("request_completed".to_string()),
                detail: None,
                payload: serde_json::json!({
                    "method": "POST",
                    "path": "/v1/responses"
                }),
            }])
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4290, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4290);
    attached.admin_base_url = base_url.clone();
    attached.supports_control_trace_api = true;
    controller.mode = ProxyMode::Attached(attached);

    let result = controller
        .read_control_trace_entries(&rt, 80)
        .expect("read attached control trace");

    assert_eq!(
        result.source,
        ControlTraceDataSource::AttachedApi {
            admin_base_url: base_url
        }
    );
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].ts_ms, 300);
    assert_eq!(result.entries[0].kind, "request_completed");

    handle.abort();
}

#[test]
fn read_control_trace_entries_uses_operator_summary_control_trace_link_when_present() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let app = Router::new().route(
        "/__alt/v1/control-trace",
        get(|| async {
            Json(vec![ControlTraceLogEntry {
                ts_ms: 301,
                kind: "request_completed".to_string(),
                service: Some("codex".to_string()),
                request_id: Some(10),
                trace_id: Some("codex-10".to_string()),
                event: Some("request_completed".to_string()),
                detail: None,
                payload: serde_json::json!({
                    "method": "POST",
                    "path": "/v1/responses"
                }),
            }])
        }),
    );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4292, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4292);
    attached.admin_base_url = base_url.clone();
    attached.supports_control_trace_api = true;
    attached.operator_summary_links = Some(crate::dashboard_core::OperatorSummaryLinks {
        control_trace: "/__alt/v1/control-trace".to_string(),
        ..Default::default()
    });
    controller.mode = ProxyMode::Attached(attached);

    let result = controller
        .read_control_trace_entries(&rt, 80)
        .expect("read attached control trace via operator summary link");

    assert_eq!(
        result.source,
        ControlTraceDataSource::AttachedApi {
            admin_base_url: base_url
        }
    );
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].ts_ms, 301);

    handle.abort();
}

#[test]
fn read_control_trace_entries_falls_back_to_local_file_when_api_is_unavailable() {
    let _env_lock = env_lock();
    let mut scoped = ScopedEnv::default();
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let trace_path = std::env::temp_dir()
        .join("codex-helper-gui-tests")
        .join(format!("control-trace-{unique}.jsonl"));
    std::fs::create_dir_all(trace_path.parent().expect("trace parent"))
        .expect("create trace parent");
    unsafe {
        scoped.set(
            "CODEX_HELPER_CONTROL_TRACE_PATH",
            trace_path.to_string_lossy().as_ref(),
        );
    }
    std::fs::write(
        &trace_path,
        [
            serde_json::json!({
                "ts_ms": 100,
                "kind": "retry_trace",
                "service": "codex",
                "request_id": 4,
                "event": "attempt_select",
                "payload": {
                    "event": "attempt_select",
                    "station_name": "right"
                }
            })
            .to_string(),
            serde_json::json!({
                "ts_ms": 200,
                "kind": "request_completed",
                "service": "codex",
                "request_id": 4,
                "event": "request_completed",
                "payload": {
                    "method": "POST",
                    "path": "/v1/responses"
                }
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .expect("write local control trace");

    let mut controller = ProxyController::new(4291, ServiceKind::Codex);
    let mut attached = AttachedStatus::new(4291);
    attached.admin_base_url = "http://100.88.0.5:4101".to_string();
    attached.supports_control_trace_api = false;
    controller.mode = ProxyMode::Attached(attached);

    let result = controller
        .read_control_trace_entries(&rt, 80)
        .expect("fallback local control trace");

    assert_eq!(
        result.source,
        ControlTraceDataSource::AttachedFallbackLocal {
            admin_base_url: "http://100.88.0.5:4101".to_string(),
            path: trace_path.clone(),
        }
    );
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[0].ts_ms, 200);
    assert_eq!(result.entries[0].kind, "request_completed");
}
