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

#[test]
fn route_attempts_are_derived_from_legacy_retry_chain() {
    let chain = vec![
        "right:https://api.right.example/v1 (idx=0) transport_error=operation timed out model=gpt-5".to_string(),
        "alpha:https://one.example/v1 (idx=1) skipped_unsupported_model=gpt-5".to_string(),
        "https://api.vibe.example/v1 (idx=2) status=200 class=- model=gpt-5-fast".to_string(),
        "all_upstreams_avoided total=3".to_string(),
    ];

    let attempts = parse_route_attempts_from_chain(&chain);

    assert_eq!(attempts.len(), 4);
    assert_eq!(attempts[0].attempt_index, 0);
    assert_eq!(attempts[0].station_name.as_deref(), Some("right"));
    assert_eq!(
        attempts[0].upstream_base_url.as_deref(),
        Some("https://api.right.example/v1")
    );
    assert_eq!(attempts[0].upstream_index, Some(0));
    assert_eq!(attempts[0].decision, "failed_transport");
    assert_eq!(
        attempts[0].error_class.as_deref(),
        Some("upstream_transport_error")
    );
    assert_eq!(attempts[0].model.as_deref(), Some("gpt-5"));
    assert_eq!(attempts[0].reason.as_deref(), Some("operation timed out"));

    assert_eq!(attempts[1].decision, "skipped_capability_mismatch");
    assert!(attempts[1].skipped);
    assert_eq!(attempts[1].station_name.as_deref(), Some("alpha"));
    assert_eq!(attempts[1].model.as_deref(), Some("gpt-5"));

    assert_eq!(attempts[2].decision, "completed");
    assert_eq!(attempts[2].status_code, Some(200));
    assert_eq!(attempts[2].station_name, None);
    assert_eq!(
        attempts[2].upstream_base_url.as_deref(),
        Some("https://api.vibe.example/v1")
    );

    assert_eq!(attempts[3].decision, "all_upstreams_avoided");
    assert_eq!(attempts[3].reason.as_deref(), Some("total=3"));
    assert!(attempts[3].skipped);
}

#[test]
fn retry_info_serializes_structured_route_attempts() {
    let upstream_chain = vec![
        "right:https://api.right.example/v1 (idx=0) transport_error=timeout model=gpt-5"
            .to_string(),
        "https://api.vibe.example/v1 (idx=1) status=200 class=- model=gpt-5".to_string(),
    ];
    let retry = RetryInfo {
        attempts: 2,
        route_attempts: parse_route_attempts_from_chain(&upstream_chain),
        upstream_chain,
    };

    let value = serde_json::to_value(&retry).expect("serialize retry info");

    assert_eq!(value["attempts"].as_u64(), Some(2));
    assert_eq!(
        value["route_attempts"][0]["decision"].as_str(),
        Some("failed_transport")
    );
    assert_eq!(
        value["route_attempts"][0]["station_name"].as_str(),
        Some("right")
    );
    assert_eq!(
        value["route_attempts"][1]["decision"].as_str(),
        Some("completed")
    );
}

#[test]
fn retry_info_station_helpers_use_derived_route_attempts() {
    let retry = RetryInfo {
        attempts: 2,
        upstream_chain: vec![
            "backup:https://api.backup.example/v1 (idx=0) transport_error=timeout model=gpt-5"
                .to_string(),
            "https://api.primary.example/v1 (idx=1) status=200 class=- model=gpt-5".to_string(),
        ],
        route_attempts: Vec::new(),
    };

    assert!(retry.touches_station("backup"));
    assert!(retry.touched_other_station(Some("primary")));
    assert!(!retry.touched_other_station(Some("backup")));
}
