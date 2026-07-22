use super::*;

const HTTP_DEBUG_ALL_CHILD: &str = "proxy::tests::http_debug::http_debug_all_transport_child";
const HTTP_DEBUG_FAILURE_CHILD: &str =
    "proxy::tests::http_debug::http_debug_sse_logical_failure_child";
const HTTP_DEBUG_RETRYABLE_FAILURE_CHILD: &str =
    "proxy::tests::http_debug::http_debug_retryable_buffered_failure_child";
const HTTP_DEBUG_FAILOVER_FAILURE_CHILD: &str =
    "proxy::tests::http_debug::http_debug_provider_failover_failure_child";
const HTTP_DEBUG_CHILD_ENV: &str = "CODEX_HELPER_HTTP_DEBUG_TEST_CHILD";

fn run_http_debug_child(test_name: &str, debug_all: bool) {
    let home = make_temp_test_dir();
    let output = std::process::Command::new(
        std::env::current_exe().expect("locate current core test executable"),
    )
    .args(["--exact", test_name, "--ignored", "--nocapture"])
    .env(HTTP_DEBUG_CHILD_ENV, "1")
    .env("CODEX_HELPER_HOME", &home)
    .env("CODEX_HELPER_HTTP_DEBUG", "1")
    .env(
        "CODEX_HELPER_HTTP_DEBUG_ALL",
        if debug_all { "1" } else { "0" },
    )
    .env("CODEX_HELPER_HTTP_LOG_REQUEST_BODY", "1")
    .env("CODEX_HELPER_HTTP_DEBUG_BODY_MAX", "65536")
    .env("CODEX_HELPER_HTTP_DEBUG_SPLIT", "1")
    .env("CODEX_HELPER_HTTP_WARN", "0")
    .output()
    .expect("run isolated HTTP debug test");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "HTTP debug child failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(home);
}

fn debug_records() -> Vec<serde_json::Value> {
    let path = crate::config::proxy_home_dir()
        .join("logs")
        .join("requests_debug.jsonl");
    std::fs::read_to_string(path)
        .expect("read split HTTP debug log")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse HTTP debug record"))
        .collect()
}

fn request_records() -> Vec<serde_json::Value> {
    std::fs::read_to_string(crate::logging::request_log_path())
        .expect("read request log")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse request log record"))
        .collect()
}

#[test]
fn buffered_and_transport_http_debug_are_complete_and_redacted() {
    run_http_debug_child(HTTP_DEBUG_ALL_CHILD, true);
}

#[test]
fn sse_logical_failure_is_recorded_in_failures_only_mode() {
    run_http_debug_child(HTTP_DEBUG_FAILURE_CHILD, false);
}

#[test]
fn retryable_buffered_failure_records_every_dispatched_attempt() {
    run_http_debug_child(HTTP_DEBUG_RETRYABLE_FAILURE_CHILD, false);
}

#[test]
fn provider_failover_records_every_dispatched_attempt() {
    run_http_debug_child(HTTP_DEBUG_FAILOVER_FAILURE_CHILD, false);
}

