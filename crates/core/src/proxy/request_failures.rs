use axum::http::{Method, StatusCode};

use crate::logging::{
    CodexBridgeLog, HeaderEntry, HttpDebugLog, RetryInfo, RouteAttemptLog, ServiceTierLog,
    log_request_with_debug, should_include_http_warn,
};
use crate::state::SessionIdentitySource;

use super::ProxyService;
use super::failure_summary::failed_proxy_client_message;
use super::request_observer::{RequestObserver, RequestPublication};

const EMPTY_TARGET_URL: &str = "-";
const CLIENT_BODY_READ_ERROR_HINT: &str =
    "读取客户端请求 body 失败（可能超过大小限制或连接中断）。";

pub(super) struct ClientBodyReadErrorParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) method: &'a Method,
    pub(super) path: &'a str,
    pub(super) client_uri: &'a str,
    pub(super) session_id: Option<String>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) cwd: Option<String>,
    pub(super) client_headers: Vec<HeaderEntry>,
    pub(super) duration_ms: u64,
    pub(super) error_message: String,
}

pub(super) struct FailedProxyRequestParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) method: &'a Method,
    pub(super) path: &'a str,
    pub(super) request_id: u64,
    pub(super) status: StatusCode,
    pub(super) message: String,
    pub(super) duration_ms: u64,
    pub(super) started_at_ms: u64,
    pub(super) session_id: Option<String>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) cwd: Option<String>,
    pub(super) effective_effort: Option<String>,
    pub(super) service_tier: ServiceTierLog,
    pub(super) codex_bridge: Option<CodexBridgeLog>,
    pub(super) retry: Option<RetryInfo>,
    pub(super) failure_route_attempts: Vec<RouteAttemptLog>,
}

pub(super) fn log_client_body_read_error(
    params: ClientBodyReadErrorParams<'_>,
) -> (StatusCode, String) {
    let ClientBodyReadErrorParams {
        proxy,
        method,
        path,
        client_uri,
        session_id,
        session_identity_source,
        cwd,
        client_headers,
        duration_ms,
        error_message,
    } = params;

    let status = StatusCode::BAD_REQUEST;
    let http_debug = build_early_error_http_debug(
        status,
        client_uri,
        client_headers,
        "client_body_read_error",
        CLIENT_BODY_READ_ERROR_HINT,
        error_message.as_str(),
    );

    log_request_with_debug(
        None,
        proxy.service_name,
        method.as_str(),
        path,
        status.as_u16(),
        duration_ms,
        None,
        None,
        None,
        None,
        None,
        EMPTY_TARGET_URL,
        session_id,
        session_identity_source,
        cwd,
        None,
        None,
        ServiceTierLog::default(),
        None,
        None,
        None,
        None,
        http_debug,
    );

    (status, error_message)
}

pub(super) async fn finish_failed_proxy_request(
    params: FailedProxyRequestParams<'_>,
) -> (StatusCode, String) {
    let FailedProxyRequestParams {
        proxy,
        method,
        path,
        request_id,
        status,
        message,
        duration_ms,
        started_at_ms,
        session_id,
        session_identity_source,
        cwd,
        effective_effort,
        service_tier,
        codex_bridge,
        retry,
        failure_route_attempts,
    } = params;
    let client_message = failed_proxy_client_message(
        status,
        message.as_str(),
        request_id,
        retry.as_ref(),
        &failure_route_attempts,
    );

    RequestObserver::new(proxy, method, path)
        .publish_terminal_once(RequestPublication {
            request_id,
            status_code: status.as_u16(),
            duration_ms,
            ended_at_ms: started_at_ms + duration_ms,
            ttfb_ms: None,
            station_name: None,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            upstream_base_url: EMPTY_TARGET_URL.to_string(),
            session_id,
            session_identity_source,
            cwd,
            model: None,
            reasoning_effort: effective_effort,
            service_tier,
            codex_bridge,
            usage: None,
            route_decision: None,
            retry,
            http_debug: None,
            streaming: false,
        })
        .await;

    (status, client_message)
}

fn build_early_error_http_debug(
    status: StatusCode,
    client_uri: &str,
    client_headers: Vec<HeaderEntry>,
    error_class: &str,
    error_hint: &str,
    error_message: &str,
) -> Option<HttpDebugLog> {
    should_include_http_warn(status.as_u16()).then(|| HttpDebugLog {
        request_body_len: None,
        upstream_request_body_len: None,
        upstream_headers_ms: None,
        upstream_first_chunk_ms: None,
        upstream_body_read_ms: None,
        upstream_error_class: Some(error_class.to_string()),
        upstream_error_hint: Some(error_hint.to_string()),
        upstream_cf_ray: None,
        client_uri: client_uri.to_string(),
        target_url: EMPTY_TARGET_URL.to_string(),
        client_headers,
        upstream_request_headers: Vec::new(),
        auth_resolution: None,
        client_body: None,
        upstream_request_body: None,
        upstream_response_headers: None,
        upstream_response_body: None,
        upstream_error: Some(error_message.to_string()),
    })
}
