use super::*;

#[test]
fn request_log_serializes_request_id_when_present() {
    let value = serde_json::to_value(RequestLog {
        timestamp_ms: 1,
        request_id: Some(42),
        trace_id: Some(request_trace_id("codex", 42)),
        service: "codex",
        method: "POST",
        path: "/v1/responses",
        status_code: 200,
        duration_ms: 123,
        ttfb_ms: Some(10),
        station_name: Some("right"),
        provider_id: Some("right".to_string()),
        endpoint_id: None,
        provider_endpoint_key: None,
        upstream_base_url: "https://example.com/v1",
        session_id: Some("sid-1".to_string()),
        session_identity_source: Some(crate::state::SessionIdentitySource::Header),
        cwd: Some("/workdir".to_string()),
        model: Some("gpt-5".to_string()),
        reasoning_effort: Some("medium".to_string()),
        service_tier: ServiceTierLog {
            requested: Some("priority".to_string()),
            effective: Some("priority".to_string()),
            actual: Some("priority".to_string()),
        },
        codex_bridge: None,
        usage: None,
        http_debug: None,
        http_debug_ref: None,
        route_decision: None,
        retry: None,
        provider_signals: Vec::new(),
        policy_actions: Vec::new(),
    })
    .expect("serialize request log");

    assert_eq!(value["request_id"].as_u64(), Some(42));
    assert_eq!(value["trace_id"].as_str(), Some("codex-42"));
    assert_eq!(value["model"].as_str(), Some("gpt-5"));
    assert_eq!(value["session_identity_source"].as_str(), Some("header"));
}

#[test]
fn request_log_can_serialize_provider_endpoint_without_station_identity() {
    let value = serde_json::to_value(RequestLog {
        timestamp_ms: 1,
        request_id: Some(42),
        trace_id: Some(request_trace_id("codex", 42)),
        service: "codex",
        method: "POST",
        path: "/v1/responses",
        status_code: 200,
        duration_ms: 123,
        ttfb_ms: Some(10),
        station_name: None,
        provider_id: Some("input".to_string()),
        endpoint_id: Some("default".to_string()),
        provider_endpoint_key: Some("codex/input/default".to_string()),
        upstream_base_url: "https://input.example/v1",
        session_id: Some("sid-1".to_string()),
        session_identity_source: None,
        cwd: Some("/workdir".to_string()),
        model: None,
        reasoning_effort: Some("medium".to_string()),
        service_tier: ServiceTierLog::default(),
        codex_bridge: None,
        usage: None,
        http_debug: None,
        http_debug_ref: None,
        route_decision: None,
        retry: None,
        provider_signals: Vec::new(),
        policy_actions: Vec::new(),
    })
    .expect("serialize request log");

    assert!(value["station_name"].is_null());
    assert_eq!(value["provider_id"].as_str(), Some("input"));
    assert_eq!(value["endpoint_id"].as_str(), Some("default"));
    assert_eq!(
        value["provider_endpoint_key"].as_str(),
        Some("codex/input/default")
    );
}

#[test]
fn request_log_serializes_codex_bridge_metadata() {
    let value = serde_json::to_value(RequestLog {
        timestamp_ms: 1,
        request_id: Some(42),
        trace_id: Some(request_trace_id("codex", 42)),
        service: "codex",
        method: "POST",
        path: "/v1/responses/compact",
        status_code: 200,
        duration_ms: 123,
        ttfb_ms: Some(10),
        station_name: Some("relay"),
        provider_id: Some("relay".to_string()),
        endpoint_id: None,
        provider_endpoint_key: None,
        upstream_base_url: "https://relay.example/v1",
        session_id: Some("sid-1".to_string()),
        session_identity_source: Some(crate::state::SessionIdentitySource::PromptCacheKey),
        cwd: Some("/workdir".to_string()),
        model: Some("gpt-5".to_string()),
        reasoning_effort: Some("medium".to_string()),
        service_tier: ServiceTierLog::default(),
        codex_bridge: Some(CodexBridgeLog {
            patch_mode: "official-imagegen".to_string(),
            remote_compaction_v1_request: true,
            remote_compaction_v2_request: true,
            downgraded_to_responses_compact: true,
            responses_websocket_request: false,
            strips_client_auth: true,
        }),
        usage: None,
        http_debug: None,
        http_debug_ref: None,
        route_decision: None,
        retry: None,
        provider_signals: Vec::new(),
        policy_actions: Vec::new(),
    })
    .expect("serialize request log");

    assert_eq!(
        value["codex_bridge"]["patch_mode"].as_str(),
        Some("official-imagegen")
    );
    assert_eq!(
        value["codex_bridge"]["remote_compaction_v1_request"].as_bool(),
        Some(true)
    );
    assert_eq!(
        value["codex_bridge"]["remote_compaction_v2_request"].as_bool(),
        Some(true)
    );
    assert_eq!(
        value["codex_bridge"]["downgraded_to_responses_compact"].as_bool(),
        Some(true)
    );
    assert_eq!(
        value["codex_bridge"]["strips_client_auth"].as_bool(),
        Some(true)
    );
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
    assert_eq!(attempts[0].stable_code(), "failed_transport");
    assert_eq!(attempts[0].code.as_deref(), Some("failed_transport"));
    assert_eq!(
        attempts[0].error_class.as_deref(),
        Some("upstream_transport_error")
    );
    assert_eq!(attempts[0].model.as_deref(), Some("gpt-5"));
    assert_eq!(attempts[0].reason.as_deref(), Some("operation timed out"));

    assert_eq!(attempts[1].decision, "skipped_capability_mismatch");
    assert_eq!(
        attempts[1].code.as_deref(),
        Some("skipped_capability_mismatch")
    );
    assert!(attempts[1].skipped);
    assert_eq!(attempts[1].station_name.as_deref(), Some("alpha"));
    assert_eq!(attempts[1].model.as_deref(), Some("gpt-5"));

    assert_eq!(attempts[2].decision, "completed");
    assert_eq!(attempts[2].code.as_deref(), Some("completed"));
    assert_eq!(attempts[2].status_code, Some(200));
    assert_eq!(attempts[2].station_name, None);
    assert_eq!(
        attempts[2].upstream_base_url.as_deref(),
        Some("https://api.vibe.example/v1")
    );

    assert_eq!(attempts[3].decision, "all_upstreams_avoided");
    assert_eq!(attempts[3].code.as_deref(), Some("all_upstreams_avoided"));
    assert_eq!(attempts[3].reason.as_deref(), Some("total=3"));
    assert!(attempts[3].skipped);
}

