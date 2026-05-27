use super::*;

#[derive(Clone, Copy)]
enum CompactPolicyTransport {
    ResponsesCompact,
    RemoteCompactionV2,
}

impl CompactPolicyTransport {
    fn proxy_path(self) -> &'static str {
        match self {
            Self::ResponsesCompact => "/responses/compact",
            Self::RemoteCompactionV2 => "/v1/responses",
        }
    }

    fn request_body(self) -> &'static str {
        match self {
            Self::ResponsesCompact => {
                r#"{"model":"gpt-5","input":[{"type":"reasoning","encrypted_content":"state"}]}"#
            }
            Self::RemoteCompactionV2 => {
                r#"{"model":"gpt-5","input":[{"type":"message","role":"user","content":"compact me"},{"type":"compaction_trigger"}],"stream":true}"#
            }
        }
    }

    fn provider_counter(self, counters: &CompactProviderCounters) -> &AtomicUsize {
        match self {
            Self::ResponsesCompact => &counters.compact,
            Self::RemoteCompactionV2 => &counters.responses,
        }
    }
}

#[derive(Clone, Default)]
struct CompactProviderCounters {
    responses: Arc<AtomicUsize>,
    compact: Arc<AtomicUsize>,
}

struct CompactPolicyFixture {
    _scoped: ScopedEnv,
    _temp_dir: std::path::PathBuf,
    client: reqwest::Client,
    proxy_addr: std::net::SocketAddr,
    proxy_handle: tokio::task::JoinHandle<()>,
    b_handle: tokio::task::JoinHandle<()>,
    c_handle: tokio::task::JoinHandle<()>,
    state: Arc<crate::state::ProxyState>,
    b_counters: CompactProviderCounters,
    c_counters: CompactProviderCounters,
}

impl CompactPolicyFixture {
    async fn new(affinity_policy: crate::config::RoutingAffinityPolicyV5) -> Self {
        let temp_dir = make_temp_test_dir();
        let mut scoped = ScopedEnv::default();
        unsafe {
            scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
            scoped.set_path(
                "CODEX_HELPER_CONTROL_TRACE_PATH",
                temp_dir.join("logs").join("control_trace.jsonl").as_path(),
            );
            scoped.set("CODEX_HELPER_CONTROL_TRACE", "1");
        }

        let b_counters = CompactProviderCounters::default();
        let c_counters = CompactProviderCounters::default();
        let (b_addr, b_handle) =
            spawn_axum_server(compact_policy_upstream("b", b_counters.clone()));
        let (c_addr, c_handle) =
            spawn_axum_server(compact_policy_upstream("c", c_counters.clone()));

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
        let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
        routing.affinity_policy = affinity_policy;
        let v4 = ProxyConfigV4 {
            retry,
            codex: ServiceViewV4 {
                providers: std::collections::BTreeMap::from([
                    (
                        "b".to_string(),
                        ProviderConfigV4 {
                            base_url: Some(format!("http://{b_addr}/v1")),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfigV4::default()
                        },
                    ),
                    (
                        "c".to_string(),
                        ProviderConfigV4 {
                            base_url: Some(format!("http://{c_addr}/v1")),
                            inline_auth: UpstreamAuth::default(),
                            ..ProviderConfigV4::default()
                        },
                    ),
                ]),
                routing: Some(routing),
                ..ServiceViewV4::default()
            },
            ..ProxyConfigV4::default()
        };
        let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
        let proxy = ProxyService::new_with_v4_source(
            Client::new(),
            Arc::new(runtime),
            Some(Arc::new(v4)),
            "codex",
            Arc::new(std::sync::Mutex::new(HashMap::new())),
        );
        let state = proxy.state.clone();
        let app = crate::proxy::router(proxy);
        let (proxy_addr, proxy_handle) = spawn_axum_server(app);

        Self {
            _scoped: scoped,
            _temp_dir: temp_dir,
            client: reqwest::Client::new(),
            proxy_addr,
            proxy_handle,
            b_handle,
            c_handle,
            state,
            b_counters,
            c_counters,
        }
    }

    async fn post_compaction(
        &self,
        transport: CompactPolicyTransport,
        session_id: &str,
    ) -> reqwest::Response {
        self.client
            .post(format!(
                "http://{}{}",
                self.proxy_addr,
                transport.proxy_path()
            ))
            .header("content-type", "application/json")
            .header("session-id", session_id)
            .body(transport.request_body())
            .send()
            .await
            .expect("send compact policy request")
    }

    fn hits(&self, provider: &str, transport: CompactPolicyTransport) -> usize {
        let counters = match provider {
            "b" => &self.b_counters,
            "c" => &self.c_counters,
            provider => panic!("unknown compact policy provider: {provider}"),
        };
        transport.provider_counter(counters).load(Ordering::SeqCst)
    }

    async fn assert_affinity_provider(&self, session_id: &str, provider: &str) {
        let affinity = self
            .state
            .get_session_route_affinity(session_id)
            .await
            .expect("route affinity recorded after compact request");
        assert_eq!(affinity.provider_endpoint.provider_id.as_str(), provider);
    }

    fn request_log_record(&self, path: &str, status: StatusCode) -> serde_json::Value {
        let request_log =
            std::fs::read_to_string(crate::logging::request_log_path()).expect("read request log");
        request_log
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|record| {
                record["path"].as_str() == Some(path)
                    && record["status_code"].as_u64() == Some(status.as_u16() as u64)
            })
            .expect("compact request log record")
    }

    fn latest_route_continuity_block_trace(&self) -> serde_json::Value {
        crate::logging::read_recent_control_trace_entries(20)
            .expect("read recent control trace entries")
            .iter()
            .rev()
            .find(|entry| entry.event.as_deref() == Some("route_continuity_blocked"))
            .expect("route continuity blocked trace")
            .payload
            .clone()
    }
}

impl Drop for CompactPolicyFixture {
    fn drop(&mut self) {
        self.proxy_handle.abort();
        self.b_handle.abort();
        self.c_handle.abort();
    }
}

