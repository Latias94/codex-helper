use super::*;

#[test]
fn request_attach_with_discovered_proxy_preloads_surface_capabilities() {
    let mut controller = ProxyController::new(4230, ServiceKind::Codex);
    controller.discovered = vec![DiscoveredProxy {
        port: 4230,
        base_url: "http://127.0.0.1:4230".to_string(),
        admin_base_url: "http://127.0.0.1:5230".to_string(),
        api_version: Some(1),
        service_name: Some("codex".to_string()),
        endpoints: Vec::new(),
        surface_capabilities: crate::dashboard_core::ControlPlaneSurfaceCapabilities {
            operator_summary: true,
            retry_config: true,
            station_specs: true,
            provider_specs: true,
            default_profile_override: true,
            session_override_reset: true,
            control_trace: true,
            request_ledger_recent: true,
            stations: true,
            station_runtime: true,
            ..Default::default()
        },
        runtime_loaded_at_ms: Some(777),
        operator_runtime_summary: Some(crate::dashboard_core::OperatorRuntimeSummary {
            runtime_loaded_at_ms: Some(777),
            runtime_source_mtime_ms: Some(778),
            configured_active_station: Some("alpha".to_string()),
            effective_active_station: Some("beta".to_string()),
            global_station_override: Some("gamma".to_string()),
            global_route_target_override: None,
            configured_default_profile: Some("steady".to_string()),
            default_profile: Some("fast".to_string()),
            default_profile_summary: None,
        }),
        operator_retry_summary: Some(crate::dashboard_core::OperatorRetrySummary {
            configured_profile: Some(crate::config::RetryProfileName::Balanced),
            supports_write: true,
            upstream_max_attempts: 2,
            provider_max_attempts: 3,
            allow_cross_station_before_first_output: true,
            recent_retried_requests: 0,
            recent_cross_station_failovers: 0,
            recent_same_station_retries: 0,
            recent_fast_mode_requests: 0,
        }),
        operator_health_summary: Some(crate::dashboard_core::OperatorHealthSummary {
            stations_draining: 1,
            stations_breaker_open: 0,
            stations_half_open: 0,
            stations_with_active_health_checks: 1,
            stations_with_probe_failures: 0,
            stations_with_degraded_passive_health: 0,
            stations_with_failing_passive_health: 0,
            stations_with_cooldown: 0,
            stations_with_usage_exhaustion: 0,
        }),
        operator_counts: Some(crate::dashboard_core::OperatorSummaryCounts {
            active_requests: 1,
            recent_requests: 2,
            sessions: 3,
            stations: 4,
            profiles: 5,
            providers: 6,
        }),
        last_error: None,
        shared_capabilities: crate::dashboard_core::SharedControlPlaneCapabilities {
            session_observability: true,
            request_history: false,
        },
        host_local_capabilities: crate::dashboard_core::HostLocalControlPlaneCapabilities {
            session_history: false,
            transcript_read: true,
            cwd_enrichment: false,
        },
        remote_admin_access: crate::dashboard_core::RemoteAdminAccessCapabilities {
            loopback_without_token: false,
            remote_requires_token: true,
            remote_enabled: true,
            token_header: "X-Test-Token".to_string(),
            token_env_var: "TEST_TOKEN_ENV".to_string(),
        },
    }];

    controller.request_attach_with_admin_base(4230, Some("http://127.0.0.1:5230".to_string()));

    let attached = controller.attached().expect("attached status");
    assert_eq!(attached.base_url, "http://127.0.0.1:4230");
    assert_eq!(attached.admin_base_url, "http://127.0.0.1:5230");
    assert_eq!(attached.api_version, Some(1));
    assert_eq!(attached.service_name.as_deref(), Some("codex"));
    assert_eq!(attached.runtime_loaded_at_ms, Some(777));
    assert_eq!(attached.runtime_source_mtime_ms, Some(778));
    assert_eq!(attached.configured_active_station.as_deref(), Some("alpha"));
    assert_eq!(attached.effective_active_station.as_deref(), Some("beta"));
    assert_eq!(attached.global_station_override.as_deref(), Some("gamma"));
    assert_eq!(
        attached.configured_default_profile.as_deref(),
        Some("steady")
    );
    assert_eq!(attached.default_profile.as_deref(), Some("fast"));
    assert!(attached.supports_operator_summary_api);
    assert!(attached.supports_retry_config_api);
    assert!(attached.supports_provider_spec_api);
    assert!(attached.supports_station_spec_api);
    assert!(attached.supports_default_profile_override);
    assert!(attached.supports_session_override_reset);
    assert!(attached.supports_control_trace_api);
    assert!(attached.supports_request_ledger_api);
    assert!(attached.supports_station_api);
    assert!(attached.supports_station_runtime_override);
    assert_eq!(
        attached
            .operator_counts
            .as_ref()
            .map(|counts| counts.sessions),
        Some(3)
    );
    assert!(attached.shared_capabilities.session_observability);
    assert!(attached.host_local_capabilities.transcript_read);
    assert!(attached.remote_admin_access.remote_enabled);
    assert_eq!(attached.remote_admin_access.token_header, "X-Test-Token");
}

