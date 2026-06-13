use super::*;
use crate::proxy::tests::harness::{
    post_images_edits_json, post_images_generations_json, spawn_test_proxy, spawn_test_upstream,
    upstream_config,
};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn openai_images_generation_endpoint_translates_request_and_response() {
    let seen_body = Arc::new(Mutex::new(None::<serde_json::Value>));
    let seen_body_for_route = seen_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen_body_for_route = seen_body_for_route.clone();
            async move {
                *seen_body_for_route.lock().expect("lock") = Some(body);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "id": "resp_image",
                        "created": 42,
                        "output": [
                            {
                                "type": "image_generation_call",
                                "id": "ig_1",
                                "status": "completed",
                                "result": "Zm9v",
                                "revised_prompt": "A neon rain cat"
                            }
                        ]
                    })),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], RetryConfig::default());
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response = post_images_generations_json(
        &client,
        &proxy,
        r#"{"model":"gpt-image-2","prompt":"一只猫在雨夜的霓虹灯下","size":"3840x2160","output_format":"png","quality":"high"}"#,
    )
    .await
    .error_for_status()
    .expect("images status")
    .json::<serde_json::Value>()
    .await
    .expect("images json");

    assert_eq!(response["created"], 42);
    assert_eq!(response["data"][0]["b64_json"], "Zm9v");
    assert_eq!(response["data"][0]["revised_prompt"], "A neon rain cat");

    let seen = seen_body
        .lock()
        .expect("lock")
        .clone()
        .expect("upstream body");
    assert_eq!(seen["model"], "gpt-5.5");
    assert_eq!(seen["input"], "一只猫在雨夜的霓虹灯下");
    assert_eq!(seen["tools"][0]["type"], "image_generation");
    assert_eq!(seen["tools"][0]["size"], "3840x2160");
    assert_eq!(seen["tools"][0]["output_format"], "png");
    assert_eq!(seen["tools"][0]["quality"], "high");
    assert_eq!(seen["tool_choice"]["type"], "image_generation");
}

#[tokio::test]
async fn openai_images_generation_endpoint_honors_explicit_responses_model() {
    let seen_body = Arc::new(Mutex::new(None::<serde_json::Value>));
    let seen_body_for_route = seen_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen_body_for_route = seen_body_for_route.clone();
            async move {
                *seen_body_for_route.lock().expect("lock") = Some(body);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "id": "resp_image",
                        "created": 42,
                        "output": [
                            {
                                "type": "image_generation_call",
                                "id": "ig_1",
                                "status": "completed",
                                "result": "Zm9v"
                            }
                        ]
                    })),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], RetryConfig::default());
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response = post_images_generations_json(
        &client,
        &proxy,
        r#"{"model":"gpt-image-2","responses_model":"gpt-5.4","prompt":"cat"}"#,
    )
    .await
    .error_for_status()
    .expect("images status")
    .json::<serde_json::Value>()
    .await
    .expect("images json");

    assert_eq!(response["data"][0]["b64_json"], "Zm9v");
    let seen = seen_body
        .lock()
        .expect("lock")
        .clone()
        .expect("upstream body");
    assert_eq!(seen["model"], "gpt-5.4");
    assert_eq!(seen["tools"][0]["type"], "image_generation");
    assert_eq!(seen["tool_choice"]["type"], "image_generation");
}

#[tokio::test]
async fn openai_images_generation_endpoint_strips_client_user_agent_before_upstream() {
    let seen_user_agent = Arc::new(Mutex::new(None::<Option<String>>));
    let seen_user_agent_for_route = seen_user_agent.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |headers: HeaderMap| {
            let seen_user_agent_for_route = seen_user_agent_for_route.clone();
            async move {
                let seen = headers
                    .get(axum::http::header::USER_AGENT)
                    .and_then(|value| value.to_str().ok())
                    .map(|value| value.to_string());
                *seen_user_agent_for_route.lock().expect("lock") = Some(seen);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "id": "resp_image",
                        "created": 42,
                        "output": [
                            {
                                "type": "image_generation_call",
                                "id": "ig_1",
                                "status": "completed",
                                "result": "Zm9v",
                                "revised_prompt": "A neon rain cat"
                            }
                        ]
                    })),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], RetryConfig::default());
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response = client
        .post(proxy.images_generations_url())
        .header("content-type", "application/json")
        .header(axum::http::header::USER_AGENT, "Python-urllib/3.13")
        .body(r#"{"model":"gpt-image-2","prompt":"cat"}"#)
        .send()
        .await
        .expect("send images request");

    let response = response.error_for_status().expect("images status");
    response
        .json::<serde_json::Value>()
        .await
        .expect("images json");

    let seen = seen_user_agent
        .lock()
        .expect("lock")
        .clone()
        .expect("upstream user-agent");
    assert_ne!(seen.as_deref(), Some("Python-urllib/3.13"));
}