fn compact_policy_upstream(
    provider: &'static str,
    counters: CompactProviderCounters,
) -> axum::Router {
    let responses_counter = counters.responses.clone();
    let compact_counter = counters.compact.clone();
    axum::Router::new()
        .route(
            "/v1/responses",
            post(move |body: axum::body::Bytes| {
                let responses_counter = responses_counter.clone();
                async move {
                    responses_counter.fetch_add(1, Ordering::SeqCst);
                    let is_remote_compaction_v2 =
                        serde_json::from_slice::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|body| {
                                body.get("input")
                                    .and_then(serde_json::Value::as_array)
                                    .cloned()
                            })
                            .is_some_and(|items| {
                                items.iter().any(|item| {
                                    item.get("type").and_then(serde_json::Value::as_str)
                                        == Some("compaction_trigger")
                                })
                            });
                    let mut response = serde_json::json!({ "provider": provider });
                    if is_remote_compaction_v2 {
                        response["compact_v2"] = serde_json::Value::Bool(true);
                    }
                    (StatusCode::OK, Json(response))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let compact_counter = compact_counter.clone();
                async move {
                    compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": provider, "compact": true })),
                    )
                }
            }),
        )
}

#[tokio::test]
async fn proxy_forwards_responses_compact_to_upstream_v1_compact_path() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));

    let hits = upstream_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses/compact",
        post(move |body: axum::body::Bytes| async move {
            hits.fetch_add(1, Ordering::SeqCst);
            let value: serde_json::Value =
                serde_json::from_slice(&body).expect("compact body should parse");
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "compact": true,
                    "model": value.get("model").and_then(|model| model.as_str()).unwrap_or("")
                })),
            )
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_proxy_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );

    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let resp = post_compact_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp
        .json::<serde_json::Value>()
        .await
        .expect("response json");
    assert_eq!(body["compact"], true);
    assert_eq!(body["model"], "gpt-5");
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);

    find_finished_request(&state, 10, |request| request.path == "/responses/compact")
        .await
        .expect("expected compact request path to be visible in finished requests");
}

#[tokio::test]
async fn proxy_normalizes_responses_compact_body_before_forwarding() {
    let upstream_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));

    let seen_body = upstream_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses/compact",
        post(move |body: axum::body::Bytes| {
            let seen_body = seen_body.clone();
            async move {
                *seen_body.lock().expect("body lock") =
                    Some(serde_json::from_slice(&body).expect("compact body should parse"));
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "output": [
                            { "type": "compaction", "encrypted_content": "summary" }
                        ]
                    })),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_proxy_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = spawn_test_proxy(cfg);

    let client = reqwest::Client::new();
    let resp = post_compact_json(
        &client,
        &proxy,
        r#"{
            "model":"gpt-5.5",
            "input":[{"type":"message","role":"user","content":"compact me"}],
            "instructions":"compact-test",
            "tools":[{"type":"function","name":"shell"}],
            "parallel_tool_calls":true,
            "reasoning":{"effort":"high"},
            "text":{"verbosity":"low"},
            "previous_response_id":"resp_123",
            "store":true,
            "stream":true,
            "service_tier":"flex",
            "prompt_cache_key":"cache_123",
            "include":["reasoning.encrypted_content"]
        }"#,
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = upstream_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("upstream compact body");

    assert_eq!(body["model"].as_str(), Some("gpt-5.5"));
    assert_eq!(body["instructions"].as_str(), Some("compact-test"));
    assert!(body.get("tools").is_some());
    assert_eq!(body["parallel_tool_calls"].as_bool(), Some(true));
    assert_eq!(body["reasoning"]["effort"].as_str(), Some("high"));
    assert_eq!(body["text"]["verbosity"].as_str(), Some("low"));
    assert!(body.get("previous_response_id").is_none());
    assert!(body.get("store").is_none());
    assert!(body.get("stream").is_none());
    assert_eq!(body["service_tier"].as_str(), Some("flex"));
    assert_eq!(body["prompt_cache_key"].as_str(), Some("cache_123"));
    assert!(body.get("include").is_none());
}

#[tokio::test]
async fn proxy_request_content_encoding_normalizes_zstd_body_before_forwarding() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let upstream_content_encoding = Arc::new(std::sync::Mutex::new(None::<String>));
    let upstream_body = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));

    let hits = upstream_hits.clone();
    let seen_encoding = upstream_content_encoding.clone();
    let seen_body = upstream_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |headers: axum::http::HeaderMap, body: Bytes| {
            let hits = hits.clone();
            let seen_encoding = seen_encoding.clone();
            let seen_body = seen_body.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                *seen_encoding.lock().expect("lock") = headers
                    .get(axum::http::header::CONTENT_ENCODING)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                *seen_body.lock().expect("lock") = body.to_vec();
                (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_proxy_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = spawn_test_proxy(cfg);

    let body = br#"{"model":"gpt-5","input":"hi"}"#;
    let compressed = zstd::stream::encode_all(Cursor::new(body), 0).expect("zstd encode");
    let resp = reqwest::Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("content-encoding", "zstd")
        .body(compressed)
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 1);
    let upstream_json: serde_json::Value =
        serde_json::from_slice(&upstream_body.lock().expect("lock")).expect("upstream json");
    assert_eq!(
        upstream_json,
        serde_json::json!({ "model": "gpt-5", "input": "hi" })
    );
    assert_eq!(*upstream_content_encoding.lock().expect("lock"), None);
}