#[test]
fn scan_local_proxies_captures_operator_summary_for_discovered_proxy() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "surface_capabilities": {
            "operator_summary": true,
            "stations": true,
            "profiles": true,
            "retry_config": true
        },
        "shared_capabilities": {
            "session_observability": true,
            "request_history": false
        },
        "host_local_capabilities": {
            "session_history": false,
            "transcript_read": false,
            "cwd_enrichment": false
        },
        "remote_admin_access": {
            "loopback_without_token": true,
            "remote_requires_token": true,
            "remote_enabled": false,
            "token_header": crate::proxy::ADMIN_TOKEN_HEADER,
            "token_env_var": crate::proxy::ADMIN_TOKEN_ENV_VAR
        },
        "endpoints": [
            "/__codex_helper/api/v1/operator/summary",
            "/__codex_helper/api/v1/runtime/status"
        ]
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
            "/__codex_helper/api/v1/runtime/status",
            get(|| async {
                Json(serde_json::json!({
                    "loaded_at_ms": 901,
                    "source_mtime_ms": 902,
                }))
            }),
        )
        .route(
            "/__codex_helper/api/v1/operator/summary",
            get(|| async {
                Json(serde_json::json!({
                    "api_version": 1,
                    "service_name": "codex",
                    "runtime": {
                        "runtime_loaded_at_ms": 901,
                        "runtime_source_mtime_ms": 902,
                        "configured_active_station": "right",
                        "effective_active_station": "vibe",
                        "global_station_override": "vibe",
                        "configured_default_profile": "balanced",
                        "default_profile": "fast"
                    },
                    "counts": {
                        "active_requests": 1,
                        "recent_requests": 2,
                        "sessions": 3,
                        "stations": 4,
                        "profiles": 5
                    },
                    "retry": {
                        "configured_profile": "balanced",
                        "supports_write": true,
                        "upstream_max_attempts": 2,
                        "provider_max_attempts": 3,
                        "allow_cross_station_before_first_output": true
                    },
                    "health": {
                        "stations_draining": 1,
                        "stations_breaker_open": 0,
                        "stations_half_open": 0,
                        "stations_with_active_health_checks": 0,
                        "stations_with_probe_failures": 0,
                        "stations_with_degraded_passive_health": 0,
                        "stations_with_failing_passive_health": 0,
                        "stations_with_cooldown": 0,
                        "stations_with_usage_exhaustion": 0
                    },
                    "session_cards": [],
                    "stations": [],
                    "profiles": [],
                    "providers": [],
                    "surface_capabilities": {
                        "operator_summary": true
                    },
                    "shared_capabilities": {
                        "session_observability": true,
                        "request_history": false
                    },
                    "host_local_capabilities": {
                        "session_history": false,
                        "transcript_read": false,
                        "cwd_enrichment": false
                    },
                    "remote_admin_access": {
                        "loopback_without_token": true,
                        "remote_requires_token": true,
                        "remote_enabled": false,
                        "token_header": crate::proxy::ADMIN_TOKEN_HEADER,
                        "token_env_var": crate::proxy::ADMIN_TOKEN_ENV_VAR
                    }
                }))
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);
    let port = base_url
        .rsplit(':')
        .next()
        .expect("port text")
        .parse::<u16>()
        .expect("port number");

    let mut controller = ProxyController::new(port, ServiceKind::Codex);
    controller
        .scan_local_proxies(&rt, port..=port)
        .expect("scan local proxies");

    let discovered = controller.discovered_proxies();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].port, port);
    assert_eq!(discovered[0].runtime_loaded_at_ms, Some(901));
    assert_eq!(
        discovered[0]
            .operator_runtime_summary
            .as_ref()
            .and_then(|runtime| runtime.effective_active_station.as_deref()),
        Some("vibe")
    );
    assert_eq!(
        discovered[0]
            .operator_counts
            .as_ref()
            .map(|counts| counts.sessions),
        Some(3)
    );
    assert_eq!(
        discovered[0]
            .operator_health_summary
            .as_ref()
            .map(|health| health.stations_draining),
        Some(1)
    );
    assert_eq!(
        discovered[0]
            .operator_retry_summary
            .as_ref()
            .map(|retry| retry.provider_max_attempts),
        Some(3)
    );

    handle.abort();
}

