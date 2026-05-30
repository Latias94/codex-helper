use axum::body::Body;
use axum::http::{Response, StatusCode, header};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy)]
pub(super) enum CodexFailureKind {
    RouteUnavailable,
    UpstreamFailure,
    StreamError,
}

impl CodexFailureKind {
    pub(super) fn helper_error(self) -> &'static str {
        match self {
            Self::RouteUnavailable => "route_unavailable",
            Self::UpstreamFailure => "upstream_failure",
            Self::StreamError => "upstream_stream_error",
        }
    }

    fn error_code(self) -> &'static str {
        match self {
            Self::RouteUnavailable | Self::UpstreamFailure => "rate_limit_exceeded",
            Self::StreamError => "upstream_error",
        }
    }

    fn response_id_prefix(self) -> &'static str {
        match self {
            Self::RouteUnavailable => "resp_codex_helper_route_unavailable",
            Self::UpstreamFailure => "resp_codex_helper_upstream_failure",
            Self::StreamError => "resp_codex_helper_stream_error",
        }
    }

    fn include_sequence_number(self) -> bool {
        matches!(self, Self::RouteUnavailable | Self::UpstreamFailure)
    }

    fn include_empty_output(self) -> bool {
        matches!(self, Self::StreamError)
    }

    fn leading_blank_event(self) -> bool {
        matches!(self, Self::StreamError)
    }
}

pub(super) struct CodexFailureSse<'a> {
    kind: CodexFailureKind,
    message: &'a str,
    retry_after_secs: Option<u64>,
    model: Option<&'a str>,
    helper_error: Option<&'a str>,
}

impl<'a> CodexFailureSse<'a> {
    pub(super) fn route_failure(
        message: &'a str,
        retry_after_secs: u64,
        kind: CodexFailureKind,
    ) -> Self {
        debug_assert!(matches!(
            kind,
            CodexFailureKind::RouteUnavailable | CodexFailureKind::UpstreamFailure
        ));
        Self {
            kind,
            message,
            retry_after_secs: Some(retry_after_secs),
            model: None,
            helper_error: None,
        }
    }

    pub(super) fn stream_error(
        message: &'a str,
        model: Option<&'a str>,
        helper_error: &'a str,
    ) -> Self {
        Self {
            kind: CodexFailureKind::StreamError,
            message,
            retry_after_secs: None,
            model,
            helper_error: Some(helper_error),
        }
    }

    pub(super) fn to_event_string(&self) -> String {
        let now_ms = crate::logging::now_ms();
        let helper_error = self
            .helper_error
            .unwrap_or_else(|| self.kind.helper_error());

        let mut metadata = Map::new();
        metadata.insert("codex_helper_error".to_string(), json!(helper_error));
        if let Some(retry_after_secs) = self.retry_after_secs {
            metadata.insert("retry_after_secs".to_string(), json!(retry_after_secs));
        }

        let mut response = Map::new();
        response.insert(
            "id".to_string(),
            json!(format!("{}_{}", self.kind.response_id_prefix(), now_ms)),
        );
        response.insert("object".to_string(), json!("response"));
        response.insert("created_at".to_string(), json!(now_ms / 1000));
        response.insert("status".to_string(), json!("failed"));
        response.insert("background".to_string(), json!(false));
        if self.kind.include_empty_output() {
            response.insert("output".to_string(), json!([]));
        }
        response.insert(
            "error".to_string(),
            json!({
                "code": self.kind.error_code(),
                "message": self.message,
            }),
        );
        response.insert("usage".to_string(), Value::Null);
        response.insert("user".to_string(), Value::Null);
        response.insert("metadata".to_string(), Value::Object(metadata));
        if let Some(model) = self.model.filter(|model| !model.trim().is_empty()) {
            response.insert("model".to_string(), json!(model));
        }

        let mut payload = Map::new();
        payload.insert("type".to_string(), json!("response.failed"));
        if self.kind.include_sequence_number() {
            payload.insert("sequence_number".to_string(), json!(1));
        }
        payload.insert("response".to_string(), Value::Object(response));

        let event = format!(
            "event: response.failed\ndata: {}\n\n",
            Value::Object(payload)
        );
        if self.kind.leading_blank_event() {
            format!("\n\n{event}")
        } else {
            event
        }
    }

    pub(super) fn into_response(self) -> Response<Body> {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(self.to_event_string()))
            .expect("synthetic SSE response should build")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_payload(event: &str) -> Value {
        let data = event
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("data line");
        serde_json::from_str(data).expect("payload json")
    }

    #[test]
    fn route_failure_event_keeps_retry_metadata_and_sequence() {
        let event = CodexFailureSse::route_failure(
            "No upstreams are currently routable; try again in 8 seconds",
            8,
            CodexFailureKind::RouteUnavailable,
        )
        .to_event_string();
        let payload = event_payload(&event);

        assert_eq!(payload["type"].as_str(), Some("response.failed"));
        assert_eq!(payload["sequence_number"].as_i64(), Some(1));
        assert_eq!(
            payload["response"]["error"]["code"].as_str(),
            Some("rate_limit_exceeded")
        );
        assert_eq!(
            payload["response"]["metadata"]["codex_helper_error"].as_str(),
            Some("route_unavailable")
        );
        assert_eq!(
            payload["response"]["metadata"]["retry_after_secs"].as_u64(),
            Some(8)
        );
    }

    #[test]
    fn stream_error_event_keeps_model_and_dynamic_helper_error() {
        let event = CodexFailureSse::stream_error(
            "Upstream stream idle timeout after 900s without bytes",
            Some("gpt-5.5"),
            "upstream_stream_idle_timeout",
        )
        .to_event_string();
        let payload = event_payload(&event);

        assert!(event.starts_with("\n\nevent: response.failed\n"));
        assert_eq!(payload["type"].as_str(), Some("response.failed"));
        assert!(payload.get("sequence_number").is_none());
        assert_eq!(
            payload["response"]["error"]["code"].as_str(),
            Some("upstream_error")
        );
        assert_eq!(payload["response"]["model"].as_str(), Some("gpt-5.5"));
        assert_eq!(
            payload["response"]["output"].as_array().map(Vec::len),
            Some(0)
        );
        assert_eq!(
            payload["response"]["metadata"]["codex_helper_error"].as_str(),
            Some("upstream_stream_idle_timeout")
        );
    }
}