#[tokio::test]
async fn proxy_request_content_encoding_passthrough_env_preserves_zstd_body_for_upstream() {
    let _lock = env_lock().await;
    let mut env = ScopedEnv::default();
    unsafe {
        env.set("CODEX_HELPER_REQUEST_BODY_ENCODING", "passthrough");
    }

    let upstream_content_encoding = Arc::new(std::sync::Mutex::new(None::<String>));
    let upstream_body = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));

    let seen_encoding = upstream_content_encoding.clone();
    let seen_body = upstream_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |headers: axum::http::HeaderMap, body: Bytes| {
            let seen_encoding = seen_encoding.clone();
            let seen_body = seen_body.clone();
            async move {
                *seen_encoding.lock().expect("lock") = headers
                    .get(axum::http::header::CONTENT_ENCODING)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                *seen_body.lock().expect("lock") = body.to_vec();
                (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_proxy_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = spawn_test_proxy(cfg);

    let body = br#"{"model":"gpt-5","input":"hi"}"#;
    let compressed = zstd::stream::encode_all(Cursor::new(body), 0).expect("zstd encode");
    let resp = reqwest::Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("content-encoding", "zstd")
        .body(compressed.clone())
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(*upstream_body.lock().expect("lock"), compressed);
    assert_eq!(
        upstream_content_encoding.lock().expect("lock").as_deref(),
        Some("zstd")
    );
}

#[tokio::test]
async fn proxy_request_content_encoding_rejects_corrupt_zstd_body() {
    let upstream_hits = Arc::new(AtomicUsize::new(0));

    let hits = upstream_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);

    let cfg = make_proxy_config(
        vec![upstream.upstream_config()],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = spawn_test_proxy(cfg);

    let resp = reqwest::Client::new()
        .post(proxy.responses_url())
        .header("content-type", "application/json")
        .header("content-encoding", "zstd")
        .body("not a zstd frame")
        .send()
        .await
        .expect("send");

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let text = resp.text().await.expect("text");
    assert!(text.contains("Content-Encoding"));
    assert_eq!(upstream_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn proxy_uses_official_session_id_affinity_for_responses_compact() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let a_responses_hits = Arc::new(AtomicUsize::new(0));
    let a_compact_hits = Arc::new(AtomicUsize::new(0));
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));

    let a_responses_counter = a_responses_hits.clone();
    let a_compact_counter = a_compact_hits.clone();
    let upstream_a = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let a_responses_counter = a_responses_counter.clone();
                async move {
                    a_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "a", "err": "quota" })),
                    )
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let a_compact_counter = a_compact_counter.clone();
                async move {
                    a_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "a", "compact": true })),
                    )
                }
            }),
        );
    let (a_addr, a_handle) = spawn_axum_server(upstream_a);

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "b", "compact": true })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["a".to_string(), "b".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::FallbackSticky;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "a".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{a_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-official")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-official")
        .await
        .expect("route affinity recorded from official session-id");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");
    assert_eq!(
        affinity.session_identity_source,
        Some(SessionIdentitySource::Header)
    );

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-official")
        .body(r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(compact["provider"].as_str(), Some("b"));
    assert_eq!(compact["compact"].as_bool(), Some(true));
    assert_eq!(a_compact_hits.load(Ordering::SeqCst), 0);
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    let recent = state.list_recent_finished(20).await;
    assert!(
        recent.iter().any(|request| {
            request.session_id.as_deref() == Some("sid-official")
                && request.session_identity_source == Some(SessionIdentitySource::Header)
        }),
        "finished requests should preserve official header session source"
    );
    let cards = state.list_session_identity_cards(20).await;
    let card = cards
        .iter()
        .find(|card| card.session_id.as_deref() == Some("sid-official"))
        .expect("session card for official session");
    assert_eq!(
        card.session_identity_source,
        Some(SessionIdentitySource::Header)
    );
    assert_eq!(
        card.route_affinity
            .as_ref()
            .and_then(|affinity| affinity.session_identity_source),
        Some(SessionIdentitySource::Header)
    );
    proxy_handle.abort();
    a_handle.abort();
    b_handle.abort();
}

#[tokio::test]
async fn proxy_pins_responses_compact_to_affinity_under_preferred_group() {
    let a_responses_hits = Arc::new(AtomicUsize::new(0));
    let a_compact_hits = Arc::new(AtomicUsize::new(0));
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));

    let a_responses_counter = a_responses_hits.clone();
    let a_compact_counter = a_compact_hits.clone();
    let upstream_a = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let a_responses_counter = a_responses_counter.clone();
                async move {
                    a_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "a", "err": "quota" })),
                    )
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let a_compact_counter = a_compact_counter.clone();
                async move {
                    a_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "a", "compact": true })),
                    )
                }
            }),
        );
    let (a_addr, a_handle) = spawn_axum_server(upstream_a);

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "b", "compact": true })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["a".to_string(), "b".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::PreferredGroup;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "a".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{a_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-preferred-group")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-preferred-group")
        .await
        .expect("route affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-preferred-group")
        .body(r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(compact["provider"].as_str(), Some("b"));
    assert_eq!(a_compact_hits.load(Ordering::SeqCst), 0);
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    a_handle.abort();
    b_handle.abort();
}

#[tokio::test]
async fn proxy_pins_remote_compaction_v2_responses_to_route_affinity() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let a_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_v2_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));

    let a_responses_counter = a_responses_hits.clone();
    let upstream_a = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let a_responses_counter = a_responses_counter.clone();
            async move {
                a_responses_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "provider": "a", "err": "quota" })),
                )
            }
        }),
    );
    let (a_addr, a_handle) = spawn_axum_server(upstream_a);

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_v2_body_counter = b_compact_v2_body.clone();
    let upstream_b = axum::Router::new().route(
        "/v1/responses",
        post(move |body: axum::body::Bytes| {
            let b_responses_counter = b_responses_counter.clone();
            let b_compact_v2_body_counter = b_compact_v2_body_counter.clone();
            async move {
                b_responses_counter.fetch_add(1, Ordering::SeqCst);
                let body_json =
                    serde_json::from_slice::<serde_json::Value>(&body).expect("json body");
                if body_json
                    .get("input")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|items| {
                        items.iter().any(|item| {
                            item.get("type").and_then(serde_json::Value::as_str)
                                == Some("compaction_trigger")
                        })
                    })
                {
                    *b_compact_v2_body_counter.lock().expect("body lock") = Some(body_json);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "provider": "b",
                            "compact_v2": true
                        })),
                    )
                } else {
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }
        }),
    );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["a".to_string(), "b".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::PreferredGroup;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "a".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{a_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-v2-affinity")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-compact-v2-affinity")
        .await
        .expect("route affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    let compact = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-v2-affinity")
        .body(
            r#"{"model":"gpt-5","input":[{"type":"message","role":"user","content":"compact me"},{"type":"compaction_trigger"}],"stream":true}"#,
        )
        .send()
        .await
        .expect("send v2 compact")
        .error_for_status()
        .expect("v2 compact status")
        .json::<serde_json::Value>()
        .await
        .expect("v2 compact json");

    assert_eq!(compact["provider"].as_str(), Some("b"));
    assert_eq!(compact["compact_v2"].as_bool(), Some(true));
    assert_eq!(a_responses_hits.load(Ordering::SeqCst), 1);
    assert_eq!(b_responses_hits.load(Ordering::SeqCst), 2);
    let forwarded = b_compact_v2_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("forwarded v2 compact body");
    assert_eq!(
        forwarded["prompt_cache_key"].as_str(),
        Some("sid-compact-v2-affinity")
    );

    let request_log =
        std::fs::read_to_string(crate::logging::request_log_path()).expect("read request log");
    let compact_record: serde_json::Value = request_log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|record| {
            record["path"].as_str() == Some("/v1/responses")
                && record["codex_bridge"]["remote_compaction_v2_request"].as_bool() == Some(true)
        })
        .expect("v2 compact request log record");
    assert_eq!(
        compact_record["provider_endpoint_key"].as_str(),
        Some("codex/b/default")
    );
    assert_eq!(
        compact_record["codex_bridge"]["remote_compaction_v1_request"].as_bool(),
        None
    );
    assert!(
        compact_record["codex_bridge"]["patch_mode"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );

    proxy_handle.abort();
    a_handle.abort();
    b_handle.abort();
}