#[test]
fn refresh_attached_supports_partial_station_surface_with_canonical_effort_api() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "shared_capabilities": {
            "session_observability": true,
            "request_history": false
        },
        "host_local_capabilities": {
            "session_history": false,
            "transcript_read": true,
            "cwd_enrichment": false
        },
        "remote_admin_access": {
            "loopback_without_token": true,
            "remote_requires_token": false,
            "remote_enabled": false,
            "token_header": crate::proxy::ADMIN_TOKEN_HEADER,
            "token_env_var": crate::proxy::ADMIN_TOKEN_ENV_VAR
        },
        "endpoints": [
            "/__codex_helper/api/v1/status/active",
            "/__codex_helper/api/v1/status/recent",
            "/__codex_helper/api/v1/status/session-stats",
            "/__codex_helper/api/v1/status/health-checks",
            "/__codex_helper/api/v1/status/station-health",
            "/__codex_helper/api/v1/runtime/status",
            "/__codex_helper/api/v1/stations",
            "/__codex_helper/api/v1/stations/runtime",
            "/__codex_helper/api/v1/overrides/global-station",
            "/__codex_helper/api/v1/overrides/session/station",
            "/__codex_helper/api/v1/overrides/session/effort",
            "/__codex_helper/api/v1/overrides/session/model"
        ]
    });
    let stations = vec![sample_station("station-partial")];
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
                    "loaded_at_ms": 31,
                    "source_mtime_ms": 32,
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
            get(|| async { Json(Some("station-partial".to_string())) }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/station",
            get(|| async {
                Json(HashMap::from([(
                    "sid-v1".to_string(),
                    "station-partial".to_string(),
                )]))
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/effort",
            get(|| async {
                Json(HashMap::from([(
                    "sid-v1".to_string(),
                    "medium".to_string(),
                )]))
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/model",
            get(|| async {
                Json(HashMap::from([(
                    "sid-v1".to_string(),
                    "gpt-5.4".to_string(),
                )]))
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4201, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4201, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("partial station snapshot");
    assert!(snapshot.supports_v1);
    assert_eq!(snapshot.stations.len(), 1);
    assert_eq!(snapshot.stations[0].name, "station-partial");
    assert_eq!(
        snapshot.global_station_override.as_deref(),
        Some("station-partial")
    );
    assert_eq!(
        snapshot
            .session_station_overrides
            .get("sid-v1")
            .map(String::as_str),
        Some("station-partial")
    );
    assert_eq!(
        snapshot
            .session_effort_overrides
            .get("sid-v1")
            .map(String::as_str),
        Some("medium")
    );
    assert_eq!(
        snapshot
            .session_model_overrides
            .get("sid-v1")
            .map(String::as_str),
        Some("gpt-5.4")
    );
    assert!(snapshot.session_service_tier_overrides.is_empty());
    assert!(snapshot.shared_capabilities.session_observability);
    assert!(snapshot.host_local_capabilities.transcript_read);
    assert!(!snapshot.supports_default_profile_override);
    assert!(!snapshot.supports_session_override_reset);
    let attached = controller.attached().expect("partial attached status");
    assert_eq!(attached.api_version, Some(1));
    assert_eq!(attached.runtime_loaded_at_ms, Some(31));
    assert_eq!(attached.runtime_source_mtime_ms, Some(32));
    assert!(attached.supports_station_api);
    assert!(attached.supports_station_runtime_override);
    assert!(!attached.remote_admin_access.remote_enabled);
    assert!(!attached.remote_admin_access.remote_requires_token);

    handle.abort();
}

#[test]
fn refresh_attached_prefers_typed_surface_capabilities_over_endpoint_strings() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "surface_capabilities": {
            "status_active": true,
            "status_recent": true,
            "status_session_stats": true,
            "status_health_checks": true,
            "status_station_health": true,
            "runtime_status": true,
            "stations": true,
            "station_runtime": true,
            "global_station_override": true,
            "global_route_override": true,
            "session_station_override": true,
            "session_route_override": true,
            "session_reasoning_effort_override": true,
            "session_model_override": true
        },
        "shared_capabilities": {
            "session_observability": true,
            "request_history": false
        },
        "host_local_capabilities": {
            "session_history": false,
            "transcript_read": false,
            "cwd_enrichment": false
        },
        "endpoints": []
    });
    let stations = vec![sample_station("typed-surface")];
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
                    "loaded_at_ms": 41,
                    "source_mtime_ms": 42,
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
            get(|| async { Json(Some("typed-surface".to_string())) }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/global-route",
            get(|| async { Json(Some("typed-route".to_string())) }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/station",
            get(|| async {
                Json(HashMap::from([(
                    "sid-typed".to_string(),
                    "typed-surface".to_string(),
                )]))
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/route",
            get(|| async {
                Json(HashMap::from([(
                    "sid-typed".to_string(),
                    "typed-route".to_string(),
                )]))
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/effort",
            get(|| async {
                Json(HashMap::from([(
                    "sid-typed".to_string(),
                    "high".to_string(),
                )]))
            }),
        )
        .route(
            "/__codex_helper/api/v1/overrides/session/model",
            get(|| async {
                Json(HashMap::from([(
                    "sid-typed".to_string(),
                    "gpt-5.4-fast".to_string(),
                )]))
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4202, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4202, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("typed surface snapshot");
    assert!(snapshot.supports_v1);
    assert_eq!(snapshot.stations.len(), 1);
    assert_eq!(snapshot.stations[0].name, "typed-surface");
    assert_eq!(
        snapshot
            .session_station_overrides
            .get("sid-typed")
            .map(String::as_str),
        Some("typed-surface")
    );
    assert_eq!(
        snapshot.global_route_target_override.as_deref(),
        Some("typed-route")
    );
    assert_eq!(
        snapshot
            .session_route_target_overrides
            .get("sid-typed")
            .map(String::as_str),
        Some("typed-route")
    );
    assert_eq!(
        snapshot
            .session_effort_overrides
            .get("sid-typed")
            .map(String::as_str),
        Some("high")
    );
    assert_eq!(
        snapshot
            .session_model_overrides
            .get("sid-typed")
            .map(String::as_str),
        Some("gpt-5.4-fast")
    );
    let attached = controller.attached().expect("typed attached status");
    assert!(attached.supports_station_api);
    assert!(attached.supports_station_runtime_override);
    assert!(attached.supports_global_route_target_override);
    assert!(attached.supports_session_route_target_override);
    assert!(!attached.supports_default_profile_override);

    handle.abort();
}

#[test]
fn refresh_attached_rejects_pre_v1_runtime_surface() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let app = Router::new()
        .route(
            "/__codex_helper/status/active",
            get(|| async { Json(Vec::<ActiveRequest>::new()) }),
        )
        .route(
            "/__codex_helper/status/recent",
            get(|| async { Json(Vec::<FinishedRequest>::new()) }),
        )
        .route(
            "/__codex_helper/config/runtime",
            get(|| async {
                Json(serde_json::json!({
                    "loaded_at_ms": 51,
                    "source_mtime_ms": 52,
                }))
            }),
        )
        .route(
            "/__codex_helper/override/session",
            get(|| async {
                Json(HashMap::from([(
                    "sid-legacy".to_string(),
                    "low".to_string(),
                )]))
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4202, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4202, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("attached snapshot");
    assert!(!snapshot.supports_v1);
    assert!(snapshot.stations.is_empty());
    assert!(snapshot.session_effort_overrides.is_empty());
    assert!(snapshot.last_error.is_some());
    let attached = controller.attached().expect("attached status");
    assert_eq!(attached.api_version, None);
    assert!(attached.last_error.is_some());
    assert_eq!(attached.runtime_loaded_at_ms, None);
    assert_eq!(attached.runtime_source_mtime_ms, None);
    assert!(!attached.supports_station_api);
    assert!(!attached.supports_station_runtime_override);

    handle.abort();
}
