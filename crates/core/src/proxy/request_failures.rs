use axum::http::{Method, StatusCode};

use crate::logging::{
    HeaderEntry, HttpDebugLog, RetryInfo, ServiceTierLog, log_request_with_debug,
    should_include_http_warn,
};
use crate::state::FinishRequestParams;

use super::ProxyService;

const EMPTY_TARGET_URL: &str = "-";
const NO_ROUTABLE_STATION_HINT: &str =
    "未找到任何可用的上游站点（active_station 未设置，或目标站点没有可用 upstream）。";
const CLIENT_BODY_READ_ERROR_HINT: &str =
    "读取客户端请求 body 失败（可能超过大小限制或连接中断）。";

pub(super) fn log_no_routable_station(
    proxy: &ProxyService,
    method: &Method,
    path: &str,
    client_uri: &str,
    session_id: Option<String>,
    client_headers: Vec<HeaderEntry>,
    duration_ms: u64,
) -> (StatusCode, String) {
    let status = StatusCode::BAD_GATEWAY;
    let message = "no routable station".to_string();
    let http_debug = build_early_error_http_debug(
        status,
        client_uri,
        client_headers,
        "no_routable_station",
        NO_ROUTABLE_STATION_HINT,
        message.as_str(),
    );

    log_request_with_debug(
        proxy.service_name,
        method.as_str(),
        path,
        status.as_u16(),
        duration_ms,
        None,
        EMPTY_TARGET_URL,
        None,
        EMPTY_TARGET_URL,
        session_id,
        None,
        None,
        ServiceTierLog::default(),
        None,
        None,
        http_debug,
    );

    (status, message)
}

pub(super) fn log_client_body_read_error(
    proxy: &ProxyService,
    method: &Method,
    path: &str,
    client_uri: &str,
    session_id: Option<String>,
    cwd: Option<String>,
    client_headers: Vec<HeaderEntry>,
    duration_ms: u64,
    error_message: String,
) -> (StatusCode, String) {
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
        proxy.service_name,
        method.as_str(),
        path,
        status.as_u16(),
        duration_ms,
        None,
        EMPTY_TARGET_URL,
        None,
        EMPTY_TARGET_URL,
        session_id,
        cwd,
        None,
        ServiceTierLog::default(),
        None,
        None,
        http_debug,
    );

    (status, error_message)
}

pub(super) async fn finish_failed_proxy_request(
    proxy: &ProxyService,
    method: &Method,
    path: &str,
    request_id: u64,
    status: StatusCode,
    message: String,
    duration_ms: u64,
    started_at_ms: u64,
    session_id: Option<String>,
    cwd: Option<String>,
    effective_effort: Option<String>,
    service_tier: ServiceTierLog,
    retry: Option<RetryInfo>,
) -> (StatusCode, String) {
    proxy
        .state
        .finish_request(FinishRequestParams {
            id: request_id,
            status_code: status.as_u16(),
            duration_ms,
            ended_at_ms: started_at_ms + duration_ms,
            observed_service_tier: None,
            usage: None,
            retry: retry.clone(),
            ttfb_ms: None,
        })
        .await;

    log_request_with_debug(
        proxy.service_name,
        method.as_str(),
        path,
        status.as_u16(),
        duration_ms,
        None,
        EMPTY_TARGET_URL,
        None,
        EMPTY_TARGET_URL,
        session_id,
        cwd,
        effective_effort,
        service_tier,
        None,
        retry,
        None,
    );

    (status, message)
}

pub(super) fn no_upstreams_available_error() -> (StatusCode, String) {
    (
        StatusCode::BAD_GATEWAY,
        "no upstreams available".to_string(),
    )
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
