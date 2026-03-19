use super::*;

#[test]
fn refresh_attached_prefers_operator_summary_for_runtime_card_fields() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "shared_capabilities": {
            "session_observability": true,
            "request_history": true
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
        "surface_capabilities": {
            "operator_summary": true
        },
        "endpoints": [
            "/__codex_helper/api/v1/operator/summary",
            "/__codex_helper/api/v1/status/active",
            "/__codex_helper/api/v1/status/recent",
            "/__codex_helper/api/v1/status/session-stats",
            "/__codex_helper/api/v1/status/health-checks",
            "/__codex_helper/api/v1/status/station-health",
            "/__codex_helper/api/v1/runtime/status"
        ]
    });
    let summary = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "runtime": {
            "runtime_loaded_at_ms": 55,
            "runtime_source_mtime_ms": 34,
            "configured_active_station": "summary-station",
            "effective_active_station": "summary-station",
            "global_station_override": "summary-station",
            "configured_default_profile": "fast",
            "default_profile": "fast",
            "default_profile_summary": {
                "name": "fast",
                "station": "summary-station",
                "model": "gpt-5.4-mini",
                "reasoning_effort": "low",
                "service_tier": "priority",
                "fast_mode": true
            }
        },
        "counts": {
            "active_requests": 0,
            "recent_requests": 0,
            "sessions": 1,
            "stations": 1,
            "profiles": 2,
            "providers": 1
        },
        "retry": {
            "configured_profile": "balanced",
            "supports_write": false,
            "upstream_max_attempts": 2,
            "provider_max_attempts": 3,
            "allow_cross_station_before_first_output": true,
            "recent_retried_requests": 2,
            "recent_cross_station_failovers": 1,
            "recent_fast_mode_requests": 2
        },
        "health": {
            "stations_draining": 1,
            "stations_breaker_open": 0,
            "stations_half_open": 0,
            "stations_with_active_health_checks": 1,
            "stations_with_probe_failures": 1,
            "stations_with_degraded_passive_health": 0,
            "stations_with_failing_passive_health": 1,
            "stations_with_cooldown": 1,
            "stations_with_usage_exhaustion": 1
        },
        "session_cards": [
            {
                "session_id": "sid-summary",
                "observation_scope": "observed_only",
                "last_client_name": "Frank-Desk",
                "last_client_addr": "100.64.0.12",
                "cwd": "G:/codes/demo",
                "active_count": 1,
                "last_model": "gpt-5.4-mini",
                "last_reasoning_effort": "low",
                "last_service_tier": "priority",
                "effective_model": {
                    "value": "gpt-5.4-mini",
                    "source": "request_payload"
                },
                "effective_reasoning_effort": {
                    "value": "low",
                    "source": "request_payload"
                },
                "effective_service_tier": {
                    "value": "priority",
                    "source": "request_payload"
                },
                "effective_station": {
                    "value": "summary-station",
                    "source": "global_override"
                }
            }
        ],
        "stations": [
            {
                "name": "summary-station",
                "enabled": true,
                "level": 1
            }
        ],
        "profiles": [
            {
                "name": "fast",
                "station": "summary-station",
                "model": "gpt-5.4-mini",
                "reasoning_effort": "low",
                "service_tier": "priority",
                "fast_mode": true,
                "is_default": true
            },
            {
                "name": "balanced",
                "station": "fallback-station",
                "model": "gpt-5.4",
                "reasoning_effort": "medium",
                "service_tier": "default",
                "fast_mode": false,
                "is_default": false
            }
        ],
        "providers": [
            {
                "name": "right",
                "configured_enabled": true,
                "effective_enabled": true,
                "routable_endpoints": 1,
                "endpoints": [
                    {
                        "provider_name": "right",
                        "name": "primary",
                        "base_url": "https://right.example.com/v1",
                        "configured_enabled": true,
                        "effective_enabled": true,
                        "routable": true
                    }
                ]
            }
        ],
        "surface_capabilities": {
            "operator_summary": true
        },
        "shared_capabilities": {
            "session_observability": true,
            "request_history": true
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
            "/__codex_helper/api/v1/operator/summary",
            get({
                let summary = summary.clone();
                move || {
                    let summary = summary.clone();
                    async move { Json(summary) }
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
                    "loaded_at_ms": 12,
                    "source_mtime_ms": 13,
                    "retry": {
                        "upstream": {
                            "max_attempts": 1,
                            "backoff_ms": 100,
                            "backoff_max_ms": 200,
                            "jitter_ms": 10,
                            "on_status": "429,500-599",
                            "on_class": ["transport"],
                            "strategy": "failover"
                        },
                        "provider": {
                            "max_attempts": 2,
                            "backoff_ms": 100,
                            "backoff_max_ms": 200,
                            "jitter_ms": 10,
                            "on_status": "429,500-599",
                            "on_class": ["transport"],
                            "strategy": "failover"
                        },
                        "allow_cross_station_before_first_output": false,
                        "never_on_status": "",
                        "never_on_class": [],
                        "cloudflare_challenge_cooldown_secs": 0,
                        "cloudflare_timeout_cooldown_secs": 0,
                        "transport_cooldown_secs": 0,
                        "cooldown_backoff_factor": 1,
                        "cooldown_backoff_max_secs": 0
                    }
                }))
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4100, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4100, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let snapshot = controller.snapshot().expect("attached snapshot");
    assert_eq!(
        snapshot.configured_active_station.as_deref(),
        Some("summary-station")
    );
    assert_eq!(
        snapshot.effective_active_station.as_deref(),
        Some("summary-station")
    );
    assert_eq!(snapshot.default_profile.as_deref(), Some("fast"));
    assert!(snapshot.supports_operator_summary_api);
    assert_eq!(
        snapshot
            .operator_runtime_summary
            .as_ref()
            .and_then(|summary| summary.default_profile_summary.as_ref())
            .map(|profile| profile.fast_mode),
        Some(true)
    );
    assert_eq!(
        snapshot
            .operator_retry_summary
            .as_ref()
            .map(|summary| summary.provider_max_attempts),
        Some(3)
    );
    assert_eq!(
        snapshot
            .operator_retry_summary
            .as_ref()
            .map(|summary| summary.recent_retried_requests),
        Some(2)
    );
    assert_eq!(
        snapshot
            .operator_retry_summary
            .as_ref()
            .map(|summary| summary.recent_cross_station_failovers),
        Some(1)
    );
    assert_eq!(
        snapshot
            .operator_retry_summary
            .as_ref()
            .map(|summary| summary.recent_fast_mode_requests),
        Some(2)
    );
    assert_eq!(
        snapshot
            .operator_counts
            .as_ref()
            .map(|counts| counts.sessions),
        Some(1)
    );
    assert_eq!(
        snapshot
            .operator_counts
            .as_ref()
            .map(|counts| counts.providers),
        Some(1)
    );
    assert_eq!(
        snapshot
            .operator_health_summary
            .as_ref()
            .map(|summary| summary.stations_with_failing_passive_health),
        Some(1)
    );
    assert_eq!(snapshot.session_cards.len(), 1);
    assert_eq!(
        snapshot.session_cards[0].session_id.as_deref(),
        Some("sid-summary")
    );
    assert_eq!(
        snapshot.session_cards[0]
            .effective_station
            .as_ref()
            .map(|value| value.value.as_str()),
        Some("summary-station")
    );
    assert_eq!(snapshot.stations.len(), 1);
    assert_eq!(snapshot.stations[0].name, "summary-station");
    assert_eq!(snapshot.profiles.len(), 2);
    assert_eq!(snapshot.profiles[0].name, "fast");
    assert!(snapshot.profiles[0].fast_mode);
    assert_eq!(snapshot.providers.len(), 1);
    assert_eq!(snapshot.providers[0].name, "right");
    assert_eq!(snapshot.providers[0].endpoints.len(), 1);
    assert_eq!(
        snapshot.providers[0].endpoints[0].base_url,
        "https://right.example.com/v1"
    );

    handle.abort();
}

