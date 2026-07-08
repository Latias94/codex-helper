use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use super::ProxyControlError;

pub(super) type AdminApiResult<T> = Result<Json<T>, AdminApiHttpError>;

#[derive(Debug, Clone, Serialize)]
pub(super) struct AdminApiError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct AdminApiHttpError {
    status: StatusCode,
    error: AdminApiError,
}

impl AdminApiHttpError {
    pub(super) fn new(
        status: StatusCode,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let retryable = status.is_server_error()
            || matches!(
                status,
                StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
            );
        Self {
            status,
            error: AdminApiError {
                code: code.into(),
                message: message.into(),
                retryable,
                hint: None,
            },
        }
    }

    pub(super) fn bad_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub(super) fn internal(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, code, message)
    }

    pub(super) fn service_unavailable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, code, message)
    }

    fn code_for_status(status: StatusCode) -> &'static str {
        match status {
            StatusCode::BAD_REQUEST => "admin_bad_request",
            StatusCode::UNAUTHORIZED => "admin_unauthorized",
            StatusCode::FORBIDDEN => "admin_forbidden",
            StatusCode::NOT_FOUND => "admin_not_found",
            StatusCode::CONFLICT => "admin_conflict",
            StatusCode::TOO_MANY_REQUESTS => "admin_rate_limited",
            StatusCode::SERVICE_UNAVAILABLE => "admin_service_unavailable",
            status if status.is_server_error() => "admin_internal_error",
            _ => "admin_request_failed",
        }
    }
}

impl From<ProxyControlError> for AdminApiHttpError {
    fn from(error: ProxyControlError) -> Self {
        Self::new(
            error.status(),
            Self::code_for_status(error.status()),
            error.message().to_string(),
        )
    }
}

impl From<(StatusCode, String)> for AdminApiHttpError {
    fn from((status, message): (StatusCode, String)) -> Self {
        Self::new(status, Self::code_for_status(status), message)
    }
}

impl From<AdminApiHttpError> for ProxyControlError {
    fn from(error: AdminApiHttpError) -> Self {
        ProxyControlError::new(error.status, error.error.message)
    }
}

impl IntoResponse for AdminApiHttpError {
    fn into_response(self) -> Response {
        (self.status, Json(self.error)).into_response()
    }
}
