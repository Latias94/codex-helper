use super::*;

#[test]
fn request_log_serializes_request_id_when_present() {
    let value = serde_json::to_value(RequestLog {
        timestamp_ms: 1,
        request_id: Some(42),
        service: "codex",
        method: "POST",
        path: "/v1/responses",
        status_code: 200,
        duration_ms: 123,
        ttfb_ms: Some(10),
        station_name: "right",
        provider_id: Some("right".to_string()),
        upstream_base_url: "https://example.com/v1",
        session_id: Some("sid-1".to_string()),
        cwd: Some("/workdir".to_string()),
        reasoning_effort: Some("medium".to_string()),
        service_tier: ServiceTierLog {
            requested: Some("priority".to_string()),
            effective: Some("priority".to_string()),
            actual: Some("priority".to_string()),
        },
        usage: None,
        http_debug: None,
        http_debug_ref: None,
        retry: None,
    })
    .expect("serialize request log");

    assert_eq!(value["request_id"].as_u64(), Some(42));
}