#[test]
fn refresh_attached_uses_operator_summary_links_for_follow_up_reads() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let caps = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "shared_capabilities": {
            "session_observability": true,
            "request_history": true
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
        "surface_capabilities": {
            "operator_summary": true,
            "snapshot": true,
            "profiles": true,
            "retry_config": true,
            "station_specs": true,
            "provider_specs": true
        },
        "endpoints": [
            "/__codex_helper/api/v1/operator/summary"
        ]
    });
    let summary = serde_json::json!({
        "api_version": 1,
        "service_name": "codex",
        "runtime": {
            "configured_active_station": "linked-station",
            "effective_active_station": "linked-station",
            "configured_default_profile": "linked-fast",
            "default_profile": "linked-fast"
        },
        "counts": {
            "active_requests": 0,
            "recent_requests": 0,
            "sessions": 0,
            "stations": 1,
            "profiles": 1,
            "providers": 1
        },
        "retry": {
            "configured_profile": "balanced",
            "supports_write": true,
            "upstream_max_attempts": 2,
            "provider_max_attempts": 2,
            "allow_cross_station_before_first_output": true,
            "recent_retried_requests": 1,
            "recent_cross_station_failovers": 1,
            "recent_fast_mode_requests": 0
        },
        "health": null,
        "session_cards": [],
        "stations": [
            {
                "name": "linked-station",
                "enabled": true,
                "level": 1
            }
        ],
        "profiles": [
            {
                "name": "linked-fast",
                "station": "linked-station",
                "service_tier": "priority",
                "fast_mode": true,
                "is_default": true
            }
        ],
        "providers": [],
        "links": {
            "snapshot": "/__alt/v1/snapshot",
            "status_active": "/__alt/v1/status/active",
            "runtime_status": "/__alt/v1/runtime/status",
            "runtime_reload": "/__alt/v1/runtime/reload",
            "status_recent": "/__alt/v1/status/recent",
            "status_session_stats": "/__alt/v1/status/session-stats",
            "status_health_checks": "/__alt/v1/status/health-checks",
            "status_station_health": "/__alt/v1/status/station-health",
            "control_trace": "/__alt/v1/control-trace",
            "retry_config": "/__alt/v1/retry/config",
            "sessions": "/__alt/v1/sessions",
            "session_by_id_template": "/__alt/v1/sessions/{session_id}",
            "session_overrides": "/__alt/v1/overrides/session",
            "global_station_override": "/__alt/v1/overrides/global-station",
            "stations": "/__alt/v1/stations",
            "station_by_name_template": "/__alt/v1/stations/{name}",
            "station_specs": "/__alt/v1/stations/specs",
            "station_spec_by_name_template": "/__alt/v1/stations/specs/{name}",
            "station_probe": "/__alt/v1/stations/probe",
            "healthcheck_start": "/__alt/v1/healthcheck/start",
            "healthcheck_cancel": "/__alt/v1/healthcheck/cancel",
            "providers": "/__alt/v1/providers",
            "provider_specs": "/__alt/v1/providers/specs",
            "provider_spec_by_name_template": "/__alt/v1/providers/specs/{name}",
            "profiles": "/__alt/v1/profiles",
            "profile_by_name_template": "/__alt/v1/profiles/{name}",
            "default_profile": "/__alt/v1/profiles/default",
            "persisted_default_profile": "/__alt/v1/profiles/default/persisted"
        },
        "surface_capabilities": {
            "operator_summary": true,
            "snapshot": true,
            "profiles": true,
            "retry_config": true,
            "station_specs": true,
            "provider_specs": true
        },
        "shared_capabilities": {
            "session_observability": true,
            "request_history": true
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
    });
    let mut snapshot_payload = sample_snapshot(vec![sample_station("linked-station")]);
    snapshot_payload.default_profile = Some("linked-fast".to_string());

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
            "/__codex_helper/api/v1/operator/summary",
            get({
                let summary = summary.clone();
                move || {
                    let summary = summary.clone();
                    async move { Json(summary) }
                }
            }),
        )
        .route(
            "/__alt/v1/profiles",
            get(|| async {
                Json(serde_json::json!({
                    "default_profile": "linked-fast",
                    "configured_default_profile": "linked-fast",
                    "profiles": [
                        {
                            "name": "linked-fast",
                            "station": "linked-station",
                            "service_tier": "priority",
                            "fast_mode": true,
                            "is_default": true
                        }
                    ]
                }))
            }),
        )
        .route(
            "/__alt/v1/retry/config",
            get(|| async {
                Json(serde_json::json!({
                    "configured": {
                        "profile": "balanced"
                    },
                    "resolved": {
                        "upstream": {
                            "max_attempts": 2,
                            "backoff_ms": 200,
                            "backoff_max_ms": 2000,
                            "jitter_ms": 100,
                            "on_status": "429,500-599,524",
                            "on_class": ["upstream_transport_error"],
                            "strategy": "same_upstream"
                        },
                        "provider": {
                            "max_attempts": 2,
                            "backoff_ms": 0,
                            "backoff_max_ms": 0,
                            "jitter_ms": 0,
                            "on_status": "401,403,404,408,429,500-599,524",
                            "on_class": ["upstream_transport_error"],
                            "strategy": "failover"
                        },
                        "allow_cross_station_before_first_output": true,
                        "never_on_status": "",
                        "never_on_class": [],
                        "cloudflare_challenge_cooldown_secs": 0,
                        "cloudflare_timeout_cooldown_secs": 0,
                        "transport_cooldown_secs": 0,
                        "cooldown_backoff_factor": 1,
                        "cooldown_backoff_max_secs": 0
                    }
                }))
            }),
        )
        .route(
            "/__alt/v1/stations/specs",
            get(|| async {
                Json(serde_json::json!({
                    "stations": [
                        {
                            "name": "linked-station",
                            "enabled": true,
                            "level": 1
                        }
                    ],
                    "providers": [
                        {
                            "name": "right",
                            "enabled": true,
                            "endpoints": [
                                {
                                    "name": "primary",
                                    "base_url": "https://right.example.com/v1",
                                    "enabled": true
                                }
                            ]
                        }
                    ]
                }))
            }),
        )
        .route(
            "/__alt/v1/providers/specs",
            get(|| async {
                Json(serde_json::json!({
                    "providers": [
                        {
                            "name": "right",
                            "enabled": true,
                            "auth_token_env": "RIGHT_API_KEY",
                            "endpoints": [
                                {
                                    "name": "primary",
                                    "base_url": "https://right.example.com/v1",
                                    "enabled": true,
                                    "priority": 0
                                }
                            ]
                        }
                    ]
                }))
            }),
        )
        .route(
            "/__alt/v1/snapshot",
            get({
                let snapshot_payload = snapshot_payload.clone();
                move || {
                    let snapshot_payload = snapshot_payload.clone();
                    async move { Json(snapshot_payload) }
                }
            }),
        );
    let (base_url, handle) = spawn_test_server(&rt, app);

    let mut controller = ProxyController::new(4101, ServiceKind::Codex);
    controller.request_attach_with_admin_base(4101, Some(base_url));
    controller.refresh_attached_if_due(&rt, Duration::ZERO);

    let attached = controller.attached().expect("attached state");
    assert_eq!(
        attached
            .operator_summary_links
            .as_ref()
            .map(|links| links.retry_config.as_str()),
        Some("/__alt/v1/retry/config")
    );
    assert!(attached.persisted_stations.contains_key("linked-station"));
    assert!(attached.persisted_providers.contains_key("right"));

    let snapshot = controller.snapshot().expect("attached snapshot");
    assert_eq!(snapshot.default_profile.as_deref(), Some("linked-fast"));
    assert_eq!(
        snapshot
            .configured_retry
            .as_ref()
            .and_then(|retry| retry.profile),
        Some(crate::config::RetryProfileName::Balanced)
    );
    assert_eq!(snapshot.stations.len(), 1);
    assert_eq!(snapshot.stations[0].name, "linked-station");

    handle.abort();
}
