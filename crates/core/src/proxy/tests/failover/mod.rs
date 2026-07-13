use super::*;

mod config_failover;
mod response_semantics;

const UPSTREAM_TWO_SUCCESS_SSE: &[u8] = b"data: {\"ok\":true,\"upstream\":2}\n\ndata: {\"type\":\"response.completed\"}\n\ndata: [DONE]\n\n";
const BACKUP_SUCCESS_SSE: &[u8] = b"data: {\"ok\":true,\"upstream\":\"backup\"}\n\ndata: {\"type\":\"response.completed\"}\n\ndata: [DONE]\n\n";

fn retry_layer_config(
    max_attempts: u32,
    on_status: &str,
    on_class: Vec<String>,
    strategy: RetryStrategy,
) -> crate::config::RetryLayerConfig {
    crate::config::RetryLayerConfig {
        max_attempts: Some(max_attempts),
        backoff_ms: Some(0),
        backoff_max_ms: Some(0),
        jitter_ms: Some(0),
        on_status: Some(on_status.to_string()),
        on_class: Some(on_class),
        strategy: Some(strategy),
    }
}

fn retry_config(
    max_attempts: u32,
    on_status: &str,
    on_class: Vec<String>,
    strategy: RetryStrategy,
) -> RetryConfig {
    retry_config_with_cooldowns(max_attempts, on_status, on_class, strategy, 0, 0, 0)
}

fn retry_config_with_cooldowns(
    max_attempts: u32,
    on_status: &str,
    on_class: Vec<String>,
    strategy: RetryStrategy,
    cloudflare_challenge_cooldown_secs: u64,
    cloudflare_timeout_cooldown_secs: u64,
    transport_cooldown_secs: u64,
) -> RetryConfig {
    RetryConfig {
        upstream: Some(retry_layer_config(
            max_attempts,
            on_status,
            on_class,
            strategy,
        )),
        cloudflare_challenge_cooldown_secs: Some(cloudflare_challenge_cooldown_secs),
        cloudflare_timeout_cooldown_secs: Some(cloudflare_timeout_cooldown_secs),
        transport_cooldown_secs: Some(transport_cooldown_secs),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    }
}

