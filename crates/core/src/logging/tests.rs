use super::*;

#[test]
fn request_trace_id_is_versioned_and_stable_within_process() {
    let first = request_trace_id("codex", 42);
    let second = request_trace_id("codex", 42);

    assert_eq!(first, second);
    assert!(is_versioned_request_trace_id(&first));
    assert_ne!(first, legacy_request_trace_id("codex", 42));
}

#[test]
fn request_trace_id_changes_with_boot_uuid() {
    let first_boot =
        uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000001").expect("first boot UUID");
    let second_boot =
        uuid::Uuid::parse_str("00000000-0000-4000-8000-000000000002").expect("second boot UUID");

    let first = request_trace_id_for_boot(&first_boot, "codex", 42);
    let second = request_trace_id_for_boot(&second_boot, "codex", 42);

    assert_ne!(first, second);
    assert!(is_versioned_request_trace_id(&first));
    assert!(is_versioned_request_trace_id(&second));
    assert_eq!(legacy_request_trace_id("codex", 42), "codex-42");
}

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
        provider_id: Some("right".to_string()),
        endpoint_id: None,
        provider_endpoint_key: None,
        upstream_origin: Some("https://example.com".to_string()),
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
    assert_eq!(
        value["trace_id"].as_str(),
        Some(request_trace_id("codex", 42).as_str())
    );
    assert_eq!(value["model"].as_str(), Some("gpt-5"));
    assert_eq!(value["session_identity_source"].as_str(), Some("header"));
}

#[test]
fn request_log_serializes_canonical_provider_endpoint_identity() {
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
        provider_id: Some("input".to_string()),
        endpoint_id: Some("default".to_string()),
        provider_endpoint_key: Some("codex/input/default".to_string()),
        upstream_origin: Some("https://input.example".to_string()),
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

    assert!(value.get("station_name").is_none());
    assert_eq!(value["provider_id"].as_str(), Some("input"));
    assert_eq!(value["endpoint_id"].as_str(), Some("default"));
    assert_eq!(
        value["provider_endpoint_key"].as_str(),
        Some("codex/input/default")
    );
    assert_eq!(
        value["upstream_origin"].as_str(),
        Some("https://input.example")
    );
    assert!(value.get("upstream_base_url").is_none());
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
        provider_id: Some("relay".to_string()),
        endpoint_id: None,
        provider_endpoint_key: None,
        upstream_origin: Some("https://relay.example".to_string()),
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
        value["codex_bridge"]["strips_client_auth"].as_bool(),
        Some(true)
    );
}

#[test]
fn upstream_origin_uses_url_parser_and_drops_credentials_path_query_and_fragment() {
    let poisoned =
        "https://user:secret@relay.example.test:8443/private/secret-path?token=hidden#fragment";

    assert_eq!(
        upstream_origin(poisoned).as_deref(),
        Some("https://relay.example.test:8443")
    );
    assert_eq!(upstream_origin("not a URL"), None);
}

#[test]
fn request_log_route_decision_drops_full_upstream_url() {
    let poisoned =
        "https://user:secret@relay.example.test:8443/private/secret-path?token=hidden#fragment";
    let route_decision = route_decision_for_request_log(Some(RouteDecisionProvenance {
        effective_upstream_base_url: Some(crate::state::ResolvedRouteValue::new(
            poisoned,
            crate::state::RouteValueSource::RuntimeFallback,
        )),
        provider_id: Some("relay".to_string()),
        endpoint_id: Some("default".to_string()),
        ..RouteDecisionProvenance::default()
    }))
    .expect("route decision");
    let text = serde_json::to_string(&route_decision).expect("serialize route decision");

    assert!(route_decision.effective_upstream_base_url.is_none());
    for secret in ["user:secret", "secret-path", "token=hidden", "fragment"] {
        assert!(!text.contains(secret));
    }
}

#[test]
fn retry_info_serializes_structured_route_attempts() {
    let retry: RetryInfo = serde_json::from_value(serde_json::json!({
        "attempts": 2,
        "upstream_chain": ["raw upstream secret"],
        "route_attempts": [
            {
                "attempt_index": 0,
                "decision": "failed_transport",
                "code": "failed_transport",
                "error_class": "upstream_transport_error",
                "reason": "timeout",
                "upstream_base_url": "https://user:secret@example.test/v1?token=hidden",
                "raw": "structured failed attempt"
            },
            {
                "attempt_index": 1,
                "decision": "completed",
                "code": "completed",
                "status_code": 200,
                "raw": "structured completed attempt"
            }
        ]
    }))
    .expect("deserialize legacy retry info");

    let value = serde_json::to_value(&retry).expect("serialize retry info");

    assert_eq!(value["attempts"].as_u64(), Some(2));
    assert!(value.get("upstream_chain").is_none());
    assert_eq!(
        value["route_attempts"][0]["decision"].as_str(),
        Some("failed_transport")
    );
    assert_eq!(
        value["route_attempts"][0]["code"].as_str(),
        Some("failed_transport")
    );
    assert!(value["route_attempts"][0].get("station_name").is_none());
    assert!(value["route_attempts"][0].get("upstream_index").is_none());
    assert!(value["route_attempts"][0].get("raw").is_none());
    assert!(value["route_attempts"][0].get("reason").is_none());
    assert!(
        value["route_attempts"][0]
            .get("upstream_base_url")
            .is_none()
    );
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
fn route_attempt_with_unknown_decision_uses_unknown_stable_code() {
    let attempt: RouteAttemptLog = serde_json::from_value(serde_json::json!({
        "attempt_index": 0,
        "decision": "future_attempt_state",
        "raw": "future structured attempt"
    }))
    .expect("structured route attempt");

    assert_eq!(attempt.code, None);
    assert_eq!(attempt.stable_code(), "unknown");
}

#[test]
fn route_attempt_with_lifecycle_failure_uses_known_stable_code() {
    let mut attempt = RouteAttemptLog {
        attempt_index: 0,
        decision: "failed_lifecycle_store".to_string(),
        ..RouteAttemptLog::default()
    };

    assert_eq!(attempt.stable_code(), "failed_lifecycle_store");
    attempt.refresh_code();
    assert_eq!(attempt.code.as_deref(), Some("failed_lifecycle_store"));
}

#[test]
fn retry_info_preserves_structured_route_attempt_endpoint_identity() {
    let retry: RetryInfo = serde_json::from_value(serde_json::json!({
        "attempts": 1,
        "upstream_chain": [],
        "route_attempts": [
            {
                "attempt_index": 0,
                "decision": "completed",
                "provider_id": "primary",
                "endpoint_id": "responses",
                "provider_endpoint_key": "codex/primary/responses",
                "upstream_base_url": "https://primary.example/v1",
                "raw": "structured completed attempt"
            }
        ]
    }))
    .expect("deserialize structured retry info");

    assert_eq!(retry.route_attempts.len(), 1);
    assert_eq!(
        retry.route_attempts[0].provider_id.as_deref(),
        Some("primary")
    );
    assert_eq!(
        retry.route_attempts[0].endpoint_id.as_deref(),
        Some("responses")
    );
    assert_eq!(
        retry.route_attempts[0].provider_endpoint_key.as_deref(),
        Some("codex/primary/responses")
    );
}
