use std::sync::OnceLock;
use std::time::Instant;

use axum::body::{Body, Bytes, to_bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, Uri};

use crate::logging::{BodyPreview, HeaderEntry, ServiceTierLog};
use crate::state::{SessionBinding, SessionIdentitySource};

use super::ProxyService;
use super::client_identity::{
    extract_client_addr, extract_client_name, extract_session_identity_with_body_fallback,
};
use super::request_body::{
    ReasoningOrchestrationIntent, RequestDialect, codex_session_identity_and_completed_body,
};
use super::request_encoding::normalize_request_content_encoding;
use super::request_failures::{
    ClientBodyReadErrorParams, FailedProxyRequestParams, client_body_read_error,
    finish_failed_proxy_request,
};
use super::request_preparation::{
    CommonRequestPreparationError, CommonRequestPreparationParams, RequestFlavor,
    codex_path_is_responses_or_compact, detect_request_flavor, load_request_config_context,
    prepare_common_request,
};
use super::response_semantics::ResponseSemanticContract;
use super::retry::RetryPlan;
use super::runtime_config::CapturedRoutePlan;

const MAX_PROXY_REQUEST_BYTES: usize = 64 * 1024 * 1024;

pub(super) struct PreparedProxyRequest {
    pub(super) method: Method,
    pub(super) uri: Uri,
    pub(super) client_uri: String,
    pub(super) client_headers: HeaderMap,
    pub(super) client_headers_entries_cache: OnceLock<Vec<HeaderEntry>>,
    pub(super) session_id: Option<String>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) session_binding: Option<SessionBinding>,
    pub(super) route_plan: CapturedRoutePlan,
    pub(super) cwd: Option<String>,
    pub(super) body_for_upstream: Bytes,
    pub(super) request_dialect: RequestDialect,
    pub(super) request_model: Option<String>,
    pub(super) effective_effort: Option<String>,
    pub(super) deferred_reasoning_intent: Option<ReasoningOrchestrationIntent>,
    pub(super) effective_service_tier: Option<String>,
    pub(super) base_service_tier: ServiceTierLog,
    pub(super) request_body_len: usize,
    pub(super) request_flavor: RequestFlavor,
    pub(super) request_body_previews: bool,
    pub(super) response_semantic_contract: Option<ResponseSemanticContract>,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
    pub(super) client_body_debug: Option<BodyPreview>,
    pub(super) client_body_warn: Option<BodyPreview>,
    pub(super) request_id: u64,
    pub(super) plan: RetryPlan,
    pub(super) cooldown_backoff: crate::endpoint_health::CooldownBackoff,
}

pub(super) async fn prepare_proxy_request(
    proxy: &ProxyService,
    req: Request<Body>,
    start: &Instant,
    started_at_ms: u64,
) -> Result<PreparedProxyRequest, (StatusCode, String)> {
    let (parts, body) = req.into_parts();
    let client_addr = extract_client_addr(&parts.extensions);
    let response_semantic_contract = parts.extensions.get::<ResponseSemanticContract>().copied();
    let uri = parts.uri;
    let client_uri = uri.to_string();
    let method = parts.method;
    let mut client_headers = parts.headers;
    let client_headers_entries_cache: OnceLock<Vec<HeaderEntry>> = OnceLock::new();

    let client_name = extract_client_name(&client_headers);

    let request_flavor =
        detect_request_flavor(proxy.service_name, &method, &client_headers, uri.path());
    let raw_body = match to_bytes(body, MAX_PROXY_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(error) => {
            let dur = start.elapsed().as_millis() as u64;
            return Err(client_body_read_error(ClientBodyReadErrorParams {
                proxy,
                method: &method,
                path: uri.path(),
                duration_ms: dur,
                error_message: error.to_string(),
            }));
        }
    };
    let raw_body = match normalize_request_content_encoding(&mut client_headers, raw_body) {
        Ok(body) => body,
        Err(error) => {
            let dur = start.elapsed().as_millis() as u64;
            return Err(client_body_read_error(ClientBodyReadErrorParams {
                proxy,
                method: &method,
                path: uri.path(),
                duration_ms: dur,
                error_message: error.to_string(),
            }));
        }
    };
    let (session_identity_hint, raw_body) = if request_flavor.is_codex_service
        && method == Method::POST
        && codex_path_is_responses_or_compact(uri.path())
    {
        codex_session_identity_and_completed_body(&mut client_headers, &raw_body)
    } else {
        (
            extract_session_identity_with_body_fallback(&client_headers, raw_body.as_ref()),
            raw_body,
        )
    };
    let request_flavor = request_flavor
        .with_remote_compaction_context_from_body(raw_body.as_ref())
        .with_responses_stream_from_body(raw_body.as_ref());
    let config = load_request_config_context(proxy, session_identity_hint.as_ref()).await;
    let request_body_previews = crate::logging::should_log_request_body_preview();
    let prepared = match prepare_common_request(CommonRequestPreparationParams {
        proxy,
        config: &config,
        method: &method,
        uri: &uri,
        client_headers: &client_headers,
        raw_body: &raw_body,
        request_dialect: RequestDialect::from_http_path(uri.path()),
        client_name,
        client_addr,
        started_at_ms,
        client_content_type: request_flavor.client_content_type.as_deref(),
        request_body_previews,
    })
    .await
    {
        Ok(prepared) => prepared,
        Err(CommonRequestPreparationError::LifecycleStoreUnavailable { message }) => {
            tracing::error!(error = %message, "failed to begin durable logical request");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to begin durable request lifecycle".to_string(),
            ));
        }
        Err(CommonRequestPreparationError::NoRoutableCandidate {
            request_id,
            session_id,
            session_identity_source,
            cwd,
            effective_effort,
            service_tier,
        }) => {
            let dur = start.elapsed().as_millis() as u64;
            return Err(finish_failed_proxy_request(FailedProxyRequestParams {
                proxy,
                method: &method,
                path: uri.path(),
                request_id,
                status: StatusCode::BAD_GATEWAY,
                message: "no routable provider candidate".to_string(),
                duration_ms: dur,
                started_at_ms,
                session_id: session_id.clone(),
                session_identity_source,
                cwd,
                effective_effort,
                service_tier,
                codex_bridge: request_flavor.codex_bridge_log.clone(),
                retry: None,
                failure_route_attempts: Vec::new(),
            })
            .await);
        }
    };

    let Some(route_plan) = prepared.route_plan else {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "request route plan was not prepared".to_string(),
        ));
    };

    Ok(PreparedProxyRequest {
        method,
        uri,
        client_uri,
        client_headers,
        client_headers_entries_cache,
        session_id: prepared.session_id,
        session_identity_source: prepared.session_identity_source,
        session_binding: prepared.session_binding,
        route_plan,
        cwd: prepared.cwd,
        body_for_upstream: prepared.body_for_upstream,
        request_dialect: prepared.request_dialect,
        request_model: prepared.request_model,
        effective_effort: prepared.effective_effort,
        deferred_reasoning_intent: prepared.deferred_reasoning_intent,
        effective_service_tier: prepared.effective_service_tier,
        base_service_tier: prepared.base_service_tier,
        request_body_len: prepared.request_body_len,
        request_flavor,
        request_body_previews: prepared.request_body_previews,
        response_semantic_contract,
        debug_max: prepared.debug_max,
        warn_max: prepared.warn_max,
        client_body_debug: prepared.client_body_debug,
        client_body_warn: prepared.client_body_warn,
        request_id: prepared.request_id,
        plan: prepared.plan,
        cooldown_backoff: prepared.cooldown_backoff,
    })
}
