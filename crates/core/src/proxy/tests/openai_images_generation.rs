use super::*;
use crate::proxy::tests::harness::{
    post_images_generations_json, spawn_test_proxy, spawn_test_upstream,
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
    assert_eq!(seen["model"], "gpt-image-2");
    assert_eq!(seen["input"], "一只猫在雨夜的霓虹灯下");
    assert_eq!(seen["tools"][0]["type"], "image_generation");
    assert_eq!(seen["tools"][0]["size"], "3840x2160");
    assert_eq!(seen["tools"][0]["output_format"], "png");
    assert_eq!(seen["tools"][0]["quality"], "high");
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