#[tokio::test]
async fn proxy_softens_hard_route_affinity_for_ordinary_responses_when_endpoint_unavailable() {
    let b_hits = Arc::new(AtomicUsize::new(0));
    let c_hits = Arc::new(AtomicUsize::new(0));

    let b_counter = b_hits.clone();
    let upstream_b = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let b_counter = b_counter.clone();
            async move {
                b_counter.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
            }
        }),
    );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_counter = c_hits.clone();
    let upstream_c = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let c_counter = c_counter.clone();
            async move {
                c_counter.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, Json(serde_json::json!({ "provider": "c" })))
            }
        }),
    );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

    let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::Hard;
    let v4 = ProxyConfigV4 {
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-hard-ordinary-soft")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send first ordinary response")
        .error_for_status()
        .expect("first ordinary response status")
        .json::<serde_json::Value>()
        .await
        .expect("first ordinary response json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-hard-ordinary-soft")
        .await
        .expect("route affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    state
        .set_provider_endpoint_runtime_state_override(
            "codex",
            ProviderEndpointKey::new("codex", "b", "default"),
            RuntimeConfigState::BreakerOpen,
            crate::logging::now_ms(),
        )
        .await;

    let second = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-hard-ordinary-soft")
        .body(r#"{"model":"gpt-5","input":"still ordinary"}"#)
        .send()
        .await
        .expect("send second ordinary response")
        .error_for_status()
        .expect("second ordinary response should escape unavailable affinity")
        .json::<serde_json::Value>()
        .await
        .expect("second ordinary response json");
    assert_eq!(second["provider"].as_str(), Some("c"));
    assert_eq!(b_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c_hits.load(Ordering::SeqCst), 1);

    let affinity_after_escape = state
        .get_session_route_affinity("sid-hard-ordinary-soft")
        .await
        .expect("route affinity updated after escape");
    assert_eq!(
        affinity_after_escape.provider_endpoint.provider_id.as_str(),
        "c"
    );

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
}

#[tokio::test]
async fn proxy_restores_route_affinity_after_restart_for_responses_compact() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let a_responses_hits = Arc::new(AtomicUsize::new(0));
    let a_compact_hits = Arc::new(AtomicUsize::new(0));
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));

    let a_responses_counter = a_responses_hits.clone();
    let a_compact_counter = a_compact_hits.clone();
    let upstream_a = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let a_responses_counter = a_responses_counter.clone();
                async move {
                    a_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "a", "err": "quota" })),
                    )
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let a_compact_counter = a_compact_counter.clone();
                async move {
                    a_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "a", "compact": true })),
                    )
                }
            }),
        );
    let (a_addr, a_handle) = spawn_axum_server(upstream_a);

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "b", "compact": true })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["a".to_string(), "b".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::PreferredGroup;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "a".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{a_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let lb_states = Arc::new(std::sync::Mutex::new(HashMap::new()));
    let client = reqwest::Client::new();

    {
        let proxy = ProxyService::new_with_v4_source(
            Client::new(),
            Arc::new(runtime.clone()),
            Some(Arc::new(v4.clone())),
            "codex",
            lb_states.clone(),
        );
        let state = proxy.state.clone();
        let app = crate::proxy::router(proxy);
        let (proxy_addr, proxy_handle) = spawn_axum_server(app);

        let first = client
            .post(format!("http://{proxy_addr}/v1/responses"))
            .header("content-type", "application/json")
            .header("session-id", "sid-restart-affinity")
            .body(r#"{"model":"gpt-5","input":"hi"}"#)
            .send()
            .await
            .expect("send responses")
            .error_for_status()
            .expect("responses status")
            .json::<serde_json::Value>()
            .await
            .expect("responses json");
        assert_eq!(first["provider"].as_str(), Some("b"));

        let affinity = state
            .get_session_route_affinity("sid-restart-affinity")
            .await
            .expect("route affinity recorded");
        assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

        proxy_handle.abort();
    }

    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        lb_states,
    );
    let restarted_state = proxy.state.clone();
    let restored = restarted_state
        .get_session_route_affinity("sid-restart-affinity")
        .await
        .expect("route affinity restored");
    assert_eq!(restored.provider_endpoint.provider_id.as_str(), "b");

    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-restart-affinity")
        .body(r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(compact["provider"].as_str(), Some("b"));
    assert_eq!(a_compact_hits.load(Ordering::SeqCst), 0);
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);

    proxy_handle.abort();
    a_handle.abort();
    b_handle.abort();
}

#[tokio::test]
async fn proxy_does_not_provider_failover_responses_compact_after_affinity_failure_under_hard_policy()
 {
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));
    let c_compact_hits = Arc::new(AtomicUsize::new(0));

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "b", "err": "compact failed" })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_compact_counter = c_compact_hits.clone();
    let upstream_c =
        axum::Router::new()
            .route(
                "/v1/responses",
                post(move || async move {
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "c" })))
                }),
            )
            .route(
                "/v1/responses/compact",
                post(move || {
                    let c_compact_counter = c_compact_counter.clone();
                    async move {
                        c_compact_counter.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({ "provider": "c", "compact": true })),
                        )
                    }
                }),
            );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::Hard;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-failure")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-compact-failure")
        .await
        .expect("route affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-failure")
        .body(r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact");
    assert_eq!(compact.status(), StatusCode::BAD_GATEWAY);
    let body = compact.text().await.expect("compact text");
    assert!(
        body.contains("compact failed"),
        "expected affine provider error body, got: {body}"
    );
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c_compact_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
}

#[tokio::test]
async fn proxy_falls_back_responses_compact_after_affinity_failure_under_fallback_sticky() {
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));
    let c_compact_hits = Arc::new(AtomicUsize::new(0));

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "b", "err": "compact failed" })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_compact_counter = c_compact_hits.clone();
    let upstream_c =
        axum::Router::new()
            .route(
                "/v1/responses",
                post(move || async move {
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "c" })))
                }),
            )
            .route(
                "/v1/responses/compact",
                post(move || {
                    let c_compact_counter = c_compact_counter.clone();
                    async move {
                        c_compact_counter.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({ "provider": "c", "compact": true })),
                        )
                    }
                }),
            );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::FallbackSticky;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-failure")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-compact-failure")
        .await
        .expect("route affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-failure")
        .body(r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(compact["provider"].as_str(), Some("c"));
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c_compact_hits.load(Ordering::SeqCst), 1);

    let affinity_after_compact = state
        .get_session_route_affinity("sid-compact-failure")
        .await
        .expect("route affinity updated");
    assert_eq!(
        affinity_after_compact
            .provider_endpoint
            .provider_id
            .as_str(),
        "c"
    );

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
}

