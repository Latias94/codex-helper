use axum::body::{Body, Bytes, to_bytes};
use axum::http::{HeaderMap, Request, Response, StatusCode, header, request::Parts};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ProxyService;
use super::handle_proxy;
use super::response_semantics::{
    HostedImageGenerationResultState, ResponseSemanticContract,
    hosted_image_generation_result_state,
};

const MAX_IMAGES_GENERATION_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_IMAGES_EDITS_REQUEST_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGES_RESPONSE_BYTES: usize = 96 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct OpenAiImagesGenerationRequest {
    model: String,
    prompt: String,
    #[serde(default)]
    n: Option<u32>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    output_format: Option<String>,
    #[serde(default)]
    moderation: Option<String>,
    #[serde(default)]
    user: Option<String>,
}

#[derive(Debug)]
struct OpenAiImagesEditRequest {
    model: String,
    prompt: String,
    images: Vec<ImageReference>,
    size: Option<String>,
    quality: Option<String>,
    background: Option<String>,
    output_format: Option<String>,
    moderation: Option<String>,
    input_fidelity: Option<String>,
    user: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawOpenAiImagesEditRequest {
    model: String,
    prompt: String,
    #[serde(default)]
    n: Option<u32>,
    #[serde(default, alias = "image")]
    images: Option<OneOrManyRawImageReference>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    output_format: Option<String>,
    #[serde(default)]
    moderation: Option<String>,
    #[serde(default)]
    input_fidelity: Option<String>,
    #[serde(default)]
    user: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrManyRawImageReference {
    Many(Vec<RawImageReference>),
    One(RawImageReference),
}

impl OneOrManyRawImageReference {
    fn into_vec(self) -> Vec<RawImageReference> {
        match self {
            Self::Many(references) => references,
            Self::One(reference) => vec![reference],
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawImageReference {
    Object(ImageReference),
    Url(String),
}

#[derive(Debug, Deserialize)]
struct ImageReference {
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    file_id: Option<String>,
}

#[derive(Debug)]
struct ImageGenerationResult {
    b64_json: String,
    revised_prompt: Option<String>,
}

pub(super) async fn handle_openai_images_generations(
    proxy: ProxyService,
    req: Request<Body>,
) -> Result<Response<Body>, (StatusCode, String)> {
    let (parts, body) = req.into_parts();
    let body_bytes = to_bytes(body, MAX_IMAGES_GENERATION_REQUEST_BYTES)
        .await
        .map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("failed to read images generation request body: {err}"),
            )
        })?;
    let image_request = parse_images_generation_request(&body_bytes)?;
    let responses_body = build_responses_image_generation_body(&image_request)?;
    let upstream_request = build_json_proxy_request(parts, "/v1/responses", responses_body)
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to build responses image generation request: {err}"),
            )
        })?;

    let response = handle_proxy(proxy, upstream_request)
        .await
        .map_err(openai_images_error)?;
    convert_responses_image_generation_response(response).await
}

pub(super) async fn handle_openai_images_edits(
    proxy: ProxyService,
    req: Request<Body>,
) -> Result<Response<Body>, (StatusCode, String)> {
    if !request_content_type_is_json(req.headers()) {
        return handle_proxy(proxy, req).await;
    }

    let (parts, body) = req.into_parts();
    let body_bytes = to_bytes(body, MAX_IMAGES_EDITS_REQUEST_BYTES)
        .await
        .map_err(|err| {
            (
                StatusCode::BAD_REQUEST,
                format!("failed to read images edits request body: {err}"),
            )
        })?;

    let json_value: Value = serde_json::from_slice(&body_bytes).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid images edits request JSON: {err}"),
        )
    })?;

    if json_value
        .get("mask")
        .is_some_and(|mask| !matches!(mask, Value::Null))
    {
        let upstream_request = build_original_proxy_request(parts, body_bytes)?;
        return handle_proxy(proxy, upstream_request).await;
    }

    let image_request = parse_images_edit_request(json_value)?;
    let responses_body = build_responses_image_edit_body(&image_request)?;
    let upstream_request = build_json_proxy_request(parts, "/v1/responses", responses_body)
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to build responses image edits request: {err}"),
            )
        })?;

    let response = handle_proxy(proxy, upstream_request)
        .await
        .map_err(openai_images_error)?;
    convert_responses_image_generation_response(response).await
}

