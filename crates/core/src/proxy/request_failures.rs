use axum::http::{Method, StatusCode};

use crate::logging::{CodexBridgeLog, HttpDebugLog, RetryInfo, RouteAttemptLog, ServiceTierLog};
use crate::runtime_store::RequestAccountingScope;
use crate::state::SessionIdentitySource;

use super::ProxyService;
use super::failure_summary::failed_proxy_client_message;
use super::request_observer::{RequestObserver, RequestPublication};

pub(super) struct ClientBodyReadErrorParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) method: &'a Method,
    pub(super) path: &'a str,
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
    pub(super) http_debug: Option<HttpDebugLog>,
    pub(super) failure_route_attempts: Vec<RouteAttemptLog>,
}

pub(super) fn client_body_read_error(
    params: ClientBodyReadErrorParams<'_>,
) -> (StatusCode, String) {
    let ClientBodyReadErrorParams {
        proxy,
        method,
        path,
        duration_ms,
        error_message,
    } = params;

    tracing::warn!(
        service = proxy.service_name,
        method = method.as_str(),
        path,
        duration_ms,
        error = %error_message,
        "client request body read failed before durable lifecycle creation"
    );
    (StatusCode::BAD_REQUEST, error_message)
}

pub(super) async fn finish_failed_proxy_request(
    params: FailedProxyRequestParams<'_>,
) -> (StatusCode, String) {
    let (status, message, _) =
        finish_failed_proxy_request_inner(params, RequestAccountingScope::Economic).await;
    (status, message)
}

pub(super) async fn finish_non_economic_failed_proxy_request(
    params: FailedProxyRequestParams<'_>,
) -> (StatusCode, String) {
    let (status, message, _) =
        finish_failed_proxy_request_inner(params, RequestAccountingScope::NonEconomic).await;
    (status, message)
}

pub(super) async fn finish_failed_proxy_request_with_accounting(
    params: FailedProxyRequestParams<'_>,
    accounting: RequestAccountingScope,
) -> (StatusCode, String) {
    let (status, message, _) = finish_failed_proxy_request_inner(params, accounting).await;
    (status, message)
}

pub(super) async fn finish_failed_proxy_request_with_publication_result(
    params: FailedProxyRequestParams<'_>,
) -> (StatusCode, String, bool) {
    finish_failed_proxy_request_inner(params, RequestAccountingScope::Economic).await
}

pub(super) async fn finish_non_economic_failed_proxy_request_with_publication_result(
    params: FailedProxyRequestParams<'_>,
) -> (StatusCode, String, bool) {
    finish_failed_proxy_request_inner(params, RequestAccountingScope::NonEconomic).await
}

async fn finish_failed_proxy_request_inner(
    params: FailedProxyRequestParams<'_>,
    accounting: RequestAccountingScope,
) -> (StatusCode, String, bool) {
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
        http_debug,
        failure_route_attempts,
    } = params;
    let client_message = failed_proxy_client_message(
        status,
        message.as_str(),
        request_id,
        retry.as_ref(),
        &failure_route_attempts,
    );

    let mut publication = RequestPublication::failure_without_upstream(
        request_id,
        status.as_u16(),
        duration_ms,
        started_at_ms,
    );
    publication.session_id = session_id;
    publication.session_identity_source = session_identity_source;
    publication.cwd = cwd;
    publication.reasoning_effort = effective_effort;
    publication.service_tier = service_tier;
    publication.codex_bridge = codex_bridge;
    publication.retry = retry;
    publication.http_debug = http_debug;
    let observer = RequestObserver::new(proxy, method, path);
    let published = observer
        .publish_terminal_with_accounting(publication, accounting)
        .await;

    (status, client_message, published)
}