#[tokio::test]
async fn openai_images_generation_endpoint_rejects_n_greater_than_one_before_upstream() {
    let hit_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hit_count_for_route = hit_count.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hit_count_for_route = hit_count_for_route.clone();
            async move {
                hit_count_for_route.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({"id": "resp_unused", "output": []})),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], RetryConfig::default());
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response = post_images_generations_json(
        &client,
        &proxy,
        r#"{"model":"gpt-image-2","prompt":"cat","n":2}"#,
    )
    .await;
    let status = response.status();
    let text = response.text().await.expect("body");

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(text.contains("n=1"));
    assert_eq!(hit_count.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn openai_images_generation_endpoint_passes_upstream_errors_through() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": "image generation is not enabled"
                    }
                })),
            )
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], RetryConfig::default());
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response =
        post_images_generations_json(&client, &proxy, r#"{"model":"gpt-image-2","prompt":"cat"}"#)
            .await;
    let status = response.status();
    let body = response.json::<serde_json::Value>().await.expect("json");

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["message"], "image generation is not enabled");
}

#[tokio::test]
async fn openai_images_generation_endpoint_reports_missing_image_result() {
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": "resp_no_image",
                    "output": [
                        {"type": "message", "content": []}
                    ]
                })),
            )
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], RetryConfig::default());
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response =
        post_images_generations_json(&client, &proxy, r#"{"model":"gpt-image-2","prompt":"cat"}"#)
            .await;
    let status = response.status();
    let text = response.text().await.expect("body");

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(text.contains("image_generation_call"));
}

#[tokio::test]
async fn openai_images_generation_missing_result_fails_over_to_next_upstream() {
    let bad_hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let bad_hits_for_route = bad_hits.clone();
    let bad_upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let bad_hits_for_route = bad_hits_for_route.clone();
            async move {
                bad_hits_for_route.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "id": "resp_no_image",
                        "output": [
                            {"type": "message", "content": []}
                        ]
                    })),
                )
            }
        }),
    );
    let good_hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let good_hits_for_route = good_hits.clone();
    let good_upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let good_hits_for_route = good_hits_for_route.clone();
            async move {
                good_hits_for_route.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "id": "resp_image",
                        "created": 45,
                        "output": [
                            {
                                "type": "image_generation_call",
                                "id": "ig_1",
                                "status": "completed",
                                "result": "YmF6"
                            }
                        ]
                    })),
                )
            }
        }),
    );
    let bad_upstream = spawn_test_upstream(bad_upstream);
    let good_upstream = spawn_test_upstream(good_upstream);
    let cfg = make_proxy_config(
        vec![
            bad_upstream.upstream_config(),
            good_upstream.upstream_config(),
        ],
        RetryConfig::default(),
    );
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response =
        post_images_generations_json(&client, &proxy, r#"{"model":"gpt-image-2","prompt":"cat"}"#)
            .await
            .error_for_status()
            .expect("images status")
            .json::<serde_json::Value>()
            .await
            .expect("images json");

    assert_eq!(response["created"], 45);
    assert_eq!(response["data"][0]["b64_json"], "YmF6");
    assert_eq!(bad_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(good_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn openai_images_generation_route_failure_returns_openai_error_json() {
    let unused_addr = reserve_unused_local_addr();
    let cfg = make_proxy_config(
        vec![upstream_config(format!("http://{unused_addr}/v1"))],
        RetryConfig::default(),
    );
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response =
        post_images_generations_json(&client, &proxy, r#"{"model":"gpt-image-2","prompt":"cat"}"#)
            .await;
    let status = response.status();
    let body = response.json::<serde_json::Value>().await.expect("json");

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(
        body["error"]["type"].as_str(),
        Some("image_generation_route_failed")
    );
    assert_eq!(body["error"]["retryable"].as_bool(), Some(true));
    assert_eq!(
        body["error"]["failure_hint"].as_str(),
        Some("all_upstreams_failed")
    );
    assert!(
        body["error"]["request_id"].as_str().is_some(),
        "request_id should be a structured field: {body}"
    );
    assert!(
        body["error"]["suggested_action"]
            .as_str()
            .is_some_and(|action| action.contains("image-generation support")),
        "suggested_action should point at image route/provider support: {body}"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("request_id"))
    );
}

#[tokio::test]
async fn openai_images_edits_endpoint_translates_json_references_and_response() {
    let seen_body = Arc::new(Mutex::new(None::<serde_json::Value>));
    let seen_body_for_route = seen_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move |Json(body): Json<serde_json::Value>| {
            let seen_body_for_route = seen_body_for_route.clone();
            async move {
                *seen_body_for_route.lock().expect("lock") = Some(body);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "id": "resp_image_edit",
                        "created": 43,
                        "output": [
                            {
                                "type": "image_generation_call",
                                "id": "ig_1",
                                "status": "completed",
                                "result": "YmFy",
                                "revised_prompt": "A revised reference edit"
                            }
                        ]
                    })),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], RetryConfig::default());
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response = post_images_edits_json(
        &client,
        &proxy,
        r#"{
            "model":"gpt-image-2",
            "prompt":"draw a character sheet from the references",
            "images":[
                {"image_url":"data:image/png;base64,Zm9v"},
                {"file_id":"file_123"}
            ],
            "size":"2160x2880",
            "output_format":"png",
            "quality":"high",
            "input_fidelity":"high"
        }"#,
    )
    .await
    .error_for_status()
    .expect("images edits status")
    .json::<serde_json::Value>()
    .await
    .expect("images edits json");

    assert_eq!(response["created"], 43);
    assert_eq!(response["data"][0]["b64_json"], "YmFy");
    assert_eq!(
        response["data"][0]["revised_prompt"],
        "A revised reference edit"
    );

    let seen = seen_body
        .lock()
        .expect("lock")
        .clone()
        .expect("upstream body");
    assert_eq!(seen["model"], "gpt-5.5");
    assert_eq!(seen["tools"][0]["type"], "image_generation");
    assert_eq!(seen["tools"][0]["size"], "2160x2880");
    assert_eq!(seen["tools"][0]["output_format"], "png");
    assert_eq!(seen["tools"][0]["quality"], "high");
    assert_eq!(seen["tools"][0]["input_fidelity"], "high");
    assert_eq!(seen["tool_choice"]["type"], "image_generation");
    assert_eq!(seen["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(
        seen["input"][0]["content"][0]["text"],
        "draw a character sheet from the references"
    );
    assert_eq!(seen["input"][0]["content"][1]["type"], "input_image");
    assert_eq!(
        seen["input"][0]["content"][1]["image_url"],
        "data:image/png;base64,Zm9v"
    );
    assert_eq!(seen["input"][0]["content"][2]["type"], "input_image");
    assert_eq!(seen["input"][0]["content"][2]["file_id"], "file_123");
}