fn parse_images_generation_request(
    body: &[u8],
) -> Result<OpenAiImagesGenerationRequest, (StatusCode, String)> {
    let request: OpenAiImagesGenerationRequest = serde_json::from_slice(body).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid images generation request JSON: {err}"),
        )
    })?;
    if request.model.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "model is required".to_string()));
    }
    if request.prompt.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "prompt is required".to_string()));
    }
    if request.n.unwrap_or(1) != 1 {
        return Err((
            StatusCode::BAD_REQUEST,
            "codex-helper images generation currently supports n=1 only".to_string(),
        ));
    }
    Ok(request)
}

fn parse_images_edit_request(
    value: Value,
) -> Result<OpenAiImagesEditRequest, (StatusCode, String)> {
    let request: RawOpenAiImagesEditRequest = serde_json::from_value(value).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid images edits request JSON: {err}"),
        )
    })?;
    if request.model.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "model is required".to_string()));
    }
    if request.prompt.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "prompt is required".to_string()));
    }
    if request.n.unwrap_or(1) != 1 {
        return Err((
            StatusCode::BAD_REQUEST,
            "codex-helper images edits currently supports n=1 only".to_string(),
        ));
    }

    let Some(raw_images) = request.images else {
        return Err((StatusCode::BAD_REQUEST, "images is required".to_string()));
    };
    let images = raw_images
        .into_vec()
        .into_iter()
        .map(normalize_image_reference)
        .collect::<Result<Vec<_>, _>>()?;
    if images.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "at least one image reference is required".to_string(),
        ));
    }
    if images.len() > 16 {
        return Err((
            StatusCode::BAD_REQUEST,
            "codex-helper images edits supports at most 16 image references".to_string(),
        ));
    }

    Ok(OpenAiImagesEditRequest {
        model: request.model,
        prompt: request.prompt,
        images,
        size: request.size,
        quality: request.quality,
        background: request.background,
        output_format: request.output_format,
        moderation: request.moderation,
        input_fidelity: request.input_fidelity,
        user: request.user,
    })
}

fn normalize_image_reference(
    raw: RawImageReference,
) -> Result<ImageReference, (StatusCode, String)> {
    let reference = match raw {
        RawImageReference::Object(reference) => reference,
        RawImageReference::Url(image_url) => ImageReference {
            image_url: Some(image_url),
            file_id: None,
        },
    };
    let image_url = reference
        .image_url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let file_id = reference
        .file_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    match (image_url, file_id) {
        (Some(image_url), None) => Ok(ImageReference {
            image_url: Some(image_url),
            file_id: None,
        }),
        (None, Some(file_id)) => Ok(ImageReference {
            image_url: None,
            file_id: Some(file_id),
        }),
        (None, None) => Err((
            StatusCode::BAD_REQUEST,
            "each image reference must include image_url or file_id".to_string(),
        )),
        (Some(_), Some(_)) => Err((
            StatusCode::BAD_REQUEST,
            "each image reference must include only one of image_url or file_id".to_string(),
        )),
    }
}

fn build_responses_image_generation_body(
    request: &OpenAiImagesGenerationRequest,
) -> Result<Vec<u8>, (StatusCode, String)> {
    let mut tool = json!({
        "type": "image_generation",
    });
    copy_optional_string(&mut tool, "size", request.size.as_deref());
    copy_optional_string(&mut tool, "quality", request.quality.as_deref());
    copy_optional_string(&mut tool, "background", request.background.as_deref());
    copy_optional_string(&mut tool, "output_format", request.output_format.as_deref());
    copy_optional_string(&mut tool, "moderation", request.moderation.as_deref());

    let mut body = json!({
        "model": request.model,
        "input": request.prompt,
        "tools": [tool],
    });
    copy_optional_string(&mut body, "user", request.user.as_deref());

    serde_json::to_vec(&body).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to serialize responses image generation request: {err}"),
        )
    })
}

fn build_responses_image_edit_body(
    request: &OpenAiImagesEditRequest,
) -> Result<Vec<u8>, (StatusCode, String)> {
    let mut tool = json!({
        "type": "image_generation",
    });
    copy_optional_string(&mut tool, "size", request.size.as_deref());
    copy_optional_string(&mut tool, "quality", request.quality.as_deref());
    copy_optional_string(&mut tool, "background", request.background.as_deref());
    copy_optional_string(&mut tool, "output_format", request.output_format.as_deref());
    copy_optional_string(&mut tool, "moderation", request.moderation.as_deref());
    copy_optional_string(
        &mut tool,
        "input_fidelity",
        request.input_fidelity.as_deref(),
    );

    let mut content = vec![json!({
        "type": "input_text",
        "text": request.prompt,
    })];
    for image in &request.images {
        let mut item = json!({
            "type": "input_image",
        });
        copy_optional_string(&mut item, "image_url", image.image_url.as_deref());
        copy_optional_string(&mut item, "file_id", image.file_id.as_deref());
        content.push(item);
    }

    let mut body = json!({
        "model": request.model,
        "input": [
            {
                "role": "user",
                "content": content,
            }
        ],
        "tools": [tool],
    });
    copy_optional_string(&mut body, "user", request.user.as_deref());

    serde_json::to_vec(&body).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to serialize responses image edits request: {err}"),
        )
    })
}