#[tokio::test]
async fn proxy_does_not_infer_continuity_domain_from_same_base_url_for_hard_state_bound_compact() {
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));
    let c_compact_hits = Arc::new(AtomicUsize::new(0));

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let c_compact_counter = c_compact_hits.clone();
    let upstream = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move |headers: axum::http::HeaderMap| {
                let b_compact_counter = b_compact_counter.clone();
                let c_compact_counter = c_compact_counter.clone();
                async move {
                    let provider = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .strip_prefix("Bearer ")
                        .unwrap_or("");
                    if provider == "c-token" {
                        c_compact_counter.fetch_add(1, Ordering::SeqCst);
                        return (
                            StatusCode::OK,
                            Json(serde_json::json!({ "provider": "c", "compact": true })),
                        );
                    }
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "b", "err": "compact failed" })),
                    )
                }
            }),
        );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::Hard;
    let shared_base_url = format!("http://{upstream_addr}/v1");
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(shared_base_url.clone()),
                        inline_auth: UpstreamAuth {
                            auth_token: Some("b-token".to_string()),
                            ..UpstreamAuth::default()
                        },
                        tags: std::collections::BTreeMap::from([(
                            "x-provider".to_string(),
                            "b".to_string(),
                        )]),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(shared_base_url),
                        inline_auth: UpstreamAuth {
                            auth_token: Some("c-token".to_string()),
                            ..UpstreamAuth::default()
                        },
                        tags: std::collections::BTreeMap::from([(
                            "x-provider".to_string(),
                            "c".to_string(),
                        )]),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-same-base-no-domain")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-same-base-no-domain")
        .body(r#"{"model":"gpt-5","input":[{"type":"reasoning","encrypted_content":"ciphertext"},{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact");
    assert_eq!(compact.status(), StatusCode::BAD_GATEWAY);
    let body = compact.text().await.expect("compact text");
    assert!(
        body.contains("compact failed"),
        "expected affine provider error body, got: {body}"
    );
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c_compact_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_allows_state_bound_compact_failover_with_explicit_continuity_domain() {
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));
    let c_compact_hits = Arc::new(AtomicUsize::new(0));

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "b", "err": "compact failed" })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_compact_counter = c_compact_hits.clone();
    let upstream_c =
        axum::Router::new()
            .route(
                "/v1/responses",
                post(move || async move {
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "c" })))
                }),
            )
            .route(
                "/v1/responses/compact",
                post(move || {
                    let c_compact_counter = c_compact_counter.clone();
                    async move {
                        c_compact_counter.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({ "provider": "c", "compact": true })),
                        )
                    }
                }),
            );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::FallbackSticky;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        continuity_domain: Some("relay-cluster-a".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        continuity_domain: Some("relay-cluster-a".to_string()),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-explicit-domain")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-explicit-domain")
        .body(r#"{"model":"gpt-5","input":[{"type":"reasoning","encrypted_content":"ciphertext"},{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(compact["provider"].as_str(), Some("c"));
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c_compact_hits.load(Ordering::SeqCst), 1);

    let affinity_after_compact = state
        .get_session_route_affinity("sid-explicit-domain")
        .await
        .expect("route affinity updated");
    assert_eq!(
        affinity_after_compact
            .provider_endpoint
            .provider_id
            .as_str(),
        "c"
    );

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
}

#[tokio::test]
async fn proxy_falls_back_responses_compact_after_affinity_failure_under_fallback_sticky_when_previous_response_id_body_field_is_present()
 {
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));
    let c_compact_hits = Arc::new(AtomicUsize::new(0));
    let c_compact_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "b", "err": "compact failed" })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_compact_counter = c_compact_hits.clone();
    let c_compact_body_counter = c_compact_body.clone();
    let upstream_c = axum::Router::new().route(
        "/v1/responses/compact",
        post(move |body: axum::body::Bytes| {
            let c_compact_counter = c_compact_counter.clone();
            let c_compact_body_counter = c_compact_body_counter.clone();
            async move {
                c_compact_counter.fetch_add(1, Ordering::SeqCst);
                let body_json = serde_json::from_slice::<serde_json::Value>(&body)
                    .expect("compact body should parse");
                *c_compact_body_counter.lock().expect("body lock") = Some(body_json);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "c", "compact": true })),
                )
            }
        }),
    );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::FallbackSticky;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-previous-response-hint")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-compact-previous-response-hint")
        .await
        .expect("route affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-previous-response-hint")
        .body(r#"{"model":"gpt-5","previous_response_id":"resp-123","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(compact["provider"].as_str(), Some("c"));
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c_compact_hits.load(Ordering::SeqCst), 1);
    let forwarded = c_compact_body
        .lock()
        .expect("body lock")
        .clone()
        .expect("forwarded compact body");
    assert!(forwarded.get("previous_response_id").is_none());
    assert_eq!(
        forwarded["prompt_cache_key"].as_str(),
        Some("sid-compact-previous-response-hint")
    );

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
}

#[tokio::test]
async fn proxy_does_not_fallback_responses_compact_when_hard_state_bound() {
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));
    let c_compact_hits = Arc::new(AtomicUsize::new(0));

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "b", "err": "compact failed" })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_compact_counter = c_compact_hits.clone();
    let upstream_c =
        axum::Router::new()
            .route(
                "/v1/responses",
                post(move || async move {
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "c" })))
                }),
            )
            .route(
                "/v1/responses/compact",
                post(move || {
                    let c_compact_counter = c_compact_counter.clone();
                    async move {
                        c_compact_counter.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({ "provider": "c", "compact": true })),
                        )
                    }
                }),
            );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::Hard;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-state-bound")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-compact-state-bound")
        .await
        .expect("route affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-state-bound")
        .body(r#"{"model":"gpt-5","input":[{"type":"reasoning","encrypted_content":"state"}]}"#)
        .send()
        .await
        .expect("send compact");
    assert_eq!(compact.status(), StatusCode::BAD_GATEWAY);
    let body = compact.text().await.expect("compact text");
    assert!(
        body.contains("compact failed"),
        "expected affine provider error body, got: {body}"
    );
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c_compact_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
}

