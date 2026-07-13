use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode, header};

use crate::logging::{CodexBridgeLog, RetryInfo, ServiceTierLog};
use crate::runtime_store::AttemptHandle;
use crate::state::{RouteDecisionProvenance, SessionIdentitySource};
use crate::usage::UsageMetrics;

use super::ProxyService;
use super::request_observer::{RequestObserver, RequestPublication};

pub(super) struct FinalizeForwardResponseParams {
    pub request_id: u64,
    pub winning_attempt: Option<AttemptHandle>,
    pub status: StatusCode,
    pub duration_ms: u64,
    pub started_at_ms: u64,
    pub upstream_headers_ms: u64,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub provider_endpoint_key: Option<String>,
    pub upstream_origin: Option<String>,
    pub session_id: Option<String>,
    pub session_identity_source: Option<SessionIdentitySource>,
    pub cwd: Option<String>,
    pub effective_effort: Option<String>,
    pub service_tier: ServiceTierLog,
    pub reported_model: Option<String>,
    pub codex_bridge: Option<CodexBridgeLog>,
    pub usage: Option<UsageMetrics>,
    pub route_decision: Option<RouteDecisionProvenance>,
    pub retry: Option<RetryInfo>,
    pub response_headers: HeaderMap,
    pub response_body: Bytes,
}

pub(super) struct FinalizedForwardResponse {
    pub(super) response: Response<Body>,
    pub(super) terminal_published: bool,
}

pub(super) async fn finish_and_build_forward_response(
    proxy: &ProxyService,
    method: &Method,
    path: &str,
    params: FinalizeForwardResponseParams,
) -> FinalizedForwardResponse {
    let FinalizeForwardResponseParams {
        request_id,
        winning_attempt,
        status,
        duration_ms,
        started_at_ms,
        upstream_headers_ms,
        provider_id,
        endpoint_id,
        provider_endpoint_key,
        upstream_origin,
        session_id,
        session_identity_source,
        cwd,
        effective_effort,
        service_tier,
        reported_model,
        codex_bridge,
        usage,
        route_decision,
        retry,
        response_headers,
        response_body,
    } = params;

    let status_code = status.as_u16();
    let streaming = response_headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"));
    let mut publication = RequestPublication::new_terminal(
        request_id,
        status_code,
        duration_ms,
        started_at_ms,
        streaming,
    );
    publication.ttfb_ms = Some(upstream_headers_ms);
    publication.winning_attempt = winning_attempt;
    publication.provider_id = provider_id;
    publication.endpoint_id = endpoint_id;
    publication.provider_endpoint_key = provider_endpoint_key;
    publication.upstream_origin = upstream_origin;
    publication.session_id = session_id;
    publication.session_identity_source = session_identity_source;
    publication.cwd = cwd;
    publication.reasoning_effort = effective_effort;
    publication.service_tier = service_tier;
    publication.reported_model = reported_model;
    publication.codex_bridge = codex_bridge;
    publication.usage = usage;
    publication.route_decision = route_decision;
    publication.retry = retry;
    let terminal_published = RequestObserver::new(proxy, method, path)
        .publish_terminal_once(publication.with_route_decision_model())
        .await;
    let response = if terminal_published {
        build_forward_response(status, &response_headers, response_body)
    } else {
        Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "error": {
                        "type": "server_error",
                        "code": "request_terminal_commit_failed",
                        "message": "failed to commit request terminal"
                    }
                })
                .to_string(),
            ))
            .expect("terminal commit failure response should build")
    };

    FinalizedForwardResponse {
        response,
        terminal_published,
    }
}

fn build_forward_response(status: StatusCode, headers: &HeaderMap, body: Bytes) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder.body(Body::from(body)).unwrap()
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::HeaderValue;

    use super::*;

    #[tokio::test]
    async fn build_forward_response_keeps_headers_and_body() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        let response = build_forward_response(
            StatusCode::CREATED,
            &headers,
            Bytes::from_static(br#"{"ok":true}"#),
        );

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.headers().get("content-type"),
            Some(&HeaderValue::from_static("application/json"))
        );

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        assert_eq!(body.as_ref(), br#"{"ok":true}"#);
    }
}