fn copy_optional_string(target: &mut Value, key: &str, value: Option<&str>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    target[key] = Value::String(value.to_string());
}

fn request_content_type_is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| {
            let value = value.to_ascii_lowercase();
            value.contains("application/json") || value.contains("+json")
        })
}

fn build_json_proxy_request(
    parts: Parts,
    uri: &str,
    body: Vec<u8>,
) -> Result<Request<Body>, axum::http::Error> {
    let mut builder = Request::builder().method(parts.method).uri(uri);
    for (name, value) in &parts.headers {
        if name != header::CONTENT_LENGTH && name != header::CONTENT_TYPE {
            builder = builder.header(name, value);
        }
    }
    builder = builder.header(header::CONTENT_TYPE, "application/json");
    let mut request = builder.body(Body::from(Bytes::from(body)))?;
    *request.extensions_mut() = parts.extensions;
    request
        .extensions_mut()
        .insert(ResponseSemanticContract::HostedImageGeneration);
    Ok(request)
}

fn build_original_proxy_request(
    parts: Parts,
    body: Bytes,
) -> Result<Request<Body>, (StatusCode, String)> {
    let mut builder = Request::builder().method(parts.method).uri(parts.uri);
    for (name, value) in &parts.headers {
        if name != header::CONTENT_LENGTH {
            builder = builder.header(name, value);
        }
    }
    let mut request = builder.body(Body::from(body)).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to rebuild images edits proxy request: {err}"),
        )
    })?;
    *request.extensions_mut() = parts.extensions;
    Ok(request)
}

async fn convert_responses_image_generation_response(
    response: Response<Body>,
) -> Result<Response<Body>, (StatusCode, String)> {
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), MAX_IMAGES_RESPONSE_BYTES)
        .await
        .map_err(|err| {
            (
                StatusCode::BAD_GATEWAY,
                format!("failed to read responses image generation response body: {err}"),
            )
        })?;

    if !status.is_success() {
        return Ok(build_response(
            status,
            content_type_from_headers(&headers),
            body,
        ));
    }

    let responses_json: Value = serde_json::from_slice(&body).map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("upstream image generation response was not valid JSON: {err}"),
        )
    })?;
    let image_result = extract_image_generation_result(&responses_json)?;
    let created = responses_json
        .get("created")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0)
        });

    let mut data = json!({
        "b64_json": image_result.b64_json,
    });
    copy_optional_string(
        &mut data,
        "revised_prompt",
        image_result.revised_prompt.as_deref(),
    );
    let body = json!({
        "created": created,
        "data": [data],
    });
    let body = serde_json::to_vec(&body).map(Bytes::from).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to serialize images generation response: {err}"),
        )
    })?;

    Ok(build_response(
        StatusCode::OK,
        Some("application/json"),
        body,
    ))
}

fn extract_image_generation_result(
    value: &Value,
) -> Result<ImageGenerationResult, (StatusCode, String)> {
    match hosted_image_generation_result_state(value) {
        HostedImageGenerationResultState::Completed => {}
        HostedImageGenerationResultState::NoOutputArray => {
            return Err((
                StatusCode::BAD_GATEWAY,
                "upstream response did not contain an output array".to_string(),
            ));
        }
        HostedImageGenerationResultState::MissingResult => {
            return Err((
                StatusCode::BAD_GATEWAY,
                "upstream response contained no completed image_generation_call result".to_string(),
            ));
        }
    }

    let output = value
        .get("output")
        .and_then(Value::as_array)
        .expect("validated hosted image output array");
    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
            continue;
        }
        if let Some(result) = item.get("result").and_then(Value::as_str)
            && !result.trim().is_empty()
        {
            return Ok(ImageGenerationResult {
                b64_json: result.to_string(),
                revised_prompt: item
                    .get("revised_prompt")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            });
        }
    }

    Err((
        StatusCode::BAD_GATEWAY,
        "upstream response contained no completed image_generation_call result".to_string(),
    ))
}