#[tokio::test]
async fn proxy_allows_state_bound_responses_compact_without_route_affinity_under_fallback_sticky() {
    let _env_guard = env_lock().await;
    let fixture =
        CompactPolicyFixture::new(crate::config::RoutingAffinityPolicyV5::FallbackSticky).await;
    let transport = CompactPolicyTransport::ResponsesCompact;
    let session_id = "sid-missing-state-bound-affinity";

    let compact = fixture.post_compaction(transport, session_id).await;

    assert_eq!(compact.status(), StatusCode::OK);
    let body = compact
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(body["provider"].as_str(), Some("b"));
    assert_eq!(fixture.hits("b", transport), 1);
    assert_eq!(fixture.hits("c", transport), 0);
    fixture.assert_affinity_provider(session_id, "b").await;
}

#[tokio::test]
async fn proxy_allows_remote_compaction_v2_without_route_affinity_under_fallback_sticky() {
    let _env_guard = env_lock().await;
    let fixture =
        CompactPolicyFixture::new(crate::config::RoutingAffinityPolicyV5::FallbackSticky).await;
    let transport = CompactPolicyTransport::RemoteCompactionV2;
    let session_id = "sid-missing-v2-affinity";

    let compact = fixture.post_compaction(transport, session_id).await;

    assert_eq!(compact.status(), StatusCode::OK);
    let body = compact
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(body["provider"].as_str(), Some("b"));
    assert_eq!(fixture.hits("b", transport), 1);
    assert_eq!(fixture.hits("c", transport), 0);

    let compact_record = fixture.request_log_record("/v1/responses", StatusCode::OK);
    assert_eq!(
        compact_record["codex_bridge"]["remote_compaction_v2_request"].as_bool(),
        Some(true)
    );
    fixture.assert_affinity_provider(session_id, "b").await;
}