#[tokio::test]
async fn openai_images_edits_endpoint_rejects_n_greater_than_one_before_upstream() {
    let hit_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hit_count_for_route = hit_count.clone();
    let upstream = axum::Router::new().route(
        "/v1/responses",
        post(move || {
            let hit_count_for_route = hit_count_for_route.clone();
            async move {
                hit_count_for_route.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (
                    StatusCode::OK,
                    Json(serde_json::json!({"id": "resp_unused", "output": []})),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], RetryConfig::default());
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response = post_images_edits_json(
        &client,
        &proxy,
        r#"{"model":"gpt-image-2","prompt":"cat","images":["data:image/png;base64,Zm9v"],"n":2}"#,
    )
    .await;
    let status = response.status();
    let text = response.text().await.expect("body");

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(text.contains("n=1"));
    assert_eq!(hit_count.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn openai_images_edits_endpoint_passes_non_json_requests_through() {
    let seen_path = Arc::new(Mutex::new(None::<String>));
    let seen_content_type = Arc::new(Mutex::new(None::<String>));
    let seen_body = Arc::new(Mutex::new(None::<String>));
    let seen_path_for_route = seen_path.clone();
    let seen_content_type_for_route = seen_content_type.clone();
    let seen_body_for_route = seen_body.clone();
    let upstream = axum::Router::new().route(
        "/v1/images/edits",
        post(move |headers: HeaderMap, request: Request<Body>| {
            let seen_path_for_route = seen_path_for_route.clone();
            let seen_content_type_for_route = seen_content_type_for_route.clone();
            let seen_body_for_route = seen_body_for_route.clone();
            async move {
                *seen_path_for_route.lock().expect("lock") = Some(request.uri().path().to_string());
                *seen_content_type_for_route.lock().expect("lock") = headers
                    .get(axum::http::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned);
                let body = to_bytes(request.into_body(), 16 * 1024)
                    .await
                    .expect("body");
                *seen_body_for_route.lock().expect("lock") =
                    Some(String::from_utf8_lossy(body.as_ref()).to_string());
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "created": 44,
                        "data": [
                            {"b64_json": "YmF6"}
                        ]
                    })),
                )
            }
        }),
    );
    let upstream = spawn_test_upstream(upstream);
    let cfg = make_proxy_config(vec![upstream.upstream_config()], RetryConfig::default());
    let proxy = spawn_test_proxy(cfg);
    let client = Client::new();

    let response = client
        .post(proxy.images_edits_url())
        .header(
            "content-type",
            "multipart/form-data; boundary=codex-helper-test",
        )
        .body("--codex-helper-test\r\n\r\nbody\r\n--codex-helper-test--\r\n")
        .send()
        .await
        .expect("send multipart edits request")
        .error_for_status()
        .expect("multipart edits status")
        .json::<serde_json::Value>()
        .await
        .expect("multipart edits json");

    assert_eq!(response["data"][0]["b64_json"], "YmF6");
    assert_eq!(
        seen_path.lock().expect("lock").as_deref(),
        Some("/v1/images/edits")
    );
    assert_eq!(
        seen_content_type.lock().expect("lock").as_deref(),
        Some("multipart/form-data; boundary=codex-helper-test")
    );
    assert!(
        seen_body
            .lock()
            .expect("lock")
            .as_deref()
            .is_some_and(|body| body.contains("codex-helper-test"))
    );
}
