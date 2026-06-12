use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use serde_json::{Value, json};

pub(super) const IMAGE_GENERATION_MISSING_RESULT_CLASS: &str = "image_generation_missing_result";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponseSemanticContract {
    HostedImageGeneration,
}

#[derive(Debug)]
pub(super) struct SemanticResponseFailure {
    pub(super) status: StatusCode,
    pub(super) error_class: &'static str,
    pub(super) message: String,
    pub(super) response_body: Bytes,
    pub(super) response_headers: HeaderMap,
}

pub(super) fn validate_success_response_semantics(
    contract: Option<ResponseSemanticContract>,
    response_body: &Bytes,
) -> Result<(), SemanticResponseFailure> {
    match contract {
        Some(ResponseSemanticContract::HostedImageGeneration) => {
            validate_hosted_image_generation_response(response_body.as_ref())
        }
        None => Ok(()),
    }
}

fn validate_hosted_image_generation_response(body: &[u8]) -> Result<(), SemanticResponseFailure> {
    match serde_json::from_slice::<Value>(body) {
        Ok(value) => match hosted_image_generation_result_state(&value) {
            HostedImageGenerationResultState::Completed => Ok(()),
            HostedImageGenerationResultState::NoOutputArray => Err(hosted_image_failure(
                "upstream image generation response did not contain an output array",
            )),
            HostedImageGenerationResultState::MissingResult => Err(hosted_image_failure(
                "upstream response contained no completed image_generation_call result",
            )),
        },
        Err(error) => Err(hosted_image_failure(format!(
            "upstream image generation response was not valid JSON: {error}"
        ))),
    }
}

pub(super) enum HostedImageGenerationResultState {
    Completed,
    NoOutputArray,
    MissingResult,
}

pub(super) fn hosted_image_generation_result_state(
    value: &Value,
) -> HostedImageGenerationResultState {
    let Some(output) = value.get("output").and_then(Value::as_array) else {
        return HostedImageGenerationResultState::NoOutputArray;
    };

    let has_completed_result = output.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("image_generation_call")
            && item
                .get("result")
                .and_then(Value::as_str)
                .is_some_and(|result| !result.trim().is_empty())
    });
    if has_completed_result {
        HostedImageGenerationResultState::Completed
    } else {
        HostedImageGenerationResultState::MissingResult
    }
}

fn hosted_image_failure(message: impl Into<String>) -> SemanticResponseFailure {
    let message = message.into();
    let response_body = json!({
        "error": {
            "message": message,
            "type": IMAGE_GENERATION_MISSING_RESULT_CLASS,
            "retryable": true,
        }
    });
    SemanticResponseFailure {
        status: StatusCode::BAD_GATEWAY,
        error_class: IMAGE_GENERATION_MISSING_RESULT_CLASS,
        message,
        response_body: Bytes::from(
            serde_json::to_vec(&response_body).unwrap_or_else(|_| b"{}".to_vec()),
        ),
        response_headers: json_response_headers(),
    }
}

fn json_response_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_image_result_state_accepts_completed_result() {
        let value = json!({
            "output": [
                {"type": "message", "content": []},
                {"type": "image_generation_call", "result": "Zm9v"}
            ]
        });

        assert!(matches!(
            hosted_image_generation_result_state(&value),
            HostedImageGenerationResultState::Completed
        ));
    }

    #[test]
    fn hosted_image_result_state_rejects_missing_result() {
        let value = json!({
            "output": [
                {"type": "image_generation_call", "status": "incomplete"}
            ]
        });

        assert!(matches!(
            hosted_image_generation_result_state(&value),
            HostedImageGenerationResultState::MissingResult
        ));
    }
}