#[tokio::test]
async fn proxy_rejects_remote_compaction_v2_without_route_affinity_under_hard_policy() {
    let _env_guard = env_lock().await;
    let fixture = CompactPolicyFixture::new(crate::config::RoutingAffinityPolicyV5::Hard).await;
    let transport = CompactPolicyTransport::RemoteCompactionV2;

    let compact = fixture
        .post_compaction(transport, "sid-hard-missing-v2-affinity")
        .await;

    assert_eq!(compact.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = compact.text().await.expect("compact text");
    assert!(
        body.contains("state-bound compact") && body.contains("route affinity"),
        "expected hard-policy continuity error body, got: {body}"
    );
    assert_eq!(fixture.hits("b", transport), 0);
    assert_eq!(fixture.hits("c", transport), 0);

    let block = fixture.latest_route_continuity_block_trace();
    assert_eq!(
        block["continuity_class"].as_str(),
        Some("provider_state_bound")
    );
    assert_eq!(
        block["reason"].as_str(),
        Some("state_bound_compact_missing_affinity")
    );
    assert_eq!(block["affinity_source"].as_str(), Some("none"));
    assert_eq!(block["provider_failover_allowed"].as_bool(), Some(false));
}

#[tokio::test]
async fn proxy_allows_remote_compaction_v2_without_prior_affinity_when_route_has_one_endpoint() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let responses_hits = Arc::new(AtomicUsize::new(0));
    let responses_counter = responses_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let responses_counter = responses_counter.clone();
            async move {
                responses_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "solo", "compact_v2": true })),
                )
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["solo".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::FallbackSticky;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([(
                "solo".to_string(),
                ProviderConfigV4 {
                    base_url: Some(format!("http://{upstream_addr}/v1")),
                    inline_auth: UpstreamAuth::default(),
                    ..ProviderConfigV4::default()
                },
            )]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let compact = reqwest::Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-single-v2-affinity")
        .body(
            r#"{"model":"gpt-5","input":[{"type":"message","role":"user","content":"compact me"},{"type":"compaction_trigger"}],"stream":true}"#,
        )
        .send()
        .await
        .expect("send v2 compact")
        .error_for_status()
        .expect("v2 compact status")
        .json::<serde_json::Value>()
        .await
        .expect("v2 compact json");

    assert_eq!(compact["provider"].as_str(), Some("solo"));
    assert_eq!(compact["compact_v2"].as_bool(), Some(true));
    assert_eq!(responses_hits.load(Ordering::SeqCst), 1);
    let affinity = state
        .get_session_route_affinity("sid-single-v2-affinity")
        .await
        .expect("single endpoint affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "solo");

    proxy_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn proxy_does_not_fallback_responses_compact_on_transport_error_when_hard_state_bound() {
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let c_compact_hits = Arc::new(AtomicUsize::new(0));

    let b_responses_counter = b_responses_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || async move {
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "b", "compact": true })),
                )
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_compact_counter = c_compact_hits.clone();
    let upstream_c =
        axum::Router::new()
            .route(
                "/v1/responses",
                post(move || async move {
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "c" })))
                }),
            )
            .route(
                "/v1/responses/compact",
                post(move || {
                    let c_compact_counter = c_compact_counter.clone();
                    async move {
                        c_compact_counter.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({ "provider": "c", "compact": true })),
                        )
                    }
                }),
            );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::Hard;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-transport-state-bound")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("sid-compact-transport-state-bound")
        .await
        .expect("route affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");

    b_handle.abort();

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-transport-state-bound")
        .body(r#"{"model":"gpt-5","input":[{"type":"reasoning","encrypted_content":"state"}]}"#)
        .send()
        .await
        .expect("send compact");
    assert_eq!(compact.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(c_compact_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    c_handle.abort();
}

#[tokio::test]
async fn proxy_rejects_legacy_state_bound_responses_compact_without_route_affinity() {
    let upstream1_hits = Arc::new(AtomicUsize::new(0));
    let upstream2_hits = Arc::new(AtomicUsize::new(0));

    let upstream1_counter = upstream1_hits.clone();
    let upstream1 = axum::Router::new().route(
        "/v1/responses/compact",
        post(move || {
            let upstream1_counter = upstream1_counter.clone();
            async move {
                upstream1_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "provider": "u1", "err": "compact failed" })),
                )
            }
        }),
    );
    let upstream1 = spawn_test_upstream(upstream1);

    let upstream2_counter = upstream2_hits.clone();
    let upstream2 = axum::Router::new().route(
        "/v1/responses/compact",
        post(move || {
            let upstream2_counter = upstream2_counter.clone();
            async move {
                upstream2_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "u2", "compact": true })),
                )
            }
        }),
    );
    let upstream2 = spawn_test_upstream(upstream2);

    let cfg = make_proxy_config(
        vec![upstream1.upstream_config(), upstream2.upstream_config()],
        retry_config(2, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = proxy_service(cfg);
    let proxy = spawn_proxy_service(proxy);

    let client = reqwest::Client::new();
    let compact = post_compact_json(
        &client,
        &proxy,
        r#"{"model":"gpt-5","input":[{"type":"reasoning","encrypted_content":"state"}]}"#,
    )
    .await;

    assert_eq!(compact.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = compact.text().await.expect("compact text");
    assert!(
        body.contains("state-bound compact") && body.contains("route affinity"),
        "expected legacy continuity error body, got: {body}"
    );
    assert_eq!(upstream1_hits.load(Ordering::SeqCst), 0);
    assert_eq!(upstream2_hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn proxy_allows_legacy_state_bound_compact_without_prior_affinity_for_single_endpoint() {
    let _env_guard = env_lock().await;
    let temp_dir = make_temp_test_dir();
    let mut scoped = ScopedEnv::default();
    unsafe {
        scoped.set_path("CODEX_HELPER_HOME", temp_dir.as_path());
    }

    let compact_hits = Arc::new(AtomicUsize::new(0));
    let compact_counter = compact_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses/compact",
        post(move || {
            let compact_counter = compact_counter.clone();
            async move {
                compact_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "solo", "compact": true })),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let mut upstream_config = upstream.upstream_config();
    upstream_config
        .tags
        .insert("provider_id".to_string(), "solo".to_string());

    let cfg = make_proxy_config(
        vec![upstream_config],
        retry_config(2, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);

    let compact = reqwest::Client::new()
        .post(proxy.compact_url())
        .header("content-type", "application/json")
        .header("session-id", "sid-single-legacy-compact")
        .body(r#"{"model":"gpt-5","input":[{"type":"reasoning","encrypted_content":"state"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");

    assert_eq!(compact["provider"].as_str(), Some("solo"));
    assert_eq!(compact["compact"].as_bool(), Some(true));
    assert_eq!(compact_hits.load(Ordering::SeqCst), 1);
    let affinity = state
        .get_session_route_affinity("sid-single-legacy-compact")
        .await
        .expect("single legacy endpoint affinity recorded");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "solo");
}

#[tokio::test]
async fn proxy_legacy_route_affinity_is_session_scoped() {
    let upstream_a_hits = Arc::new(AtomicUsize::new(0));
    let upstream_b_hits = Arc::new(AtomicUsize::new(0));

    let upstream_a_counter = upstream_a_hits.clone();
    let upstream_a = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let upstream_a_counter = upstream_a_counter.clone();
            async move {
                let hit = upstream_a_counter.fetch_add(1, Ordering::SeqCst) + 1;
                if hit == 2 {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "a", "err": "quota" })),
                    )
                } else {
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "a" })))
                }
            }
        }),
    );
    let upstream_a = spawn_test_upstream(upstream_a);

    let upstream_b_counter = upstream_b_hits.clone();
    let upstream_b = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let upstream_b_counter = upstream_b_counter.clone();
            async move {
                upstream_b_counter.fetch_add(1, Ordering::SeqCst);
                (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
            }
        }),
    );
    let upstream_b = spawn_test_upstream(upstream_b);

    let mut upstream_a_config = upstream_a.upstream_config();
    upstream_a_config
        .tags
        .insert("provider_id".to_string(), "a".to_string());
    let mut upstream_b_config = upstream_b.upstream_config();
    upstream_b_config
        .tags
        .insert("provider_id".to_string(), "b".to_string());

    let cfg = make_proxy_config(
        vec![upstream_a_config, upstream_b_config],
        retry_config(1, "502", Vec::new(), RetryStrategy::Failover),
    );
    let proxy = proxy_service(cfg);
    let state = proxy.state.clone();
    let proxy = spawn_proxy_service(proxy);
    let client = reqwest::Client::new();

    let send = |session_id: &'static str, client: reqwest::Client, url: String| async move {
        client
            .post(url)
            .header("content-type", "application/json")
            .header("session-id", session_id)
            .body(r#"{"model":"gpt-5","input":"hi"}"#)
            .send()
            .await
            .expect("send responses request")
            .json::<serde_json::Value>()
            .await
            .expect("json response")
    };

    let first_a = send("sid-a", client.clone(), proxy.responses_url()).await;
    assert_eq!(first_a["provider"].as_str(), Some("a"));

    let fallback_b = send("sid-b", client.clone(), proxy.responses_url()).await;
    assert_eq!(fallback_b["provider"].as_str(), Some("b"));

    let sticky_a = send("sid-a", client.clone(), proxy.responses_url()).await;
    assert_eq!(sticky_a["provider"].as_str(), Some("a"));

    assert_eq!(upstream_a_hits.load(Ordering::SeqCst), 3);
    assert_eq!(upstream_b_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        state
            .get_session_route_affinity("sid-a")
            .await
            .expect("sid-a affinity")
            .provider_endpoint
            .provider_id
            .as_str(),
        "a"
    );
    assert_eq!(
        state
            .get_session_route_affinity("sid-b")
            .await
            .expect("sid-b affinity")
            .provider_endpoint
            .provider_id
            .as_str(),
        "b"
    );
}