#[tokio::test]
#[ignore = "launched by buffered_and_transport_http_debug_are_complete_and_redacted"]
async fn http_debug_all_transport_child() {
    if std::env::var_os(HTTP_DEBUG_CHILD_ENV).is_none() {
        return;
    }

    const REQUEST_BODY_SENTINEL: &str = "client-body-visible-73f9";
    const RESPONSE_BODY_SENTINEL: &str = "upstream-body-visible-84ac";
    const QUERY_SECRET: &str = "query-secret-98bd";
    const API_KEY_SECRET: &str = "api-key-secret-a33f";
    const AUTH_TOKEN_SECRET: &str = "auth-token-secret-c611";
    const ATTESTATION_SECRET: &str = "attestation-secret-d78a";
    const RESPONSE_HEADER_SECRET: &str = "response-header-secret-e17d";
    const HELPER_BEARER_TOKEN: &str = "helper-bearer-secret-746b";
    const SESSION_ID: &str = "debug-session-415b";

    let captured_upstream_body = Arc::new(std::sync::Mutex::new(None::<Vec<u8>>));
    let upstream_body_for_handler = Arc::clone(&captured_upstream_body);
    let upstream = axum::Router::new().route(
        "/gateway/v1/responses",
        post(move |body: Bytes| {
            let upstream_body_for_handler = Arc::clone(&upstream_body_for_handler);
            async move {
                *upstream_body_for_handler
                    .lock()
                    .expect("capture upstream request body") = Some(body.to_vec());
                let mut response = Response::new(Body::from(format!(
                    r#"{{"id":"resp-debug","output":"{RESPONSE_BODY_SENTINEL}","echo_authorization":"Bearer {HELPER_BEARER_TOKEN}","echo_token":"{HELPER_BEARER_TOKEN}"}}"#
                )));
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response.headers_mut().insert(
                    "x-backend-secret",
                    HeaderValue::from_static(RESPONSE_HEADER_SECRET),
                );
                response
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let config = make_helper_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/gateway"),
            auth: UpstreamAuth {
                auth_token: Some(HELPER_BEARER_TOKEN.to_string().into()),
                ..UpstreamAuth::default()
            },
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    let proxy = ProxyService::new(Client::new(), Arc::new(config), "codex");
    let (proxy_addr, proxy_handle) = spawn_axum_server(crate::proxy::router(proxy));

    let client_request_body = format!(r#"{{"model":"gpt-5","input":"{REQUEST_BODY_SENTINEL}"}}"#);
    let response = Client::new()
        .post(format!(
            "http://{proxy_addr}/v1/responses?token={QUERY_SECRET}"
        ))
        .header("content-type", "application/json")
        .header("api-key", API_KEY_SECRET)
        .header("x-auth-token", AUTH_TOKEN_SECRET)
        .header("x-oai-attestation", ATTESTATION_SECRET)
        .header("session-id", SESSION_ID)
        .body(client_request_body.clone())
        .send()
        .await
        .expect("send buffered debug request");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .text()
            .await
            .expect("read buffered response")
            .contains(RESPONSE_BODY_SENTINEL)
    );
    let actual_upstream_body = captured_upstream_body
        .lock()
        .expect("read captured upstream request body")
        .clone()
        .expect("upstream request body");
    let actual_upstream_body =
        String::from_utf8(actual_upstream_body).expect("upstream request body is JSON");

    proxy_handle.abort();
    upstream_handle.abort();

    let unused_addr = reserve_unused_local_addr();
    let config = make_helper_config(
        vec![UpstreamConfig {
            base_url: format!("http://{unused_addr}/v1"),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    let proxy = ProxyService::new(Client::new(), Arc::new(config), "codex");
    let (proxy_addr, proxy_handle) = spawn_axum_server(crate::proxy::router(proxy));
    let transport_response = Client::new()
        .post(format!(
            "http://{proxy_addr}/v1/responses?token={QUERY_SECRET}"
        ))
        .header("content-type", "application/json")
        .header("api-key", API_KEY_SECRET)
        .body(format!(
            r#"{{"model":"gpt-5","input":"{REQUEST_BODY_SENTINEL}"}}"#
        ))
        .send()
        .await
        .expect("send transport failure request");
    assert_eq!(transport_response.status(), StatusCode::BAD_GATEWAY);
    let _ = transport_response
        .bytes()
        .await
        .expect("read transport failure response");
    proxy_handle.abort();

    let records = debug_records();
    assert!(records.len() >= 2, "records={records:#?}");
    let serialized = records
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    for secret in [
        QUERY_SECRET,
        API_KEY_SECRET,
        AUTH_TOKEN_SECRET,
        ATTESTATION_SECRET,
        RESPONSE_HEADER_SECRET,
        HELPER_BEARER_TOKEN,
    ] {
        assert!(!serialized.contains(secret), "leaked secret {secret}");
    }
    assert!(serialized.contains("[REDACTED]"));
    assert!(serialized.contains(REQUEST_BODY_SENTINEL));
    assert!(serialized.contains(RESPONSE_BODY_SENTINEL));
    assert!(serialized.contains(r#""client_uri":"/v1/responses""#));
    assert!(serialized.contains("upstream_transport_error"));

    let successful = records
        .iter()
        .find(|record| record["status_code"].as_u64() == Some(200))
        .expect("successful HTTP debug record");
    let logged_client_body = successful["http_debug"]["client_body"]["data"]
        .as_str()
        .expect("logged client body");
    let logged_upstream_body = successful["http_debug"]["upstream_request_body"]["data"]
        .as_str()
        .expect("logged upstream body");
    assert_eq!(logged_client_body, client_request_body);
    assert_eq!(logged_upstream_body, actual_upstream_body);
    assert_eq!(
        successful["http_debug"]["upstream_uri"].as_str(),
        Some("/gateway/v1/responses")
    );
    assert_eq!(
        successful["http_debug"]["request_body_len"].as_u64(),
        Some(client_request_body.len() as u64)
    );
    let client_json: serde_json::Value =
        serde_json::from_str(logged_client_body).expect("parse logged client body");
    let upstream_json: serde_json::Value =
        serde_json::from_str(logged_upstream_body).expect("parse logged upstream body");
    assert!(client_json.get("prompt_cache_key").is_none());
    assert_eq!(upstream_json["prompt_cache_key"].as_str(), Some(SESSION_ID));
}

#[tokio::test]
#[ignore = "launched by sse_logical_failure_is_recorded_in_failures_only_mode"]
async fn http_debug_sse_logical_failure_child() {
    if std::env::var_os(HTTP_DEBUG_CHILD_ENV).is_none() {
        return;
    }

    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            let padding = "x".repeat(70 * 1024);
            let mut response = Response::new(Body::from(format!(
                "event: response.output_text.delta\n\
data: {{\"type\":\"response.output_text.delta\",\"delta\":\"{padding}\"}}\n\n\
event: response.failed\n\
data: {{\"type\":\"response.failed\",\"response\":{{\"id\":\"resp-debug-failed\",\"error\":{{\"message\":\"logical-failure-visible-2b0e\"}}}}}}\n\n"
            )));
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            response
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let config = make_helper_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        RetryConfig::default(),
    );
    let proxy = ProxyService::new(Client::new(), Arc::new(config), "codex");
    let (proxy_addr, proxy_handle) = spawn_axum_server(crate::proxy::router(proxy));

    let response = Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .body(r#"{"model":"gpt-5","input":"debug SSE","stream":true}"#)
        .send()
        .await
        .expect("send SSE logical failure request");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.expect("drain SSE failure response");
    assert!(body.contains("response.failed"), "body={body}");

    proxy_handle.abort();
    upstream_handle.abort();

    let records = debug_records();
    let record = records
        .iter()
        .find(|record| record["status_code"].as_u64() == Some(502))
        .expect("logical SSE failure debug record");
    assert_eq!(record["http_debug"]["client_uri"], "/v1/responses");
    assert!(
        record["http_debug"]["upstream_response_body"]["data"]
            .as_str()
            .is_some_and(|body| body.contains("logical-failure-visible-2b0e")),
        "record={record:#?}"
    );
    assert_eq!(
        record["http_debug"]["upstream_response_body"]["window"].as_str(),
        Some("tail")
    );
}

#[tokio::test]
#[ignore = "launched by retryable_buffered_failure_records_every_dispatched_attempt"]
async fn http_debug_retryable_buffered_failure_child() {
    if std::env::var_os(HTTP_DEBUG_CHILD_ENV).is_none() {
        return;
    }

    const FIRST_RESPONSE_BODY_SENTINEL: &str = "retryable-first-body-visible-c92e";
    const SECOND_RESPONSE_BODY_SENTINEL: &str = "retryable-second-body-visible-d13f";

    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let hits = upstream_hits.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hits = hits.clone();
            async move {
                let attempt = hits.fetch_add(1, Ordering::SeqCst);
                let sentinel = if attempt == 0 {
                    FIRST_RESPONSE_BODY_SENTINEL
                } else {
                    SECOND_RESPONSE_BODY_SENTINEL
                };
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": {
                            "message": sentinel,
                        },
                    })),
                )
            }
        }),
    );
    let (upstream_addr, upstream_handle) = spawn_axum_server(upstream);
    let retry = RetryConfig {
        upstream: Some(crate::config::RetryLayerConfig {
            max_attempts: Some(2),
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
        ..RetryConfig::default()
    };
    let config = make_helper_config(
        vec![UpstreamConfig {
            base_url: format!("http://{upstream_addr}/v1"),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }],
        retry,
    );
    let proxy = ProxyService::new(Client::new(), Arc::new(config), "codex");
    let (proxy_addr, proxy_handle) = spawn_axum_server(crate::proxy::router(proxy));

    let response = Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","input":"retryable debug failure"}"#)
        .send()
        .await
        .expect("send retryable buffered failure request");
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let _ = response
        .bytes()
        .await
        .expect("read retryable buffered failure response");

    proxy_handle.abort();
    upstream_handle.abort();

    assert_eq!(upstream_hits.load(Ordering::SeqCst), 2);

    let request_record = request_records()
        .into_iter()
        .find(|record| record["status_code"].as_u64() == Some(502))
        .expect("final failed request record");
    let debug_ref = request_record["http_debug_ref"]
        .as_object()
        .expect("split HTTP debug reference");
    assert_eq!(
        debug_ref.get("file").and_then(serde_json::Value::as_str),
        Some("requests_debug.jsonl")
    );
    let debug_id = debug_ref
        .get("id")
        .and_then(serde_json::Value::as_str)
        .expect("HTTP debug reference id");

    let debug_records = debug_records();
    let debug_record = debug_records
        .iter()
        .find(|record| record["id"].as_str() == Some(debug_id))
        .expect("referenced split HTTP debug record");
    assert_eq!(debug_record["status_code"].as_u64(), Some(502));
    assert!(
        debug_record["http_debug"]["upstream_response_body"]["data"]
            .as_str()
            .is_some_and(|body| body.contains(SECOND_RESPONSE_BODY_SENTINEL)),
        "record={debug_record:#?}"
    );

    let attempt_refs = request_record["http_debug_attempt_refs"]
        .as_array()
        .expect("per-attempt HTTP debug references");
    assert_eq!(attempt_refs.len(), 2, "refs={attempt_refs:#?}");
    for (attempt_index, sentinel) in [
        (0_u64, FIRST_RESPONSE_BODY_SENTINEL),
        (1_u64, SECOND_RESPONSE_BODY_SENTINEL),
    ] {
        let attempt_ref = attempt_refs
            .iter()
            .find(|reference| reference["route_attempt_index"].as_u64() == Some(attempt_index))
            .expect("stable per-attempt HTTP debug reference");
        assert_eq!(attempt_ref["file"].as_str(), Some("requests_debug.jsonl"));
        let attempt_debug_id = attempt_ref["id"]
            .as_str()
            .expect("per-attempt HTTP debug id");
        let attempt_record = debug_records
            .iter()
            .find(|record| record["id"].as_str() == Some(attempt_debug_id))
            .expect("per-attempt split HTTP debug record");
        assert_eq!(
            attempt_record["route_attempt_index"].as_u64(),
            Some(attempt_index)
        );
        assert!(
            attempt_record["http_debug"]["upstream_response_body"]["data"]
                .as_str()
                .is_some_and(|body| body.contains(sentinel)),
            "record={attempt_record:#?}"
        );
    }
}

#[tokio::test]
#[ignore = "launched by provider_failover_records_every_dispatched_attempt"]
async fn http_debug_provider_failover_failure_child() {
    if std::env::var_os(HTTP_DEBUG_CHILD_ENV).is_none() {
        return;
    }

    const FIRST_RESPONSE_BODY_SENTINEL: &str = "failover-first-body-visible-873a";
    const SECOND_RESPONSE_BODY_SENTINEL: &str = "failover-second-body-visible-1f4c";

    let first_hits = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&first_hits);
    let first = axum::Router::new().route(
        "/primary-root/v1/responses",
        post(move || {
            let hits = Arc::clone(&hits);
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": { "message": FIRST_RESPONSE_BODY_SENTINEL },
                    })),
                )
            }
        }),
    );
    let (first_addr, first_handle) = spawn_axum_server(first);

    let second_hits = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&second_hits);
    let second = axum::Router::new().route(
        "/secondary-root/v1/responses",
        post(move || {
            let hits = Arc::clone(&hits);
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": { "message": SECOND_RESPONSE_BODY_SENTINEL },
                    })),
                )
            }
        }),
    );
    let (second_addr, second_handle) = spawn_axum_server(second);

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
        transport_cooldown_secs: Some(0),
        cooldown_backoff_factor: Some(1),
        cooldown_backoff_max_secs: Some(0),
        ..RetryConfig::default()
    };
    let config = make_helper_config(
        vec![
            UpstreamConfig {
                base_url: format!("http://{first_addr}/primary-root"),
                auth: UpstreamAuth::default(),
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
            UpstreamConfig {
                base_url: format!("http://{second_addr}/secondary-root"),
                auth: UpstreamAuth::default(),
                tags: HashMap::new(),
                supported_models: HashMap::new(),
                model_mapping: HashMap::new(),
            },
        ],
        retry,
    );
    let proxy = ProxyService::new(Client::new(), Arc::new(config), "codex");
    let (proxy_addr, proxy_handle) = spawn_axum_server(crate::proxy::router(proxy));

    let response = Client::new()
        .post(format!("http://{proxy_addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"model":"gpt-5","input":"provider failover debug"}"#)
        .send()
        .await
        .expect("send provider failover debug request");
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let _ = response
        .bytes()
        .await
        .expect("read provider failover response");

    proxy_handle.abort();
    first_handle.abort();
    second_handle.abort();
    assert_eq!(first_hits.load(Ordering::SeqCst), 1);
    assert_eq!(second_hits.load(Ordering::SeqCst), 1);

    let request_record = request_records()
        .into_iter()
        .find(|record| record["status_code"].as_u64() == Some(502))
        .expect("final failed request record");
    let attempt_refs = request_record["http_debug_attempt_refs"]
        .as_array()
        .expect("per-attempt HTTP debug references");
    assert_eq!(attempt_refs.len(), 2, "refs={attempt_refs:#?}");

    let records = debug_records();
    for (attempt_index, sentinel, upstream_uri) in [
        (
            0_u64,
            FIRST_RESPONSE_BODY_SENTINEL,
            "/primary-root/v1/responses",
        ),
        (
            1_u64,
            SECOND_RESPONSE_BODY_SENTINEL,
            "/secondary-root/v1/responses",
        ),
    ] {
        let reference = attempt_refs
            .iter()
            .find(|reference| reference["route_attempt_index"].as_u64() == Some(attempt_index))
            .expect("stable provider failover attempt reference");
        let debug_id = reference["id"].as_str().expect("attempt debug id");
        let record = records
            .iter()
            .find(|record| record["id"].as_str() == Some(debug_id))
            .expect("referenced provider failover debug record");
        assert_eq!(record["route_attempt_index"].as_u64(), Some(attempt_index));
        assert_eq!(record["http_debug"]["upstream_uri"], upstream_uri);
        assert!(
            record["http_debug"]["upstream_response_body"]["data"]
                .as_str()
                .is_some_and(|body| body.contains(sentinel)),
            "record={record:#?}"
        );
    }
}