fn capacity_wait_config(primary_base_url: String, backup_base_url: Option<String>) -> HelperConfig {
    let mut providers = std::collections::BTreeMap::from([(
        "primary".to_string(),
        ProviderConfig {
            base_url: Some(primary_base_url),
            inline_auth: UpstreamAuth::default(),
            limits: ProviderConcurrencyLimits {
                max_concurrent_requests: Some(1),
                limit_group: None,
            },
            ..ProviderConfig::default()
        },
    )]);
    let mut children = vec!["primary".to_string()];
    if let Some(backup_base_url) = backup_base_url {
        providers.insert(
            "backup".to_string(),
            ProviderConfig {
                base_url: Some(backup_base_url),
                inline_auth: UpstreamAuth::default(),
                ..ProviderConfig::default()
            },
        );
        children.push("backup".to_string());
    }
    let mut routing = RouteGraphConfig::ordered_failover(children);
    routing.scheduling_preset = SchedulingPreset::ContinuityFirst;

    HelperConfig {
        codex: ServiceRouteConfig {
            providers,
            routing: Some(routing),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    }
}

async fn wait_for_provider_pending(
    proxy: &ProxyService,
    provider_id: &str,
    max_concurrent_requests: u32,
    expected: u32,
) {
    let runtime_revision = proxy.config.capture().await.revision();
    let limit = crate::proxy::concurrency_limits::ConcurrencyLimit::new(
        max_concurrent_requests,
        runtime_revision,
    )
    .expect("test provider concurrency limit must be non-zero");
    let provider_endpoint = crate::runtime_identity::ProviderEndpointKey::new(
        proxy.service_name,
        provider_id,
        "default",
    );
    let key = format!("endpoint:{}", provider_endpoint.stable_key());

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if proxy
                .concurrency_limiter
                .snapshot(key.as_str(), limit)
                .pending
                == expected
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider pending waiter count should converge");
}

#[tokio::test]
async fn proxy_failover_retries_502_then_uses_second_upstream() {
    run_failover_retries_502_then_uses_second_upstream().await;
}

#[tokio::test]
async fn production_request_path_uses_route_plan_executor() {
    let before = super::super::provider_execution::route_executor_request_path_test_invocations();

    run_failover_retries_502_then_uses_second_upstream().await;

    let after = super::super::provider_execution::route_executor_request_path_test_invocations();
    assert!(after > before);
}

async fn run_failover_retries_502_then_uses_second_upstream() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "nope" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "upstream": 2 })),
            )
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = retry_config(2, "502", Vec::new(), RetryStrategy::Failover);
    let cfg = make_helper_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(proxy_client, Arc::new(cfg), "codex");
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let req = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#);
    let resp = req.send().await.expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(
        body.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body}"
    );
    let finished = state.list_recent_finished(10).await;
    let request = finished
        .iter()
        .find(|request| request.status_code == StatusCode::OK)
        .expect("finished request");
    let retry = request.retry.as_ref().expect("retry info");
    assert_eq!(retry.route_attempts.len(), 3);
    assert_eq!(
        retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.endpoint_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("default"), Some("default"), Some("default")]
    );
    assert_eq!(
        retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.provider_endpoint_key.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("codex/u1/default"),
            Some("codex/u1/default"),
            Some("codex/u2/default")
        ]
    );
    assert_eq!(
        retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.preference_group)
            .collect::<Vec<_>>(),
        vec![Some(0), Some(0), Some(1)]
    );
    assert_eq!(
        retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.route_path.clone())
            .collect::<Vec<_>>(),
        vec![
            vec!["main".to_string(), "u1".to_string()],
            vec!["main".to_string(), "u1".to_string()],
            vec!["main".to_string(), "u2".to_string()],
        ]
    );
    let route_decision = request
        .route_decision
        .as_ref()
        .expect("finished route decision");
    assert_eq!(route_decision.endpoint_id.as_deref(), Some("default"));
    assert_eq!(route_decision.route_path, vec!["main", "u2"]);
    // Two-layer model: retry current upstream first, then fail over.
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_route_graph_route_graph_affinity_is_session_scoped() {
    let input_hits = Arc::new(AtomicUsize::new(0));
    let input1_hits = Arc::new(AtomicUsize::new(0));
    let right_hits = Arc::new(AtomicUsize::new(0));

    let input_counter = input_hits.clone();
    let input = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let input_counter = input_counter.clone();
            async move {
                let hit = input_counter.fetch_add(1, Ordering::SeqCst) + 1;
                if hit == 2 {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "input", "err": "quota" })),
                    )
                } else {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "input" })),
                    )
                }
            }
        }),
    );
    let (input_addr, input_handle) = spawn_axum_server(input);

    let input1_counter = input1_hits.clone();
    let input1 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let input1_counter = input1_counter.clone();
            async move {
                let hit = input1_counter.fetch_add(1, Ordering::SeqCst) + 1;
                if hit == 1 {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "input1", "err": "quota" })),
                    )
                } else {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "input1" })),
                    )
                }
            }
        }),
    );
    let (input1_addr, input1_handle) = spawn_axum_server(input1);

    let right_counter = right_hits.clone();
    let right = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let right_counter = right_counter.clone();
            async move {
                right_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "right" })),
                )
            }
        }),
    );
    let (right_addr, right_handle) = spawn_axum_server(right);

    let retry = RetryConfig {
        profile: Some(RetryProfileName::AggressiveFailover),
        upstream: Some(retry_layer_config(
            1,
            "502",
            Vec::new(),
            RetryStrategy::Failover,
        )),
        provider: Some(retry_layer_config(
            4,
            "502",
            Vec::new(),
            RetryStrategy::Failover,
        )),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let monthly_tags =
        std::collections::BTreeMap::from([("billing".to_string(), "monthly".to_string())]);
    let source = HelperConfig {
        retry,
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "input".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{input_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        tags: monthly_tags.clone(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "input1".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{input1_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        tags: monthly_tags,
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "right".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{right_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "monthly_first".to_string(),
                routes: std::collections::BTreeMap::from([
                    (
                        "monthly_first".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::TagPreferred,
                            children: vec!["monthly_pool".to_string(), "right".to_string()],
                            prefer_tags: vec![std::collections::BTreeMap::from([(
                                "billing".to_string(),
                                "monthly".to_string(),
                            )])],
                            on_exhausted: crate::config::RouteExhaustedAction::Continue,
                            ..RouteNodeConfig::default()
                        },
                    ),
                    (
                        "monthly_pool".to_string(),
                        RouteNodeConfig {
                            strategy: RouteStrategy::OrderedFailover,
                            children: vec!["input".to_string(), "input1".to_string()],
                            on_exhausted: crate::config::RouteExhaustedAction::Continue,
                            ..RouteNodeConfig::default()
                        },
                    ),
                ]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let first = send_responses_json(&client, proxy_addr, Some("sid-input")).await;
    assert_eq!(first["provider"].as_str(), Some("input"));

    let fallback = send_responses_json(&client, proxy_addr, Some("sid-right")).await;
    assert_eq!(fallback["provider"].as_str(), Some("right"));
    assert_eq!(input_hits.load(Ordering::SeqCst), 2);
    assert_eq!(input1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(right_hits.load(Ordering::SeqCst), 1);
    let finished_after_fallback = state.list_recent_finished(10).await;
    let fallback_request = finished_after_fallback
        .iter()
        .find(|request| {
            request.session_id.as_deref() == Some("sid-right")
                && request.provider_id.as_deref() == Some("right")
        })
        .expect("fallback finished request");
    assert_eq!(
        fallback_request
            .route_decision
            .as_ref()
            .and_then(|decision| decision.endpoint_id.as_deref()),
        Some("default")
    );
    let fallback_retry = fallback_request
        .retry
        .as_ref()
        .expect("fallback request retry trace");
    assert_eq!(
        fallback_retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.provider_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("input"), Some("input1"), Some("right")]
    );
    assert_eq!(
        fallback_retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.provider_endpoint_key.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("codex/input/default"),
            Some("codex/input1/default"),
            Some("codex/right/default"),
        ]
    );
    assert_eq!(
        fallback_retry
            .route_attempts
            .iter()
            .map(|attempt| (attempt.provider_max_attempts, attempt.upstream_max_attempts))
            .collect::<Vec<_>>(),
        vec![(Some(4), Some(1)), (Some(4), Some(1)), (Some(4), Some(1))]
    );
    assert_eq!(
        fallback_retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.avoided_candidate_indices.clone())
            .collect::<Vec<_>>(),
        vec![Vec::<usize>::new(), vec![0], vec![0, 1]]
    );
    let fallback_affinity_snapshot = state
        .get_session_route_affinity("sid-right")
        .await
        .expect("right affinity after fallback");
    assert_eq!(
        fallback_affinity_snapshot
            .provider_endpoint
            .provider_id
            .as_str(),
        "right"
    );
    assert_eq!(
        fallback_affinity_snapshot.change_reason.as_str(),
        "failover_after_status_502"
    );

    let sticky_after_fallback = send_responses_json(&client, proxy_addr, Some("sid-right")).await;
    assert_eq!(sticky_after_fallback["provider"].as_str(), Some("right"));

    let sticky = send_responses_json(&client, proxy_addr, Some("sid-input")).await;
    assert_eq!(sticky["provider"].as_str(), Some("input"));

    let new_session = send_responses_json(&client, proxy_addr, Some("sid-new")).await;
    assert_eq!(new_session["provider"].as_str(), Some("input"));

    assert_eq!(input_hits.load(Ordering::SeqCst), 4);
    assert_eq!(input1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(right_hits.load(Ordering::SeqCst), 2);

    let affinities = state.list_session_route_affinities().await;
    assert_eq!(
        affinities
            .get("sid-input")
            .map(|affinity| affinity.provider_endpoint.provider_id.as_str()),
        Some("input")
    );
    let fallback_affinity = affinities.get("sid-right").expect("right affinity");
    assert_eq!(
        fallback_affinity.provider_endpoint.provider_id.as_str(),
        "right"
    );
    assert_eq!(
        fallback_affinity.provider_endpoint.stable_key(),
        "codex/right/default"
    );
    assert_eq!(
        fallback_affinity.change_reason.as_str(),
        "failover_after_status_502"
    );

    let cards = state.list_session_identity_cards(20).await;
    let right_card = cards
        .iter()
        .find(|card| card.session_id.as_deref() == Some("sid-right"))
        .expect("right card");
    assert_eq!(
        right_card
            .route_affinity
            .as_ref()
            .map(|affinity| affinity.provider_endpoint.provider_id.as_str()),
        Some("right")
    );

    proxy_handle.abort();
    input_handle.abort();
    input1_handle.abort();
    right_handle.abort();
}

#[tokio::test]
async fn proxy_route_graph_route_graph_health_does_not_write_synthetic_routing_lb_state() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let primary_counter = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            async move {
                let hit = primary_counter.fetch_add(1, Ordering::SeqCst) + 1;
                if hit == 1 {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "primary", "err": "first" })),
                    )
                } else {
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "primary" })),
                    )
                }
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup" })),
                )
            }
        }),
    );
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let retry = RetryConfig {
        upstream: Some(retry_layer_config(
            1,
            "502",
            Vec::new(),
            RetryStrategy::Failover,
        )),
        provider: Some(retry_layer_config(
            2,
            "502",
            Vec::new(),
            RetryStrategy::Failover,
        )),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let source = HelperConfig {
        retry,
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "monthly_first".to_string(),
                routes: std::collections::BTreeMap::from([(
                    "monthly_first".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::OrderedFailover,
                        children: vec!["primary".to_string(), "backup".to_string()],
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let fallback = send_responses_json(&client, proxy_addr, Some("sid-failover")).await;
    assert_eq!(fallback["provider"].as_str(), Some("backup"));
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);
    let sticky = send_responses_json(&client, proxy_addr, Some("sid-failover")).await;
    assert_eq!(sticky["provider"].as_str(), Some("backup"));
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_route_graph_balanced_scheduling_waits_for_preferred_capacity() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));
    let primary_started = Arc::new(tokio::sync::Notify::new());
    let release_primary = Arc::new(tokio::sync::Notify::new());

    let primary_counter = primary_hits.clone();
    let primary_started_for_route = primary_started.clone();
    let release_primary_for_route = release_primary.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            let primary_started = primary_started_for_route.clone();
            let release_primary = release_primary_for_route.clone();
            async move {
                let hit = primary_counter.fetch_add(1, Ordering::SeqCst);
                if hit == 0 {
                    primary_started.notify_one();
                    release_primary.notified().await;
                }
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "primary" })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup" })),
                )
            }
        }),
    );
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        limits: ProviderConcurrencyLimits {
                            max_concurrent_requests: Some(1),
                            limit_group: None,
                        },
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "main".to_string(),
                routes: std::collections::BTreeMap::from([(
                    "main".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::OrderedFailover,
                        children: vec!["primary".to_string(), "backup".to_string()],
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let first_request = tokio::spawn({
        let client = client.clone();
        async move { send_responses_json(&client, proxy_addr, Some("sid-primary")).await }
    });
    tokio::time::timeout(Duration::from_secs(2), primary_started.notified())
        .await
        .expect("primary request should acquire the only local concurrency permit");

    let second_request = tokio::spawn({
        let client = client.clone();
        async move { send_responses_json(&client, proxy_addr, Some("sid-second")).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let second_finished_early = second_request.is_finished();
    let backup_hits_before_release = backup_hits.load(Ordering::SeqCst);

    release_primary.notify_one();
    let first = tokio::time::timeout(Duration::from_secs(2), first_request)
        .await
        .expect("first request should finish after release")
        .expect("first request task should join");
    let second = tokio::time::timeout(Duration::from_secs(2), second_request)
        .await
        .expect("second request should acquire released preferred capacity")
        .expect("second request task should join");

    assert!(
        !second_finished_early,
        "balanced request must wait for capacity"
    );
    assert_eq!(backup_hits_before_release, 0);
    assert_eq!(first["provider"].as_str(), Some("primary"));
    assert_eq!(second["provider"].as_str(), Some("primary"));
    assert_eq!(primary_hits.load(Ordering::SeqCst), 2);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_http_capacity_wait_keeps_captured_runtime_snapshot_across_reload() {
    let _env_guard = env_lock().await;
    let codex_home = make_temp_test_dir();
    let helper_home = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HOME", &codex_home);
        scoped.set_path("CODEX_HELPER_HOME", &helper_home);
    }

    let old_hits = Arc::new(AtomicUsize::new(0));
    let first_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let old_counter = old_hits.clone();
    let first_started_for_route = first_started.clone();
    let release_first_for_route = release_first.clone();
    let old_upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let old_counter = old_counter.clone();
            let first_started = first_started_for_route.clone();
            let release_first = release_first_for_route.clone();
            async move {
                let hit = old_counter.fetch_add(1, Ordering::SeqCst) + 1;
                if hit == 1 {
                    first_started.notify_one();
                    release_first.notified().await;
                }
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "runtime": "old", "hit": hit })),
                )
            }
        }),
    );
    let (old_addr, old_handle) = spawn_axum_server(old_upstream);

    let new_hits = Arc::new(AtomicUsize::new(0));
    let new_counter = new_hits.clone();
    let new_upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let new_counter = new_counter.clone();
            async move {
                let hit = new_counter.fetch_add(1, Ordering::SeqCst) + 1;
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "runtime": "new", "hit": hit })),
                )
            }
        }),
    );
    let (new_addr, new_handle) = spawn_axum_server(new_upstream);

    let initial = capacity_wait_config(format!("http://{old_addr}/v1"), None);
    crate::config::save_helper_config(&initial)
        .await
        .expect("save initial capacity route config");
    let loaded = crate::config::load_config_with_source()
        .await
        .expect("load initial capacity route config");
    let proxy = proxy_with_loaded_route_graph_config(loaded);
    let retained = proxy.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let first_request = tokio::spawn({
        let client = client.clone();
        async move { send_responses_json(&client, proxy_addr, Some("sid-reload-first")).await }
    });
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first request should occupy the old primary capacity");

    let queued_request = tokio::spawn({
        let client = client.clone();
        async move { send_responses_json(&client, proxy_addr, Some("sid-reload-queued")).await }
    });
    wait_for_provider_pending(&retained, "primary", 1, 1).await;

    let reloaded = capacity_wait_config(format!("http://{new_addr}/v1"), None);
    crate::config::save_helper_config(&reloaded)
        .await
        .expect("save reloaded capacity route config");
    assert!(
        retained
            .config
            .force_reload_from_disk()
            .await
            .expect("reload capacity route config"),
        "changed primary origin should publish a new runtime snapshot"
    );

    release_first.notify_one();
    let first = tokio::time::timeout(Duration::from_secs(2), first_request)
        .await
        .expect("first request should finish after release")
        .expect("first request task should join");
    let queued = tokio::time::timeout(Duration::from_secs(2), queued_request)
        .await
        .expect("queued request should acquire the released old capacity")
        .expect("queued request task should join");
    let next = send_responses_json(&client, proxy_addr, Some("sid-reload-next")).await;

    assert_eq!(first["runtime"].as_str(), Some("old"));
    assert_eq!(queued["runtime"].as_str(), Some("old"));
    assert_eq!(next["runtime"].as_str(), Some("new"));
    assert_eq!(old_hits.load(Ordering::SeqCst), 2);
    assert_eq!(new_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    old_handle.abort();
    new_handle.abort();
    let _ = std::fs::remove_dir_all(codex_home);
    let _ = std::fs::remove_dir_all(helper_home);
}