#[tokio::test]
async fn proxy_waits_short_affinity_cooldown_before_responses_compact_under_hard_policy() {
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));
    let c_compact_hits = Arc::new(AtomicUsize::new(0));

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "b", "compact": true })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

    let c_compact_counter = c_compact_hits.clone();
    let upstream_c = axum::Router::new().route(
        "/v1/responses/compact",
        post(move || {
            let c_compact_counter = c_compact_counter.clone();
            async move {
                c_compact_counter.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({ "provider": "c", "compact": true })),
                )
            }
        }),
    );
    let (c_addr, c_handle) = spawn_axum_server(upstream_c);

    let retry = RetryConfig {
        transport_cooldown_secs: Some(1),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let mut routing = RoutingConfigV4::ordered_failover(vec!["b".to_string(), "c".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::Hard;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "c".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{c_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = Client::new();
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-cooldown")
        .body(r#"{"model":"gpt-5","input":"hi"}"#)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));
    assert_eq!(b_responses_hits.load(Ordering::SeqCst), 1);

    state
        .penalize_provider_endpoint_attempt(
            "codex",
            crate::runtime_identity::ProviderEndpointKey::new("codex", "b", "default"),
            1,
            crate::lb::CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        )
        .await;

    let started = std::time::Instant::now();
    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .header("session-id", "sid-compact-cooldown")
        .body(r#"{"model":"gpt-5","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");

    assert_eq!(compact["provider"].as_str(), Some("b"));
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "compact should wait out the short affinity cooldown"
    );
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    assert_eq!(c_compact_hits.load(Ordering::SeqCst), 0);

    proxy_handle.abort();
    b_handle.abort();
    c_handle.abort();
}

#[tokio::test]
async fn proxy_uses_prompt_cache_key_affinity_when_session_headers_are_absent() {
    let a_responses_hits = Arc::new(AtomicUsize::new(0));
    let a_compact_hits = Arc::new(AtomicUsize::new(0));
    let b_responses_hits = Arc::new(AtomicUsize::new(0));
    let b_compact_hits = Arc::new(AtomicUsize::new(0));

    let a_responses_counter = a_responses_hits.clone();
    let a_compact_counter = a_compact_hits.clone();
    let upstream_a = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let a_responses_counter = a_responses_counter.clone();
                async move {
                    a_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({ "provider": "a", "err": "quota" })),
                    )
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let a_compact_counter = a_compact_counter.clone();
                async move {
                    a_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "a", "compact": true })),
                    )
                }
            }),
        );
    let (a_addr, a_handle) = spawn_axum_server(upstream_a);

    let b_responses_counter = b_responses_hits.clone();
    let b_compact_counter = b_compact_hits.clone();
    let upstream_b = axum::Router::new()
        .route(
            "/v1/responses",
            post(move || {
                let b_responses_counter = b_responses_counter.clone();
                async move {
                    b_responses_counter.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, Json(serde_json::json!({ "provider": "b" })))
                }
            }),
        )
        .route(
            "/v1/responses/compact",
            post(move || {
                let b_compact_counter = b_compact_counter.clone();
                async move {
                    b_compact_counter.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({ "provider": "b", "compact": true })),
                    )
                }
            }),
        );
    let (b_addr, b_handle) = spawn_axum_server(upstream_b);

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
    let mut routing = RoutingConfigV4::ordered_failover(vec!["a".to_string(), "b".to_string()]);
    routing.affinity_policy = crate::config::RoutingAffinityPolicyV5::FallbackSticky;
    let v4 = ProxyConfigV4 {
        retry,
        codex: ServiceViewV4 {
            providers: std::collections::BTreeMap::from([
                (
                    "a".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{a_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
                (
                    "b".to_string(),
                    ProviderConfigV4 {
                        base_url: Some(format!("http://{b_addr}/v1")),
                        inline_auth: UpstreamAuth::default(),
                        ..ProviderConfigV4::default()
                    },
                ),
            ]),
            routing: Some(routing),
            ..ServiceViewV4::default()
        },
        ..ProxyConfigV4::default()
    };
    let runtime = crate::config::compile_v4_to_runtime(&v4).expect("compile v4");
    let proxy = ProxyService::new_with_v4_source(
        Client::new(),
        Arc::new(runtime),
        Some(Arc::new(v4)),
        "codex",
        Arc::new(std::sync::Mutex::new(HashMap::new())),
    );
    let state = proxy.state.clone();
    let app = crate::proxy::router(proxy);
    let (proxy_addr, proxy_handle) = spawn_axum_server(app);

    let client = reqwest::Client::new();
    let first_body = br#"{"model":"gpt-5","prompt_cache_key":"pcache-affinity","input":"hi"}"#;
    let first_compressed =
        zstd::stream::encode_all(Cursor::new(first_body), 0).expect("zstd encode");
    let first = client
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("content-encoding", "zstd")
        .body(first_compressed)
        .send()
        .await
        .expect("send responses")
        .error_for_status()
        .expect("responses status")
        .json::<serde_json::Value>()
        .await
        .expect("responses json");
    assert_eq!(first["provider"].as_str(), Some("b"));

    let affinity = state
        .get_session_route_affinity("pcache-affinity")
        .await
        .expect("route affinity recorded from prompt_cache_key");
    assert_eq!(affinity.provider_endpoint.provider_id.as_str(), "b");
    assert_eq!(
        affinity.session_identity_source,
        Some(SessionIdentitySource::PromptCacheKey)
    );

    let compact = client
        .post(format!("http://{proxy_addr}/responses/compact"))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","prompt_cache_key":"pcache-affinity","input":[{"role":"user","content":"compact me"}]}"#)
        .send()
        .await
        .expect("send compact")
        .error_for_status()
        .expect("compact status")
        .json::<serde_json::Value>()
        .await
        .expect("compact json");
    assert_eq!(compact["provider"].as_str(), Some("b"));
    assert_eq!(compact["compact"].as_bool(), Some(true));
    assert_eq!(a_compact_hits.load(Ordering::SeqCst), 0);
    assert_eq!(b_compact_hits.load(Ordering::SeqCst), 1);
    let affinity_after_compact = state
        .get_session_route_affinity("pcache-affinity")
        .await
        .expect("route affinity still keyed by prompt_cache_key after compact");
    assert_eq!(
        affinity_after_compact
            .provider_endpoint
            .provider_id
            .as_str(),
        "b"
    );
    assert_eq!(
        affinity_after_compact.session_identity_source,
        Some(SessionIdentitySource::PromptCacheKey)
    );
    let recent = state.list_recent_finished(20).await;
    assert!(
        recent.iter().any(|request| {
            request.session_id.as_deref() == Some("pcache-affinity")
                && request.session_identity_source == Some(SessionIdentitySource::PromptCacheKey)
        }),
        "finished requests should preserve prompt_cache_key session source"
    );
    let cards = state.list_session_identity_cards(20).await;
    let card = cards
        .iter()
        .find(|card| card.session_id.as_deref() == Some("pcache-affinity"))
        .expect("session card for prompt_cache_key fallback");
    assert_eq!(
        card.session_identity_source,
        Some(SessionIdentitySource::PromptCacheKey)
    );
    assert_eq!(
        card.route_affinity
            .as_ref()
            .and_then(|affinity| affinity.session_identity_source),
        Some(SessionIdentitySource::PromptCacheKey)
    );

    proxy_handle.abort();
    a_handle.abort();
    b_handle.abort();
}