#[test]
fn route_attempts_are_derived_from_provider_endpoint_retry_chain() {
    let chain = vec![
        "endpoint=codex/input/default group=0 compat_station=routing upstream_index=0 url=https://input.example/v1 status=502 class=server_error model=gpt-5".to_string(),
        "endpoint=codex/right/default group=1 compat_station=routing upstream_index=2 url=https://right.example/v1 skipped_unsupported_model=gpt-5".to_string(),
    ];

    let attempts = parse_route_attempts_from_chain(&chain);

    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[0].provider_endpoint_key.as_deref(),
        Some("codex/input/default")
    );
    assert_eq!(attempts[0].provider_id.as_deref(), Some("input"));
    assert_eq!(attempts[0].endpoint_id.as_deref(), Some("default"));
    assert_eq!(attempts[0].preference_group, Some(0));
    assert_eq!(attempts[0].station_name.as_deref(), Some("routing"));
    assert_eq!(attempts[0].upstream_index, Some(0));
    assert_eq!(
        attempts[0].upstream_base_url.as_deref(),
        Some("https://input.example/v1")
    );
    assert_eq!(attempts[0].decision, "failed_status");
    assert_eq!(attempts[0].code.as_deref(), Some("failed_status"));

    assert_eq!(
        attempts[1].provider_endpoint_key.as_deref(),
        Some("codex/right/default")
    );
    assert_eq!(attempts[1].provider_id.as_deref(), Some("right"));
    assert_eq!(attempts[1].preference_group, Some(1));
    assert_eq!(attempts[1].decision, "skipped_capability_mismatch");
    assert_eq!(
        attempts[1].code.as_deref(),
        Some("skipped_capability_mismatch")
    );
    assert!(attempts[1].skipped);
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
        value["route_attempts"][0]["code"].as_str(),
        Some("failed_transport")
    );
    assert!(value["route_attempts"][0]["station_name"].is_null());
    assert!(value["route_attempts"][0]["upstream_index"].is_null());
    assert_eq!(
        value["route_attempts"][1]["decision"].as_str(),
        Some("completed")
    );
    assert_eq!(
        value["route_attempts"][1]["code"].as_str(),
        Some("completed")
    );
}

#[test]
fn legacy_route_attempt_without_code_keeps_stable_code_fallback() {
    let attempt: RouteAttemptLog = serde_json::from_value(serde_json::json!({
        "attempt_index": 0,
        "decision": "failed_status",
        "status_code": 429,
        "error_class": "upstream_rate_limited",
        "raw": "status=429 class=upstream_rate_limited"
    }))
    .expect("legacy route attempt");

    assert_eq!(attempt.code, None);
    assert_eq!(attempt.stable_code(), "failed_status");
}

#[test]
fn retry_info_reads_legacy_route_attempt_station_identity() {
    let retry: RetryInfo = serde_json::from_value(serde_json::json!({
        "attempts": 1,
        "upstream_chain": [],
        "route_attempts": [
            {
                "attempt_index": 0,
                "decision": "completed",
                "station_name": "legacy",
                "upstream_index": 2,
                "upstream_base_url": "https://legacy.example/v1",
                "raw": "legacy:https://legacy.example/v1 (idx=2) status=200"
            }
        ]
    }))
    .expect("deserialize legacy retry info");

    assert_eq!(retry.route_attempts.len(), 1);
    assert_eq!(
        retry.route_attempts[0].station_name.as_deref(),
        Some("legacy")
    );
    assert_eq!(retry.route_attempts[0].upstream_index, Some(2));
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