#[tokio::test]
async fn proxy_http_capacity_wait_keeps_captured_policy_until_next_request() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let first_started = Arc::new(tokio::sync::Notify::new());
    let release_first = Arc::new(tokio::sync::Notify::new());
    let primary_counter = primary_hits.clone();
    let first_started_for_route = first_started.clone();
    let release_first_for_route = release_first.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            let first_started = first_started_for_route.clone();
            let release_first = release_first_for_route.clone();
            async move {
                let hit = primary_counter.fetch_add(1, Ordering::SeqCst) + 1;
                if hit == 1 {
                    first_started.notify_one();
                    release_first.notified().await;
                }
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "primary", "hit": hit })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let backup_hits = Arc::new(AtomicUsize::new(0));
    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                let hit = backup_counter.fetch_add(1, Ordering::SeqCst) + 1;
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup", "hit": hit })),
                )
            }
        }),
    );
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let source = capacity_wait_config(
        format!("http://{primary_addr}/v1"),
        Some(format!("http://{backup_addr}/v1")),
    );
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let retained = proxy.clone();
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let first_request = tokio::spawn({
        let client = client.clone();
        async move { send_responses_json(&client, proxy_addr, Some("sid-policy-first")).await }
    });
    tokio::time::timeout(Duration::from_secs(2), first_started.notified())
        .await
        .expect("first request should occupy the primary capacity");

    let queued_request = tokio::spawn({
        let client = client.clone();
        async move { send_responses_json(&client, proxy_addr, Some("sid-policy-queued")).await }
    });
    wait_for_provider_pending(&retained, "primary", 1, 1).await;

    state
        .set_provider_manual_eligibility(
            crate::runtime_identity::ProviderEndpointKey::new("codex", "primary", "default"),
            crate::runtime_store::ProviderManualEligibility::Disabled,
            Some("test disables primary after the HTTP request is queued".to_string()),
            crate::logging::now_ms(),
        )
        .await
        .expect("commit manual primary eligibility");
    retained
        .config
        .publish_provider_policy(state.capture_provider_policy_snapshot().await)
        .await
        .expect("publish disabled primary policy snapshot");

    release_first.notify_one();
    let first = tokio::time::timeout(Duration::from_secs(2), first_request)
        .await
        .expect("first request should finish after release")
        .expect("first request task should join");
    let queued = tokio::time::timeout(Duration::from_secs(2), queued_request)
        .await
        .expect("queued request should acquire capacity under its captured policy")
        .expect("queued request task should join");
    let next = send_responses_json(&client, proxy_addr, Some("sid-policy-next")).await;

    assert_eq!(first["provider"].as_str(), Some("primary"));
    assert_eq!(queued["provider"].as_str(), Some("primary"));
    assert_eq!(next["provider"].as_str(), Some("backup"));
    assert_eq!(primary_hits.load(Ordering::SeqCst), 2);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_route_graph_route_graph_skips_provider_when_local_concurrency_limit_is_saturated() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));
    let primary_started = Arc::new(tokio::sync::Notify::new());
    let release_primary = Arc::new(tokio::sync::Notify::new());

    let primary_counter = primary_hits.clone();
    let primary_started_for_route = primary_started.clone();
    let release_primary_for_route = release_primary.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let primary_counter = primary_counter.clone();
            let primary_started = primary_started_for_route.clone();
            let release_primary = release_primary_for_route.clone();
            async move {
                primary_counter.fetch_add(1, Ordering::SeqCst);
                primary_started.notify_one();
                release_primary.notified().await;
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "primary" })),
                )
            }
        }),
    );
    let (primary_addr, primary_handle) = spawn_axum_server(primary);

    let backup_counter = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let backup_counter = backup_counter.clone();
            async move {
                backup_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "backup" })),
                )
            }
        }),
    );
    let (backup_addr, backup_handle) = spawn_axum_server(backup);

    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{primary_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        limits: ProviderConcurrencyLimits {
                            max_concurrent_requests: Some(1),
                            limit_group: None,
                        },
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{backup_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "main".to_string(),
                scheduling_preset: SchedulingPreset::ThroughputFirst,
                routes: std::collections::BTreeMap::from([(
                    "main".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::OrderedFailover,
                        children: vec!["primary".to_string(), "backup".to_string()],
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let proxy_for_explain = proxy.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let first_request = tokio::spawn({
        let client = client.clone();
        async move { send_responses_json(&client, proxy_addr, Some("sid-primary")).await }
    });
    tokio::time::timeout(Duration::from_secs(2), primary_started.notified())
        .await
        .expect("primary request should acquire the only local concurrency permit");

    let explain = proxy_for_explain
        .routing_explain(
            crate::routing_ir::RouteRequestContext::default(),
            Some("sid-second".to_string()),
        )
        .await
        .expect("build routing explain");
    let explain = serde_json::to_value(explain).expect("serialize routing explain");
    assert_eq!(
        explain["selected_route"]["provider_id"].as_str(),
        Some("backup")
    );
    assert_eq!(
        explain["candidates"][0]["skip_reasons"][0]["code"].as_str(),
        Some("concurrency_saturated")
    );
    assert_eq!(
        explain["candidates"][0]["skip_reasons"][0]["active"].as_u64(),
        Some(1)
    );
    assert_eq!(
        explain["candidates"][0]["skip_reasons"][0]["limit"].as_u64(),
        Some(1)
    );
    assert_eq!(
        explain["candidates"][0]["capacity"]["active"].as_u64(),
        Some(1)
    );
    assert_eq!(
        explain["candidates"][0]["capacity"]["limit"].as_u64(),
        Some(1)
    );
    assert_eq!(
        explain["candidates"][0]["capacity"]["effective_max_concurrent_requests"].as_u64(),
        Some(1)
    );
    assert_eq!(
        explain["candidates"][0]["capacity"]["saturated"].as_bool(),
        Some(true)
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["available"].as_bool(),
        Some(false)
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["runtime_available"].as_bool(),
        Some(false)
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["routable_except_usage"].as_bool(),
        Some(false)
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["dominant_reason"]["code"].as_str(),
        Some("concurrency_saturated")
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["concurrency_active"].as_u64(),
        Some(1)
    );
    assert_eq!(
        explain["candidates"][0]["availability"]["concurrency_limit"].as_u64(),
        Some(1)
    );

    let second = match tokio::time::timeout(
        Duration::from_secs(2),
        send_responses_json(&client, proxy_addr, Some("sid-second")),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            release_primary.notify_waiters();
            panic!("second request should fail over instead of waiting for primary: {error}");
        }
    };
    assert_eq!(second["provider"].as_str(), Some("backup"));
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    release_primary.notify_one();
    let first = tokio::time::timeout(Duration::from_secs(2), first_request)
        .await
        .expect("primary request should finish after release")
        .expect("primary request task should join");
    assert_eq!(first["provider"].as_str(), Some("primary"));
    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    primary_handle.abort();
    backup_handle.abort();
}

#[tokio::test]
async fn proxy_route_graph_conditional_routing_selects_branch_by_request_model() {
    let small_hits = Arc::new(AtomicUsize::new(0));
    let large_hits = Arc::new(AtomicUsize::new(0));

    let small_counter = small_hits.clone();
    let small = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            small_counter.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "provider": "small" })),
            )
        }),
    );
    let (small_addr, small_handle) = spawn_axum_server(small);

    let large_counter = large_hits.clone();
    let large = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            large_counter.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "provider": "large" })),
            )
        }),
    );
    let (large_addr, large_handle) = spawn_axum_server(large);

    let source = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "small".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{}/v1", small_addr)),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "large".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{}/v1", large_addr)),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig {
                entry: "root".to_string(),
                routes: std::collections::BTreeMap::from([(
                    "root".to_string(),
                    RouteNodeConfig {
                        strategy: RouteStrategy::Conditional,
                        when: Some(RouteCondition {
                            model: Some("gpt-5".to_string()),
                            ..RouteCondition::default()
                        }),
                        then: Some("large".to_string()),
                        default_route: Some("small".to_string()),
                        ..RouteNodeConfig::default()
                    },
                )]),
                ..RouteGraphConfig::default()
            }),
            ..ServiceRouteConfig::default()
        },
        ..HelperConfig::default()
    };
    let proxy = ProxyService::new(Client::new(), Arc::new(source), "codex");
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);
    let client = reqwest::Client::new();

    let large_resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send large branch")
        .error_for_status()
        .expect("large branch status")
        .text()
        .await
        .expect("large branch body");
    assert!(large_resp.contains(r#""provider":"large"#));

    let small_resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-4.1","input":"hi"}"#)
        .send()
        .await
        .expect("send small branch")
        .error_for_status()
        .expect("small branch status")
        .text()
        .await
        .expect("small branch body");
    assert!(small_resp.contains(r#""provider":"small"#));

    assert_eq!(large_hits.load(Ordering::SeqCst), 1);
    assert_eq!(small_hits.load(Ordering::SeqCst), 1);

    let finished = state.list_recent_finished(10).await;
    let route_paths = finished
        .iter()
        .filter_map(|request| request.route_decision.as_ref())
        .map(|decision| decision.route_path.clone())
        .collect::<Vec<_>>();
    assert!(
        route_paths
            .iter()
            .any(|path| path == &vec!["root", "large"])
    );
    assert!(
        route_paths
            .iter()
            .any(|path| path == &vec!["root", "small"])
    );

    proxy_handle.abort();
    small_handle.abort();
    large_handle.abort();
}

#[tokio::test]
async fn proxy_same_upstream_retries_502_then_succeeds_without_failover() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            let n = u1_hits.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "err": "first attempt 502" })),
                )
            } else {
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "ok": true, "upstream": 1 })),
                )
            }
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "ok": true, "upstream": 2 })),
            )
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = retry_config(2, "502", Vec::new(), RetryStrategy::SameUpstream);
    let cfg = make_helper_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(proxy_client, Arc::new(cfg), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(
        body.contains(r#""upstream":1"#),
        "expected response from upstream1, got: {body}"
    );
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_across_requests_penalizes_502_when_no_internal_retry() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "always 502" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(UPSTREAM_TWO_SUCCESS_SSE)]
                        .into_iter()
                        .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(2),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_helper_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(proxy_client, Arc::new(cfg), "codex");
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    // First request hits upstream1, gets a retryable 502, and fails over to upstream2.
    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body1_s}"
    );
    assert!(!body1_s.contains("response.failed"), "{body1_s}");
    let mut finished = Vec::new();
    for _ in 0..100 {
        finished = state.list_recent_finished(10).await;
        if finished.iter().any(|request| request.retry.is_some()) {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    let retry = finished
        .iter()
        .find_map(|request| request.retry.as_ref())
        .expect("streaming failover should record retry info");
    assert_eq!(retry.attempts, 2);
    assert_eq!(retry.route_attempts.len(), 2);
    assert_eq!(retry.route_attempts[0].decision, "failed_status");
    assert_eq!(retry.route_attempts[0].provider_id.as_deref(), Some("u1"));
    assert_eq!(retry.route_attempts[0].provider_attempt, Some(1));
    assert_eq!(retry.route_attempts[1].decision, "completed");
    assert_eq!(retry.route_attempts[1].provider_id.as_deref(), Some("u2"));
    assert_eq!(
        retry.route_attempts[1].endpoint_id.as_deref(),
        Some("default")
    );
    assert_eq!(
        retry.route_attempts[1].provider_endpoint_key.as_deref(),
        Some("codex/u2/default")
    );
    assert_eq!(retry.route_attempts[1].provider_attempt, Some(1));
    assert!(retry.route_attempts[1].upstream_headers_ms.is_some());

    // Second request should now go directly to upstream2 thanks to the cooldown on upstream1.
    let (status2, body2) = {
        let mut last_status = StatusCode::INTERNAL_SERVER_ERROR;
        let mut last_body: Bytes = Bytes::new();
        for attempt in 0..3 {
            let resp2 = client
                .post(format!("http://{}/v1/responses", proxy_addr))
                .header("content-type", "application/json")
                .header("accept", "text/event-stream")
                .body(r#"{"model":"gpt","input":"hi"}"#)
                .send()
                .await
                .expect("send");
            last_status = resp2.status();
            last_body = resp2.bytes().await.expect("read bytes");
            if last_status == StatusCode::OK {
                break;
            }
            if attempt < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }
        }
        (last_status, last_body)
    };
    assert_eq!(status2, StatusCode::OK);
    let body_s = String::from_utf8_lossy(&body2);
    assert!(
        body_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body_s}"
    );
    assert!(!body_s.contains("response.failed"), "{body_s}");

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_across_requests_penalizes_transport_error_when_no_internal_retry() {
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(UPSTREAM_TWO_SUCCESS_SSE)]
                        .into_iter()
                        .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let unused = reserve_unused_local_addr();

    let proxy_client = Client::new();
    let retry = retry_config_with_cooldowns(
        1,
        "502",
        vec!["upstream_transport_error".to_string()],
        RetryStrategy::Failover,
        0,
        0,
        60,
    );
    let cfg = make_helper_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", unused),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(proxy_client, Arc::new(cfg), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body1_s}"
    );
    assert!(!body1_s.contains("response.failed"), "{body1_s}");

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body = resp2.bytes().await.expect("read bytes");
    let body_s = String::from_utf8_lossy(&body);
    assert!(
        body_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body_s}"
    );
    assert!(!body_s.contains("response.failed"), "{body_s}");
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_failover_across_requests_penalizes_cloudflare_challenge_when_no_internal_retry() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            let mut resp = Response::new(Body::from(
                "<html><body>/cdn-cgi/ challenge-platform __CF$cv$params</body></html>",
            ));
            *resp.status_mut() = StatusCode::FORBIDDEN;
            resp.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            resp.headers_mut()
                .insert("server", HeaderValue::from_static("cloudflare"));
            resp.headers_mut()
                .insert("cf-ray", HeaderValue::from_static("test"));
            resp
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(UPSTREAM_TWO_SUCCESS_SSE)]
                        .into_iter()
                        .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = retry_config_with_cooldowns(
        1,
        "502",
        vec!["cloudflare_challenge".to_string()],
        RetryStrategy::Failover,
        60,
        0,
        0,
    );
    let cfg = make_helper_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u1".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: {
                    let mut t = HashMap::new();
                    t.insert("provider_id".to_string(), "u2".to_string());
                    t
                },
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(proxy_client, Arc::new(cfg), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body1_s}"
    );

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = resp2.bytes().await.expect("read bytes");
    let body2_s = String::from_utf8_lossy(&body2);
    assert!(
        body2_s.contains(r#""upstream":2"#),
        "expected response from upstream2, got: {body2_s}"
    );

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_multi_config_failover_across_requests_respects_cooldown() {
    let primary_hits = Arc::new(AtomicUsize::new(0));
    let backup_hits = Arc::new(AtomicUsize::new(0));

    let p_hits = primary_hits.clone();
    let primary = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            p_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "primary 502" })),
            )
        }),
    );
    let (p_addr, p_handle) = spawn_axum_server(primary);

    let b_hits = backup_hits.clone();
    let backup = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            b_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(BACKUP_SUCCESS_SSE)]
                        .into_iter()
                        .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (b_addr, b_handle) = spawn_axum_server(backup);

    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(2),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };

    let cfg = HelperConfig {
        codex: ServiceRouteConfig {
            providers: std::collections::BTreeMap::from([
                (
                    "primary".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{p_addr}/v1")),
                        ..ProviderConfig::default()
                    },
                ),
                (
                    "backup".to_string(),
                    ProviderConfig {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        ..ProviderConfig::default()
                    },
                ),
            ]),
            routing: Some(RouteGraphConfig::ordered_failover(vec![
                "primary".to_string(),
                "backup".to_string(),
            ])),
            ..ServiceRouteConfig::default()
        },
        retry,
        ..HelperConfig::default()
    };

    let proxy = ProxyService::new(Client::new(), Arc::new(cfg), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::OK);
    let body1 = resp1.bytes().await.expect("read bytes");
    let body1_s = String::from_utf8_lossy(&body1);
    assert!(
        body1_s.contains(r#""upstream":"backup""#),
        "expected response from backup, got: {body1_s}"
    );
    assert!(!body1_s.contains("response.failed"), "{body1_s}");

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = resp2.bytes().await.expect("read bytes");
    let body2_s = String::from_utf8_lossy(&body2);
    assert!(
        body2_s.contains(r#""upstream":"backup""#),
        "expected response from backup, got: {body2_s}"
    );
    assert!(!body2_s.contains("response.failed"), "{body2_s}");

    assert_eq!(primary_hits.load(Ordering::SeqCst), 1);
    assert_eq!(backup_hits.load(Ordering::SeqCst), 2);

    proxy_handle.abort();
    p_handle.abort();
    b_handle.abort();
}

#[tokio::test]
async fn proxy_does_not_failover_when_502_is_not_retryable_and_threshold_not_reached() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "always 502" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            async move {
                let s = stream::iter(
                    vec![Bytes::from_static(UPSTREAM_TWO_SUCCESS_SSE)]
                        .into_iter()
                        .map(Ok::<Bytes, Infallible>),
                );
                let mut resp = Response::new(Body::from_stream(s));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp
            }
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(60),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_helper_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(proxy_client, Arc::new(cfg), "codex");
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();

    let resp1 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp1.status(), StatusCode::BAD_GATEWAY);

    let resp2 = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp2.status(), StatusCode::BAD_GATEWAY);

    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 2);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn proxy_retries_each_upstream_once_and_stops_when_all_avoided() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let u1_hits = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u1_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "u1 502" })),
            )
        }),
    );
    let (u1_addr, u1_handle) = spawn_axum_server(upstream1);

    let u2_hits = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            u2_hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "u2 502" })),
            )
        }),
    );
    let (u2_addr, u2_handle) = spawn_axum_server(upstream2);

    let proxy_client = Client::new();
    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_helper_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{}/v1", u1_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{}/v1", u2_addr),
                auth: UpstreamAuth {
                    auth_token: None,
                    auth_token_env: None,
                    api_key: None,
                    api_key_env: None,
                },
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );

    let proxy = ProxyService::new(proxy_client, Arc::new(cfg), "codex");
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt","input":"hi","stream":true}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("read body");
    assert!(body.contains("event: response.failed"), "{body}");
    assert!(body.contains(r#""code":"rate_limit_exceeded""#), "{body}");
    assert!(
        body.contains(r#""codex_helper_error":"upstream_failure""#),
        "{body}"
    );
    assert!(body.contains("all upstream attempts failed"), "{body}");
    assert!(body.contains("endpoint=codex/test/default"), "{body}");
    assert!(body.contains("endpoint=codex/test-2/default"), "{body}");
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 1);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 1);

    let finished = state.list_recent_finished(1).await;
    let failed = finished
        .first()
        .expect("failed request should remain recorded as a 502");
    assert_eq!(failed.status_code, 502);
    let retry = failed.retry.as_ref().expect("retry trace");
    assert_eq!(
        retry
            .route_attempts
            .iter()
            .map(|attempt| attempt.decision.as_str())
            .collect::<Vec<_>>(),
        vec!["failed_status", "failed_status"]
    );

    proxy_handle.abort();
    u1_handle.abort();
    u2_handle.abort();
}