fn content_type_from_headers(headers: &axum::http::HeaderMap) -> Option<&'static str> {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            if value.starts_with("application/json") {
                "application/json"
            } else {
                "text/plain"
            }
        })
}

fn build_response(status: StatusCode, content_type: Option<&str>, body: Bytes) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(Body::from(body)).unwrap()
}

fn openai_images_error(error: (StatusCode, String)) -> (StatusCode, String) {
    let (status, message) = error;
    if looks_like_json(&message) {
        return (status, message);
    }
    let body = json!({
        "error": {
            "message": message,
            "type": "image_generation_route_failed",
            "retryable": status.is_server_error(),
        }
    });
    (
        status,
        serde_json::to_string(&body).unwrap_or_else(|_| {
            r#"{"error":{"message":"image generation route failed","type":"image_generation_route_failed","retryable":true}}"#
                .to_string()
        }),
    )
}

fn looks_like_json(value: &str) -> bool {
    value.trim_start().starts_with('{') || value.trim_start().starts_with('[')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_images_generation_body_maps_to_responses_tool_request() {
        let request = OpenAiImagesGenerationRequest {
            model: "gpt-image-2".to_string(),
            prompt: "一只猫在雨夜的霓虹灯下".to_string(),
            n: None,
            size: Some("3840x2160".to_string()),
            quality: Some("high".to_string()),
            background: None,
            output_format: Some("png".to_string()),
            moderation: None,
            user: None,
        };

        let body = build_responses_image_generation_body(&request).expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json");

        assert_eq!(json["model"], "gpt-image-2");
        assert_eq!(json["input"], "一只猫在雨夜的霓虹灯下");
        assert_eq!(json["tools"][0]["type"], "image_generation");
        assert_eq!(json["tools"][0]["size"], "3840x2160");
        assert_eq!(json["tools"][0]["quality"], "high");
        assert_eq!(json["tools"][0]["output_format"], "png");
        assert!(json.get("tool_choice").is_none());
    }

    #[test]
    fn openai_images_generation_rejects_multi_image_requests() {
        let body = br#"{"model":"gpt-image-2","prompt":"cat","n":2}"#;
        let err = parse_images_generation_request(body).expect_err("n>1 rejected");

        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("n=1"));
    }

    #[test]
    fn openai_images_generation_extracts_response_result() {
        let response = json!({
            "output": [
                {"type": "message", "content": []},
                {
                    "type": "image_generation_call",
                    "id": "ig_1",
                    "status": "completed",
                    "result": "Zm9v",
                    "revised_prompt": "revised cat"
                }
            ]
        });

        let result = extract_image_generation_result(&response).expect("image result");

        assert_eq!(result.b64_json, "Zm9v");
        assert_eq!(result.revised_prompt.as_deref(), Some("revised cat"));
    }

    #[test]
    fn openai_images_edit_body_maps_references_to_responses_input_images() {
        let request = parse_images_edit_request(json!({
            "model": "gpt-image-2",
            "prompt": "restyle using references",
            "images": [
                {"image_url": "data:image/png;base64,Zm9v"},
                {"file_id": "file_123"}
            ],
            "size": "3840x2160",
            "output_format": "png",
            "quality": "high"
        }))
        .expect("edit request");

        let body = build_responses_image_edit_body(&request).expect("body");
        let json: Value = serde_json::from_slice(&body).expect("json");

        assert_eq!(json["model"], "gpt-image-2");
        assert_eq!(json["tools"][0]["type"], "image_generation");
        assert_eq!(json["tools"][0]["size"], "3840x2160");
        assert_eq!(json["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(
            json["input"][0]["content"][0]["text"],
            "restyle using references"
        );
        assert_eq!(json["input"][0]["content"][1]["type"], "input_image");
        assert_eq!(
            json["input"][0]["content"][1]["image_url"],
            "data:image/png;base64,Zm9v"
        );
        assert_eq!(json["input"][0]["content"][2]["type"], "input_image");
        assert_eq!(json["input"][0]["content"][2]["file_id"], "file_123");
    }

    #[test]
    fn openai_images_edit_rejects_empty_references() {
        let err = parse_images_edit_request(json!({
            "model": "gpt-image-2",
            "prompt": "cat",
            "images": []
        }))
        .expect_err("empty images rejected");

        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(
            err.1.contains("at least one image"),
            "unexpected error: {}",
            err.1
        );
    }
}
