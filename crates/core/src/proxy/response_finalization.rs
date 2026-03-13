use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode};

use crate::logging::{RetryInfo, ServiceTierLog, log_request_with_debug};
use crate::state::FinishRequestParams;
use crate::usage::UsageMetrics;

use super::ProxyService;

pub(super) struct FinalizeForwardResponseParams {
    pub request_id: u64,
    pub status: StatusCode,
    pub duration_ms: u64,
    pub started_at_ms: u64,
    pub upstream_headers_ms: u64,
    pub station_name: String,
    pub provider_id: Option<String>,
    pub upstream_base_url: String,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub effective_effort: Option<String>,
    pub service_tier: ServiceTierLog,
    pub usage: Option<UsageMetrics>,
    pub retry: Option<RetryInfo>,
    pub response_headers: HeaderMap,
    pub response_body: Bytes,
}

pub(super) async fn finish_and_build_forward_response(
    proxy: &ProxyService,
    method: &Method,
    path: &str,
    params: FinalizeForwardResponseParams,
) -> Response<Body> {
    let FinalizeForwardResponseParams {
        request_id,
        status,
        duration_ms,
        started_at_ms,
        upstream_headers_ms,
        station_name,
        provider_id,
        upstream_base_url,
        session_id,
        cwd,
        effective_effort,
        service_tier,
        usage,
        retry,
        response_headers,
        response_body,
    } = params;

    proxy
        .state
        .finish_request(FinishRequestParams {
            id: request_id,
            status_code: status.as_u16(),
            duration_ms,
            ended_at_ms: started_at_ms + duration_ms,
            observed_service_tier: service_tier.actual.clone(),
            usage: usage.clone(),
            retry: retry.clone(),
            ttfb_ms: Some(upstream_headers_ms),
        })
        .await;

    log_request_with_debug(
        proxy.service_name,
        method.as_str(),
        path,
        status.as_u16(),
        duration_ms,
        Some(upstream_headers_ms),
        station_name.as_str(),
        provider_id,
        upstream_base_url.as_str(),
        session_id,
        cwd,
        effective_effort,
        service_tier,
        usage,
        retry,
        None,
    );

    build_forward_response(status, &response_headers, response_body)
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