#[tokio::test]
async fn failed_single_attempt_records_route_attempts_for_logs() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || async move {
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "err": "single 502" })),
            )
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::SameUpstream),
        }),
        provider: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(1),
            backoff_ms: Some(0),
            backoff_max_ms: Some(0),
            jitter_ms: Some(0),
            on_status: Some("502".to_string()),
            on_class: Some(Vec::new()),
            strategy: Some(RetryStrategy::Failover),
        }),
        cloudflare_challenge_cooldown_secs: Some(0),
        cloudflare_timeout_cooldown_secs: Some(0),
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..Default::default()
    };
    let cfg = make_helper_config(
        vec![UpstreamConfig {
            base_url: format!("http://{}/v1", upstream_addr),
            auth: UpstreamAuth {
                auth_token: None,
                auth_token_env: None,
                api_key: None,
                api_key_env: None,
            },
            tags: {
                let mut tags = HashMap::new();
                tags.insert("provider_id".to_string(), "solo".to_string());
                tags
            },
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        retry,
    );

    let proxy = ProxyService::new(Client::new(), Arc::new(cfg), "codex");
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/responses", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt","input":"hi"}"#)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = resp.text().await.expect("read body");
    assert!(
        body.contains("all upstream attempts failed") && body.contains("single 502"),
        "expected single-attempt failure summary, got: {body}"
    );

    let finished = state.list_recent_finished(1).await;
    let retry = finished
        .first()
        .and_then(|request| request.retry.as_ref())
        .expect("failed single attempt should keep route attempts in request logs");
    assert_eq!(retry.attempts, 1);
    assert_eq!(retry.route_attempts.len(), 1);
    assert_eq!(retry.route_attempts[0].decision, "failed_status");
    assert_eq!(retry.route_attempts[0].provider_id.as_deref(), Some("solo"));
    assert_eq!(retry.route_attempts[0].status_code, Some(502));

    proxy_handle.abort();
    upstream_handle.abort();
}
