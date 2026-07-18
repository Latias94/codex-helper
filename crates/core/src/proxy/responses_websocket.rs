use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{
    CloseFrame as AxumCloseFrame, Message as AxumWsMessage, Utf8Bytes, WebSocket, WebSocketUpgrade,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;

use crate::auth_resolution::UpstreamAuthResolutionError;
use crate::credentials::CredentialGenerationMarker;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http as tungstenite_http;

use crate::logging::{CodexBridgeLog, RouteAttemptLog, ServiceTierLog};
use crate::provider_catalog::AccountFingerprint;
use crate::routing_ir::{
    RouteCandidate, RoutePlanAttemptState, RoutePlanExecutor, RoutePlanRuntimeState,
    RoutePlanTemplate,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::runtime_store::{AttemptHandle, AttemptOutcome, AttemptRouteEvidence, EconomicsState};
use crate::state::{
    AttemptProviderScopeCapture, CapturedUpstreamAttemptContext, ResolvedRouteValue,
    RouteDecisionProvenance, RouteValueSource, SessionIdentitySource,
};

use super::attempt_failures::{TerminalUpstreamFailureParams, apply_terminal_upstream_failure};
use super::attempt_health::record_attempt_success;
use super::attempt_request::inject_auth_headers;
use super::classify::{classify_observed_upstream_response, is_credential_auth_failure};
use super::client_identity::extract_session_identity;
use super::codex_failure::CodexFailureKind;
use super::concurrency_limits::{
    ConcurrencyAcquireError, ConcurrencyLimit, ConcurrencyPermit, ConcurrencyWaitPolicy,
};
use super::request_body::{
    ReasoningOrchestrationIntent, RequestDialect, apply_deferred_reasoning_intent,
    codex_session_identity_and_completed_body, extract_model_from_response_body,
    extract_service_tier_from_response_body,
};
use super::request_continuity::{
    RequestContinuityClassification, RequestContinuityClassificationInput,
    RequestContinuityContract, RequestTransport, RouteContinuityDecisionInput,
    classify_request_continuity,
};
use super::request_failures::{
    FailedProxyRequestParams, finish_failed_proxy_request,
    finish_failed_proxy_request_with_publication_result, finish_non_economic_failed_proxy_request,
    finish_non_economic_failed_proxy_request_with_publication_result,
};
use super::request_observer::{RequestObserver, RequestPublication};
use super::request_preparation::{
    CommonRequestPreparationError, CommonRequestPreparationParams, load_request_config_context,
    prepare_common_request,
};
use super::retry::{RetryPlan, retry_info_for_failed_attempts, retry_info_for_observed_attempts};
use super::route_affinity::{
    SessionRouteReservationDecision, apply_session_route_reservation_to_runtime,
    claim_session_route_reservation, lock_session_route_reservation_selection,
    prepare_session_route_affinity_success,
};
use super::route_attempts::{
    ErrorRouteAttemptParams, RouteAttemptErrorKind, StartRouteAttemptParams,
    StatusRouteAttemptParams, record_error_route_attempt, record_status_route_attempt,
    start_selected_route_attempt,
};
use super::route_target_selection::{
    acquire_candidate_concurrency_permit, apply_auth_resolution_to_runtime,
    apply_routing_operator_control_to_runtime, restrict_route_state_to_affinity_continuity_domain,
    route_graph_request_requires_existing_affinity, route_graph_runtime_for_request,
    runtime_for_acquired_candidate_revalidation, runtime_for_capacity_wait_selection,
    select_route_graph_candidate,
};
use super::runtime_config::{CapturedRoutePlan, RuntimeSnapshot};
use super::selected_upstream_request::apply_selected_model_mapping;
use super::{CLIENT_NAME_HEADER, ProxyService};
use crate::routing_ir::CapturedRouteCandidate;

const RESPONSES_WS_BETA_HEADER: &str = "responses_websockets=2026-02-06";
const WS_PROVIDER_ENDPOINT_HEADER: &str = "x-codex-helper-provider-endpoint";
const UPSTREAM_AUTH_UNAVAILABLE_REASON: &str = "configured upstream credentials are unavailable";
const WS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const WS_CONNECTIONS_PER_REQUEST_SLOT: u32 = 2;

type UpstreamWebSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct ResponsesWebSocketPrepared {
    method: Method,
    uri: Uri,
    session_id: Option<String>,
    session_identity_source: Option<SessionIdentitySource>,
    cwd: Option<String>,
    request_id: u64,
    started_at_ms: u64,
    start: Instant,
    first_message: AxumWsMessage,
    body_for_upstream: axum::body::Bytes,
    request_dialect: RequestDialect,
    request_model: Option<String>,
    effective_effort: Option<String>,
    deferred_reasoning_intent: Option<ReasoningOrchestrationIntent>,
    base_service_tier: ServiceTierLog,
    request_continuity: RequestContinuityClassification,
    is_warmup: bool,
    plan: RetryPlan,
    cooldown_backoff: crate::endpoint_health::CooldownBackoff,
    route_plan: CapturedRoutePlan,
}

struct ResponsesWebSocketSelected {
    target: CapturedRouteCandidate,
    attempt_context: CapturedUpstreamAttemptContext,
    upstream_first_message: AxumWsMessage,
    provider_id: Option<String>,
    model_note: String,
    effective_effort: Option<String>,
    route_decision: RouteDecisionProvenance,
    route_attempts: Vec<RouteAttemptLog>,
    route_attempt_index: usize,
    route_graph_key: Option<String>,
}

struct ResponsesWebSocketHandshakeRoute {
    runtime_snapshot: Arc<RuntimeSnapshot>,
    route_revision: u64,
    handshake_credential_generation: CredentialGenerationMarker,
    route_template: Arc<RoutePlanTemplate>,
    binding_source: WebSocketHandshakeBindingSource,
    routing_control_graph_key: String,
    target: CapturedRouteCandidate,
    route_graph_key: String,
    avoided_indices: Vec<usize>,
    avoided_total: usize,
    total_upstreams: usize,
}

struct ResponsesWebSocketUpstream {
    route: ResponsesWebSocketHandshakeRoute,
    socket: UpstreamWebSocket,
    status_code: u16,
    headers_ms: u64,
    attempt_scope: ResponsesWebSocketAttemptScope,
    _connection_permit: Option<ConcurrencyPermit>,
}

#[derive(Clone)]
struct ResponsesWebSocketAttemptScope {
    endpoint: reqwest::Url,
    account_fingerprint: AccountFingerprint,
}

type ConcurrencyAdmissionFuture =
    BoxFuture<'static, Result<Option<ConcurrencyPermit>, ConcurrencyAcquireError>>;

struct PendingResponsesWebSocketCreate {
    prepared: ResponsesWebSocketPrepared,
    admission: ConcurrencyAdmissionFuture,
}

struct ActiveResponsesWebSocketCreate {
    prepared: ResponsesWebSocketPrepared,
    selected: ResponsesWebSocketSelected,
    attempt_handle: AttemptHandle,
    _concurrency_permit: Option<ConcurrencyPermit>,
}

struct PrepareResponsesWebSocketParams {
    uri: Uri,
    client_headers: HeaderMap,
    first_message: AxumWsMessage,
    start: Instant,
    started_at_ms: u64,
}

struct ResponsesWebSocketSelectionFailure {
    status: StatusCode,
    message: String,
    route_attempts: Vec<RouteAttemptLog>,
}

impl ResponsesWebSocketSelectionFailure {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            route_attempts: Vec::new(),
        }
    }
}

pub(super) async fn handle_responses_websocket(
    proxy: ProxyService,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    headers: HeaderMap,
    uri: Uri,
) -> impl IntoResponse {
    if proxy.service_name != "codex" {
        return (
            StatusCode::NOT_FOUND,
            "Responses WebSocket is only supported for Codex",
        )
            .into_response();
    }

    let ws = match ws {
        Ok(ws) => ws,
        Err(_) => {
            return (
                StatusCode::UPGRADE_REQUIRED,
                "WebSocket upgrade required (Upgrade: websocket)",
            )
                .into_response();
        }
    };

    let runtime_snapshot = proxy.config.capture().await;
    let route =
        match prepare_responses_websocket_handshake_route(&proxy, runtime_snapshot, &headers).await
        {
            Ok(route) => route,
            Err(failure) => return (failure.status, failure.message).into_response(),
        };
    let target_url = match proxy.build_target(&route.target, &uri) {
        Ok(url) => url,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("invalid upstream websocket target: {error}"),
            )
                .into_response();
        }
    };
    let upstream_headers =
        match upstream_ws_handshake_headers(&headers, &route.target, target_url.as_str()) {
            Ok(headers) => headers,
            Err(error) => {
                tracing::warn!(
                    provider_id = route.target.provider_id(),
                    auth_error_code = error.code(),
                    error = %error,
                    "selected WebSocket provider authentication could not be resolved"
                );
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    UPSTREAM_AUTH_UNAVAILABLE_REASON,
                )
                    .into_response();
            }
        };
    let upstream_url = match http_url_to_ws(target_url.clone()) {
        Ok(url) => url,
        Err(message) => return (StatusCode::BAD_GATEWAY, message).into_response(),
    };
    let upstream_request = match upstream_ws_request(upstream_url.as_str(), &upstream_headers) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("invalid upstream websocket request: {error}"),
            )
                .into_response();
        }
    };
    let connection_permit = match acquire_websocket_connection_permit(&proxy, &route).await {
        Ok(permit) => permit,
        Err(error) => {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                format!("WebSocket connection capacity unavailable: {error:?}"),
            )
                .into_response();
        }
    };

    let upstream_start = Instant::now();
    let (upstream_socket, upstream_response) =
        match tokio::time::timeout(WS_HANDSHAKE_TIMEOUT, connect_async(upstream_request)).await {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                if let tungstenite::Error::Http(response) = &error {
                    let classification = classify_observed_upstream_response(
                        response.status().as_u16(),
                        response.headers(),
                        response.body().as_deref().unwrap_or_default(),
                    );
                    if is_credential_auth_failure(
                        response.status(),
                        classification.class.as_deref(),
                    ) {
                        proxy
                            .config
                            .schedule_credential_refresh(route.target.credential());
                    }
                }
                return upstream_ws_handshake_error_response(error);
            }
            Err(_) => {
                return (
                    StatusCode::GATEWAY_TIMEOUT,
                    "upstream WebSocket handshake timed out",
                )
                    .into_response();
            }
        };
    let upstream_headers_ms = upstream_start.elapsed().as_millis() as u64;
    let upstream_status_code = upstream_response.status().as_u16();
    let selected_protocol = upstream_response
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let success_headers = filter_upstream_ws_success_headers(upstream_response.headers());
    let route_revision = route.route_revision;
    let account_fingerprint = proxy.state.derive_provider_account_fingerprint(
        route.target.runtime_identity().credential_scope.as_deref(),
        &upstream_headers,
    );
    let upstream = ResponsesWebSocketUpstream {
        route,
        socket: upstream_socket,
        status_code: upstream_status_code,
        headers_ms: upstream_headers_ms,
        attempt_scope: ResponsesWebSocketAttemptScope {
            endpoint: target_url,
            account_fingerprint,
        },
        _connection_permit: connection_permit,
    };

    tracing::debug!(
        service = proxy.service_name,
        route_revision,
        "upstream WebSocket handshake completed before downstream upgrade"
    );
    let ws = if let Some(protocol) = selected_protocol {
        ws.protocols([protocol])
    } else {
        ws
    };
    let mut response = ws.on_upgrade(move |socket| async move {
        serve_responses_websocket(proxy, socket, headers, uri, upstream).await;
    });
    for (name, value) in success_headers {
        if let Some(name) = name {
            response.headers_mut().append(name, value);
        }
    }
    response
}

async fn serve_responses_websocket(
    proxy: ProxyService,
    client_socket: WebSocket,
    client_headers: HeaderMap,
    uri: Uri,
    upstream: ResponsesWebSocketUpstream,
) {
    relay_websocket_streams(proxy, client_headers, uri, client_socket, upstream).await;
}

async fn prepare_responses_websocket(
    proxy: &ProxyService,
    params: PrepareResponsesWebSocketParams,
) -> Result<ResponsesWebSocketPrepared, (StatusCode, String)> {
    let PrepareResponsesWebSocketParams {
        uri,
        client_headers,
        first_message,
        start,
        started_at_ms,
    } = params;
    let method = Method::GET;
    let mut client_headers = client_headers;
    let client_name = client_headers
        .get(CLIENT_NAME_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let client_addr = None;

    let raw_body = first_message_body_bytes(&first_message).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "missing first response.create message".to_string(),
        )
    })?;
    let value = serde_json::from_slice::<serde_json::Value>(raw_body.as_ref()).ok();
    if value
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(serde_json::Value::as_str)
        != Some("response.create")
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "first WebSocket data message must be response.create".to_string(),
        ));
    }
    let is_warmup = value
        .as_ref()
        .and_then(|value| value.get("generate"))
        .and_then(serde_json::Value::as_bool)
        == Some(false);
    let (session_identity_hint, raw_body) =
        codex_session_identity_and_completed_body(&mut client_headers, &raw_body);
    let config = load_request_config_context(proxy, session_identity_hint.as_ref()).await;
    let request_continuity = classify_request_continuity(RequestContinuityClassificationInput {
        transport: RequestTransport::ResponsesWebSocket,
        is_codex_service: proxy.service_name == "codex",
        is_user_turn: true,
        is_remote_compaction_v1_request: false,
        raw_body: raw_body.as_ref(),
    });

    let prepared = match prepare_common_request(CommonRequestPreparationParams {
        proxy,
        config: &config,
        method: &method,
        uri: &uri,
        client_headers: &client_headers,
        raw_body: &raw_body,
        request_dialect: RequestDialect::ResponsesWebSocket,
        client_name,
        client_addr,
        started_at_ms,
        client_content_type: client_headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        request_body_previews: false,
    })
    .await
    {
        Ok(prepared) => prepared,
        Err(CommonRequestPreparationError::LifecycleStoreUnavailable { message }) => {
            tracing::error!(error = %message, "failed to begin durable WebSocket request");
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
            let message = "no upstreams available".to_string();
            let _ = finish_failed_proxy_request(FailedProxyRequestParams {
                proxy,
                method: &method,
                path: uri.path(),
                request_id,
                status: StatusCode::BAD_GATEWAY,
                message: message.clone(),
                duration_ms: start.elapsed().as_millis() as u64,
                started_at_ms,
                session_id,
                session_identity_source,
                cwd,
                effective_effort,
                service_tier,
                codex_bridge: None,
                retry: None,
                failure_route_attempts: Vec::new(),
            })
            .await;
            return Err((StatusCode::BAD_GATEWAY, message));
        }
    };
    let route_plan = prepared.route_plan.ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "WebSocket request route plan was not prepared".to_string(),
        )
    })?;

    Ok(ResponsesWebSocketPrepared {
        method,
        uri,
        session_id: prepared.session_id,
        session_identity_source: prepared.session_identity_source,
        cwd: prepared.cwd,
        request_id: prepared.request_id,
        started_at_ms,
        start,
        first_message,
        body_for_upstream: prepared.body_for_upstream,
        request_dialect: prepared.request_dialect,
        request_model: prepared.request_model,
        effective_effort: prepared.effective_effort,
        deferred_reasoning_intent: prepared.deferred_reasoning_intent,
        base_service_tier: prepared.base_service_tier,
        request_continuity,
        is_warmup,
        plan: prepared.plan,
        cooldown_backoff: prepared.cooldown_backoff,
        route_plan,
    })
}

async fn prepare_responses_websocket_handshake_route(
    proxy: &ProxyService,
    runtime_snapshot: Arc<RuntimeSnapshot>,
    client_headers: &HeaderMap,
) -> Result<ResponsesWebSocketHandshakeRoute, ResponsesWebSocketSelectionFailure> {
    let session_identity = extract_session_identity(client_headers);
    let session_id = session_identity
        .as_ref()
        .map(|identity| identity.value().to_string());
    let provider_policy = runtime_snapshot.provider_policy();
    let graph = runtime_snapshot
        .route_graph(proxy.service_name)
        .ok_or_else(|| {
            ResponsesWebSocketSelectionFailure::new(
                StatusCode::BAD_GATEWAY,
                "captured WebSocket route graph has an unknown service",
            )
        })?;
    let routing_control_graph_key = graph.digest().to_string();
    let template = graph.handshake_plan();
    let runtime_identities = template.candidate_identities().map_err(|error| {
        ResponsesWebSocketSelectionFailure::new(
            StatusCode::BAD_GATEWAY,
            format!("captured WebSocket credential binding is invalid: {error}"),
        )
    })?;
    let mut runtime = proxy
        .state
        .route_plan_runtime_state_with_provider_policy(
            proxy.service_name,
            provider_policy.as_ref(),
            runtime_snapshot.revision(),
            runtime_identities.as_slice(),
        )
        .await;
    apply_auth_resolution_to_runtime(proxy.service_name, &template, &mut runtime).map_err(
        |error| {
            ResponsesWebSocketSelectionFailure::new(
                StatusCode::BAD_GATEWAY,
                format!("captured WebSocket credential binding is invalid: {error}"),
            )
        },
    )?;
    apply_routing_operator_control_to_runtime(
        proxy,
        routing_control_graph_key.as_str(),
        &mut runtime,
    )
    .await;

    let route_graph_key = template.route_graph_key();
    let affinity = if let Some(session_id) = session_id.as_deref() {
        proxy
            .state
            .peek_session_route_affinity(session_id)
            .await
            .filter(|affinity| {
                affinity.route_graph_key == route_graph_key
                    && template.contains_provider_endpoint(
                        &affinity.provider_endpoint,
                        affinity.upstream_base_url.as_str(),
                    )
            })
    } else {
        None
    };
    let candidates = unique_handshake_candidates(&template);
    let explicit_endpoint = explicit_ws_provider_endpoint(client_headers)?;
    let selection_hint = if let Some(explicit) = explicit_endpoint.as_deref() {
        let candidate = candidates
            .iter()
            .find(|(key, candidate)| {
                key.as_str() == explicit
                    || format!("{}/{}", candidate.provider_id, candidate.endpoint_id) == explicit
            })
            .map(|(key, _)| key.clone())
            .ok_or_else(|| {
                ResponsesWebSocketSelectionFailure::new(
                    StatusCode::BAD_REQUEST,
                    format!("unknown {WS_PROVIDER_ENDPOINT_HEADER} value"),
                )
            })?;
        if affinity
            .as_ref()
            .is_some_and(|affinity| affinity.provider_endpoint.stable_key() != candidate)
        {
            return Err(ResponsesWebSocketSelectionFailure::new(
                StatusCode::CONFLICT,
                "explicit WebSocket endpoint conflicts with session route affinity",
            ));
        }
        HandshakeSelectionHint::Explicit(candidate)
    } else if let Some(affinity) = affinity.as_ref() {
        HandshakeSelectionHint::Affinity(affinity.provider_endpoint.clone())
    } else if let Some(preferred) = runtime
        .new_session_preference()
        .filter(|preferred| {
            candidates
                .iter()
                .any(|(key, _)| key == &preferred.stable_key())
        })
        .cloned()
    {
        HandshakeSelectionHint::Preference(preferred)
    } else if candidates.len() == 1 {
        HandshakeSelectionHint::Singleton(candidates[0].0.clone())
    } else {
        return Err(ResponsesWebSocketSelectionFailure::new(
            StatusCode::UPGRADE_REQUIRED,
            format!(
                "Responses WebSocket route is ambiguous before upgrade; provide session affinity or {WS_PROVIDER_ENDPOINT_HEADER}"
            ),
        ));
    };

    if let Some(affinity) = affinity.as_ref() {
        runtime.set_affinity_provider_endpoint_with_observed_at(
            Some(affinity.provider_endpoint.clone()),
            Some(affinity.last_selected_at_ms),
            Some(affinity.last_changed_at_ms),
        );
    }
    select_captured_handshake_route(
        runtime_snapshot,
        template,
        runtime,
        routing_control_graph_key,
        route_graph_key,
        selection_hint,
        affinity
            .as_ref()
            .map(|affinity| &affinity.provider_endpoint),
    )
}

enum HandshakeSelectionHint {
    Explicit(String),
    Affinity(ProviderEndpointKey),
    Preference(ProviderEndpointKey),
    Singleton(String),
}

#[derive(Clone, Copy)]
enum WebSocketHandshakeBindingSource {
    Explicit,
    Affinity,
    Preference,
    Singleton,
}

fn unique_handshake_candidates(template: &RoutePlanTemplate) -> Vec<(String, &RouteCandidate)> {
    let mut candidates = BTreeMap::new();
    for candidate in &template.candidates {
        candidates
            .entry(
                template
                    .candidate_provider_endpoint_key(candidate)
                    .stable_key(),
            )
            .or_insert(candidate);
    }
    candidates.into_iter().collect()
}

fn explicit_ws_provider_endpoint(
    headers: &HeaderMap,
) -> Result<Option<String>, ResponsesWebSocketSelectionFailure> {
    let Some(value) = headers.get(WS_PROVIDER_ENDPOINT_HEADER) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| {
        ResponsesWebSocketSelectionFailure::new(
            StatusCode::BAD_REQUEST,
            format!("invalid {WS_PROVIDER_ENDPOINT_HEADER} header"),
        )
    })?;
    let value = value.trim();
    if value.is_empty() {
        return Err(ResponsesWebSocketSelectionFailure::new(
            StatusCode::BAD_REQUEST,
            format!("empty {WS_PROVIDER_ENDPOINT_HEADER} header"),
        ));
    }
    Ok(Some(value.to_string()))
}

async fn acquire_websocket_connection_permit(
    proxy: &ProxyService,
    route: &ResponsesWebSocketHandshakeRoute,
) -> Result<Option<ConcurrencyPermit>, ConcurrencyAcquireError> {
    let candidate = route.target.candidate();
    let Some(limit) = candidate.concurrency.max_concurrent_requests else {
        return Ok(None);
    };
    let connection_limit = limit.saturating_mul(WS_CONNECTIONS_PER_REQUEST_SLOT);
    let Some(limit) = ConcurrencyLimit::new(connection_limit, route.route_revision) else {
        return Ok(None);
    };
    let provider_endpoint = route
        .route_template
        .candidate_provider_endpoint_key(candidate);
    let Some(request_limit_key) = candidate
        .concurrency
        .limit_key(proxy.service_name, &provider_endpoint)
    else {
        return Ok(None);
    };

    proxy
        .concurrency_limiter
        .acquire(
            format!("websocket-connection:{request_limit_key}"),
            limit,
            None,
            ConcurrencyWaitPolicy::new(Duration::ZERO, 0),
        )
        .await
        .map(Some)
}

fn compatible_websocket_route(
    proxy: &ProxyService,
    bound: &ResponsesWebSocketHandshakeRoute,
    current_snapshot: Arc<RuntimeSnapshot>,
    client_headers: &HeaderMap,
    uri: &Uri,
    attempt_scope: &ResponsesWebSocketAttemptScope,
) -> Option<ResponsesWebSocketHandshakeRoute> {
    let graph = current_snapshot.route_graph(proxy.service_name)?;
    let routing_control_graph_key = graph.digest().to_string();
    if routing_control_graph_key != bound.routing_control_graph_key {
        return None;
    }
    compatible_websocket_route_with_template(
        proxy,
        bound,
        current_snapshot,
        Arc::clone(&bound.route_template),
        routing_control_graph_key,
        client_headers,
        uri,
        attempt_scope,
    )
}

fn compatible_websocket_route_for_plan(
    proxy: &ProxyService,
    bound: &ResponsesWebSocketHandshakeRoute,
    route_plan: &CapturedRoutePlan,
    client_headers: &HeaderMap,
    uri: &Uri,
    attempt_scope: &ResponsesWebSocketAttemptScope,
) -> Option<ResponsesWebSocketHandshakeRoute> {
    let runtime_snapshot = route_plan.runtime_snapshot();
    let routing_control_graph_key = route_plan.routing_control_graph_key().to_string();
    if routing_control_graph_key != bound.routing_control_graph_key {
        return None;
    }
    compatible_websocket_route_with_template(
        proxy,
        bound,
        runtime_snapshot,
        Arc::new(route_plan.template().clone()),
        routing_control_graph_key,
        client_headers,
        uri,
        attempt_scope,
    )
}

#[allow(clippy::too_many_arguments)]
fn compatible_websocket_route_with_template(
    proxy: &ProxyService,
    bound: &ResponsesWebSocketHandshakeRoute,
    current_snapshot: Arc<RuntimeSnapshot>,
    template: Arc<RoutePlanTemplate>,
    routing_control_graph_key: String,
    client_headers: &HeaderMap,
    uri: &Uri,
    attempt_scope: &ResponsesWebSocketAttemptScope,
) -> Option<ResponsesWebSocketHandshakeRoute> {
    if current_snapshot.credential_generation().marker() != bound.handshake_credential_generation {
        return None;
    }
    let current_graph = current_snapshot.route_graph(proxy.service_name)?;
    if current_graph.digest() != routing_control_graph_key
        || current_graph.handshake_plan().scheduling_preset != template.scheduling_preset
        || template.scheduling_preset != bound.route_template.scheduling_preset
        || current_snapshot.provider_catalog().catalog_revision()
            != bound.runtime_snapshot.provider_catalog().catalog_revision()
    {
        return None;
    }
    let route_graph_key = template.route_graph_key();
    let topology = template.continuity_topology();
    let candidate =
        topology.find_candidate_by_provider_endpoint(bound.target.provider_endpoint())?;
    let target = template.capture_candidate(candidate).ok()?;
    if target.continuity_domain() != bound.target.continuity_domain() {
        return None;
    }

    let target_url = proxy.build_target(&target, uri).ok()?;
    if target_url != attempt_scope.endpoint {
        return None;
    }
    let upstream_headers =
        upstream_ws_handshake_headers(client_headers, &target, target_url.as_str()).ok()?;
    if proxy.state.derive_provider_account_fingerprint(
        target.runtime_identity().credential_scope.as_deref(),
        &upstream_headers,
    ) != attempt_scope.account_fingerprint
    {
        return None;
    }

    Some(ResponsesWebSocketHandshakeRoute {
        route_revision: current_snapshot.revision(),
        runtime_snapshot: current_snapshot,
        handshake_credential_generation: bound.handshake_credential_generation.clone(),
        route_template: template,
        binding_source: bound.binding_source,
        routing_control_graph_key,
        target,
        route_graph_key,
        avoided_indices: bound.avoided_indices.clone(),
        avoided_total: bound.avoided_total,
        total_upstreams: bound.total_upstreams,
    })
}

#[allow(clippy::too_many_arguments)]
fn select_captured_handshake_route(
    runtime_snapshot: Arc<RuntimeSnapshot>,
    template: RoutePlanTemplate,
    runtime: RoutePlanRuntimeState,
    routing_control_graph_key: String,
    route_graph_key: String,
    selection_hint: HandshakeSelectionHint,
    affinity_endpoint: Option<&ProviderEndpointKey>,
) -> Result<ResponsesWebSocketHandshakeRoute, ResponsesWebSocketSelectionFailure> {
    let executor = RoutePlanExecutor::new(&template);
    let mut route_state = RoutePlanAttemptState::default();
    let selection = if affinity_endpoint.is_some() {
        executor.select_supported_candidate_with_soft_affinity_runtime_state(
            &mut route_state,
            &runtime,
            None,
        )
    } else {
        executor.select_supported_candidate_with_runtime_state(&mut route_state, &runtime, None)
    };
    let selected = selection.selected.ok_or_else(|| {
        let all_candidates_missing_auth = !template.candidates.is_empty()
            && template.candidates.iter().all(|candidate| {
                runtime
                    .candidate_runtime_snapshot(&template, candidate)
                    .missing_auth
            });
        ResponsesWebSocketSelectionFailure::new(
            StatusCode::SERVICE_UNAVAILABLE,
            if all_candidates_missing_auth {
                UPSTREAM_AUTH_UNAVAILABLE_REASON
            } else {
                "captured WebSocket endpoint is not currently eligible"
            },
        )
    })?;
    let binding_source = match &selection_hint {
        HandshakeSelectionHint::Explicit(_) => WebSocketHandshakeBindingSource::Explicit,
        HandshakeSelectionHint::Affinity(_) => WebSocketHandshakeBindingSource::Affinity,
        HandshakeSelectionHint::Preference(_) => WebSocketHandshakeBindingSource::Preference,
        HandshakeSelectionHint::Singleton(_) => WebSocketHandshakeBindingSource::Singleton,
    };
    let selected_key = selected.provider_endpoint.stable_key();
    match &selection_hint {
        HandshakeSelectionHint::Explicit(expected)
        | HandshakeSelectionHint::Singleton(expected)
            if selected_key != *expected =>
        {
            return Err(ResponsesWebSocketSelectionFailure::new(
                StatusCode::CONFLICT,
                "captured WebSocket endpoint is not selected by current route policy",
            ));
        }
        HandshakeSelectionHint::Affinity(expected) if selected.provider_endpoint != *expected => {
            let topology = template.continuity_topology();
            let affinity_candidate = topology
                .find_candidate_by_provider_endpoint(expected)
                .ok_or_else(|| {
                    ResponsesWebSocketSelectionFailure::new(
                        StatusCode::UPGRADE_REQUIRED,
                        "session WebSocket affinity is no longer present in the captured route",
                    )
                })?;
            let affinity_domain = topology.candidate_domain(affinity_candidate);
            let selected_domain = topology.candidate_domain(selected.candidate);
            if !affinity_domain.is_explicit() || affinity_domain != selected_domain {
                return Err(ResponsesWebSocketSelectionFailure::new(
                    StatusCode::UPGRADE_REQUIRED,
                    "session WebSocket affinity cannot be resolved before upgrade",
                ));
            }
        }
        HandshakeSelectionHint::Preference(expected) if selected.provider_endpoint != *expected => {
            return Err(ResponsesWebSocketSelectionFailure::new(
                StatusCode::CONFLICT,
                "new-session WebSocket preference is not currently selectable",
            ));
        }
        _ => {}
    }

    let target = template
        .capture_candidate(selected.candidate)
        .map_err(|_| {
            ResponsesWebSocketSelectionFailure::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "captured WebSocket route has no matching credential binding",
            )
        })?;
    let handshake_credential_generation = runtime_snapshot.credential_generation().marker();
    Ok(ResponsesWebSocketHandshakeRoute {
        route_revision: runtime_snapshot.revision(),
        runtime_snapshot,
        handshake_credential_generation,
        route_template: Arc::new(template.clone()),
        binding_source,
        routing_control_graph_key,
        target,
        route_graph_key,
        avoided_indices: selection.avoided_candidate_indices,
        avoided_total: selection.avoided_total,
        total_upstreams: selection.total_upstreams,
    })
}

struct BuildSelectedParams<'a> {
    proxy: &'a ProxyService,
    prepared: &'a ResponsesWebSocketPrepared,
    attempt_scope: &'a ResponsesWebSocketAttemptScope,
    target: CapturedRouteCandidate,
    route_graph_key: Option<String>,
    avoided_indices: Vec<usize>,
    avoided_total: usize,
    total_upstreams: usize,
}

async fn build_selected(
    params: BuildSelectedParams<'_>,
) -> Result<ResponsesWebSocketSelected, ResponsesWebSocketSelectionFailure> {
    let BuildSelectedParams {
        proxy,
        prepared,
        attempt_scope,
        target,
        route_graph_key,
        avoided_indices,
        avoided_total,
        total_upstreams,
    } = params;
    if let Some(request_model) = prepared.request_model.as_deref()
        && !target.is_model_supported(request_model)
    {
        return Err(ResponsesWebSocketSelectionFailure::new(
            StatusCode::BAD_REQUEST,
            format!(
                "captured WebSocket endpoint does not support requested model {request_model:?}"
            ),
        ));
    }
    let mapping = apply_selected_model_mapping(
        &target,
        &prepared.body_for_upstream,
        prepared.request_model.as_deref(),
    );
    let model_note = mapping.model_note;
    let effective_model = mapping.effective_model;
    let mut mapped_body = mapping.body;
    let route_decision = route_decision_from_model_note(model_note.as_str());
    let route_scope = target.provider_endpoint_key();
    let attempt_context = proxy
        .state
        .capture_upstream_attempt_context(
            prepared.request_id,
            AttemptRouteEvidence {
                provider_endpoint_key: Some(target.provider_endpoint_key()),
                provider_id: Some(target.provider_id().to_owned()),
                endpoint_id: Some(target.endpoint_id().to_owned()),
                route_path: target.route_path().to_vec(),
                upstream_base_url: Some(target.base_url().to_owned()),
                mapped_model: effective_model,
            },
            AttemptProviderScopeCapture {
                endpoint: attempt_scope.endpoint.clone(),
                route_scope,
                account_fingerprint: attempt_scope.account_fingerprint,
            },
        )
        .await
        .map_err(|error| {
            ResponsesWebSocketSelectionFailure::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to capture durable WebSocket attempt context: {error}"),
            )
        })?;
    let effective_effort = if let Some(intent) = prepared.deferred_reasoning_intent {
        mapped_body = apply_deferred_reasoning_intent(
            &mapped_body,
            prepared.request_dialect,
            intent,
            attempt_context.request_contract(),
        )
        .map_err(|error| {
            ResponsesWebSocketSelectionFailure::new(StatusCode::BAD_REQUEST, error.to_string())
        })?;
        Some("max".to_string())
    } else {
        prepared.effective_effort.clone()
    };
    let filtered_body = proxy.filter.apply_bytes(mapped_body);
    let upstream_first_message =
        replace_data_message_body(&prepared.first_message, filtered_body.as_ref()).ok_or_else(
            || {
                ResponsesWebSocketSelectionFailure::new(
                    StatusCode::BAD_REQUEST,
                    "first response.create message must be text or binary JSON",
                )
            },
        )?;
    let avoid_set = avoided_indices.into_iter().collect::<HashSet<_>>();
    let mut route_attempts = Vec::new();
    let route_attempt_index = start_selected_route_attempt(
        &mut route_attempts,
        StartRouteAttemptParams {
            target: &target,
            provider_id: Some(target.provider_id()),
            provider_attempt: 0,
            upstream_attempt: 0,
            provider_max_attempts: prepared.plan.route.max_attempts,
            upstream_max_attempts: 1,
            model_note: model_note.as_str(),
            avoid_set: &avoid_set,
            avoided_total,
            total_upstreams,
        },
    );

    Ok(ResponsesWebSocketSelected {
        target,
        attempt_context,
        upstream_first_message,
        provider_id: None,
        model_note,
        effective_effort,
        route_decision,
        route_attempts,
        route_attempt_index,
        route_graph_key,
    })
}

async fn relay_websocket_streams(
    proxy: ProxyService,
    client_headers: HeaderMap,
    uri: Uri,
    client_socket: WebSocket,
    upstream: ResponsesWebSocketUpstream,
) {
    let ResponsesWebSocketUpstream {
        mut route,
        socket: upstream_socket,
        status_code,
        headers_ms: upstream_headers_ms,
        attempt_scope,
        _connection_permit,
    } = upstream;
    let (mut client_sender, mut client_receiver) = client_socket.split();
    let (mut upstream_sender, mut upstream_receiver) = upstream_socket.split();
    let mut pending: Option<PendingResponsesWebSocketCreate> = None;
    let mut active: Option<ActiveResponsesWebSocketCreate> = None;
    let close_reason: String;

    loop {
        tokio::select! {
            admission = async {
                pending
                    .as_mut()
                    .expect("pending admission branch must be guarded")
                    .admission
                    .as_mut()
                    .await
            }, if pending.is_some() => {
                let pending_create = pending
                    .take()
                    .expect("completed admission must have pending request state");
                let permit = match admission {
                    Ok(permit) => permit,
                    Err(error) => {
                        let message = format!("WebSocket request capacity admission failed: {error:?}");
                        finish_websocket_pre_upstream_failure(
                            &proxy,
                            &pending_create.prepared,
                            StatusCode::SERVICE_UNAVAILABLE,
                            message.clone(),
                            Vec::new(),
                        )
                        .await;
                        send_client_ws_error_and_close(
                            &mut client_sender,
                            1013,
                            "websocket_capacity_unavailable",
                            message.as_str(),
                        )
                        .await;
                        close_reason = message;
                        break;
                    }
                };

                let current = proxy.config.capture().await;
                let Some(current_route) = compatible_websocket_route(
                    &proxy,
                    &route,
                    current,
                    &client_headers,
                    &uri,
                    &attempt_scope,
                )
                else {
                    let message = "WebSocket route changed while request waited for capacity; reconnect required".to_string();
                    finish_websocket_pre_upstream_failure(
                        &proxy,
                        &pending_create.prepared,
                        StatusCode::CONFLICT,
                        message.clone(),
                        Vec::new(),
                    )
                    .await;
                    drop(permit);
                    send_client_ws_error_and_close(
                        &mut client_sender,
                        1012,
                        "websocket_reconnect_required",
                        message.as_str(),
                    )
                    .await;
                    close_reason = message;
                    break;
                };
                route = current_route;
                if !websocket_bound_candidate_is_current(
                    &proxy,
                    &route,
                    &pending_create.prepared,
                    WebSocketCandidateValidationPhase::AfterAdmission,
                )
                .await
                {
                    let message = "WebSocket endpoint binding is no longer eligible; reconnect required".to_string();
                    finish_websocket_pre_upstream_failure(
                        &proxy,
                        &pending_create.prepared,
                        StatusCode::CONFLICT,
                        message.clone(),
                        Vec::new(),
                    )
                    .await;
                    drop(permit);
                    send_client_ws_error_and_close(
                        &mut client_sender,
                        1012,
                        "websocket_reconnect_required",
                        message.as_str(),
                    )
                    .await;
                    close_reason = message;
                    break;
                }

                let mut prepared = pending_create.prepared;
                let selected = match build_selected(BuildSelectedParams {
                    proxy: &proxy,
                    prepared: &prepared,
                    attempt_scope: &attempt_scope,
                    target: route.target.clone(),
                    route_graph_key: (route.route_template.affinity_policy
                        != crate::config::RouteAffinityPolicy::Off)
                        .then(|| route.route_graph_key.clone()),
                    avoided_indices: route.avoided_indices.clone(),
                    avoided_total: route.avoided_total,
                    total_upstreams: route.total_upstreams,
                })
                .await
                {
                    Ok(selected) => selected,
                    Err(failure) => {
                        finish_websocket_pre_upstream_failure(
                            &proxy,
                            &prepared,
                            failure.status,
                            failure.message.clone(),
                            failure.route_attempts,
                        )
                        .await;
                        drop(permit);
                        send_client_ws_error_and_close(
                            &mut client_sender,
                            close_code_for_status(failure.status),
                            "websocket_request_rejected",
                            failure.message.as_str(),
                        )
                        .await;
                        close_reason = failure.message;
                        break;
                    }
                };
                prepared.effective_effort = selected.effective_effort.clone();
                proxy
                    .state
                    .update_request_route(prepared.request_id, selected.route_decision.clone())
                    .await;

                let attempt_handle = match proxy
                    .state
                    .begin_upstream_attempt(
                        &selected.attempt_context,
                        crate::logging::now_ms(),
                    )
                    .await
                {
                    Ok(handle) => handle,
                    Err(error) => {
                        let message = format!(
                            "failed to begin durable WebSocket upstream attempt: {error}"
                        );
                        finish_websocket_pre_upstream_failure(
                            &proxy,
                            &prepared,
                            StatusCode::INTERNAL_SERVER_ERROR,
                            message.clone(),
                            selected.route_attempts,
                        )
                        .await;
                        drop(permit);
                        send_client_ws_error_and_close(
                            &mut client_sender,
                            1011,
                            "websocket_attempt_store_unavailable",
                            message.as_str(),
                        )
                        .await;
                        close_reason = message;
                        break;
                    }
                };

                if let Err(error) = upstream_sender
                    .send(axum_to_tungstenite_message(
                        selected.upstream_first_message.clone(),
                    ))
                    .await
                {
                    let message = error.to_string();
                    let _ = finish_websocket_failure(
                        &proxy,
                        &prepared,
                        selected,
                        attempt_handle,
                        "upstream_transport_error",
                        message.clone(),
                    )
                    .await;
                    drop(permit);
                    send_client_ws_error_and_close(
                        &mut client_sender,
                        1011,
                        "upstream_transport_error",
                        message.as_str(),
                    )
                    .await;
                    close_reason = message;
                    break;
                }

                active = Some(ActiveResponsesWebSocketCreate {
                    prepared,
                    selected,
                    attempt_handle,
                    _concurrency_permit: permit,
                });
            }
            client_message = client_receiver.next() => {
                match client_message {
                    Some(Ok(message)) => {
                        if matches!(message, AxumWsMessage::Close(_)) {
                            let _ = upstream_sender.send(axum_to_tungstenite_message(message)).await;
                            close_reason = "client closed the WebSocket".to_string();
                            break;
                        }

                        let event_type = match websocket_client_event_type(&message) {
                            Ok(event_type) => event_type,
                            Err(message) => {
                                send_client_ws_error_and_close(
                                    &mut client_sender,
                                    1008,
                                    "websocket_invalid_event",
                                    message.as_str(),
                                )
                                .await;
                                close_reason = message;
                                break;
                            }
                        };
                        if event_type.as_deref() == Some("response.create") {
                            if pending.is_some() || active.is_some() {
                                let message = "overlapping response.create is not allowed on one WebSocket".to_string();
                                send_client_ws_error_and_close(
                                    &mut client_sender,
                                    1008,
                                    "websocket_overlapping_response_create",
                                    message.as_str(),
                                )
                                .await;
                                close_reason = message;
                                break;
                            }
                            let prepared = match prepare_responses_websocket(
                                &proxy,
                                PrepareResponsesWebSocketParams {
                                    uri: uri.clone(),
                                    client_headers: client_headers.clone(),
                                    first_message: message,
                                    start: Instant::now(),
                                    started_at_ms: crate::logging::now_ms(),
                                },
                            )
                            .await
                            {
                                Ok(prepared) => prepared,
                                Err((status, message)) => {
                                    send_client_ws_error_and_close(
                                        &mut client_sender,
                                        close_code_for_status(status),
                                        "websocket_request_rejected",
                                        message.as_str(),
                                    )
                                    .await;
                                    close_reason = message;
                                    break;
                                }
                            };
                            let Some(admitted_route) = compatible_websocket_route_for_plan(
                                &proxy,
                                &route,
                                &prepared.route_plan,
                                &client_headers,
                                &uri,
                                &attempt_scope,
                            ) else {
                                let message = "WebSocket route changed; reconnect required".to_string();
                                finish_websocket_pre_upstream_failure(
                                    &proxy,
                                    &prepared,
                                    StatusCode::CONFLICT,
                                    message.clone(),
                                    Vec::new(),
                                )
                                .await;
                                send_client_ws_error_and_close(
                                    &mut client_sender,
                                    1012,
                                    "websocket_reconnect_required",
                                    message.as_str(),
                                )
                                .await;
                                close_reason = message;
                                break;
                            };
                            route = admitted_route;
                            if !websocket_bound_candidate_is_current(
                                &proxy,
                                &route,
                                &prepared,
                                WebSocketCandidateValidationPhase::BeforeAdmission,
                            )
                            .await
                            {
                                let message = "WebSocket endpoint binding is no longer eligible; reconnect required".to_string();
                                finish_websocket_pre_upstream_failure(
                                    &proxy,
                                    &prepared,
                                    StatusCode::CONFLICT,
                                    message.clone(),
                                    Vec::new(),
                                )
                                .await;
                                send_client_ws_error_and_close(
                                    &mut client_sender,
                                    1012,
                                    "websocket_reconnect_required",
                                    message.as_str(),
                                )
                                .await;
                                close_reason = message;
                                break;
                            }
                            let admission = websocket_concurrency_admission(
                                proxy.clone(),
                                route.route_template.clone(),
                                route.target.clone(),
                                route.route_revision,
                                prepared.session_id.clone(),
                            );
                            pending = Some(PendingResponsesWebSocketCreate {
                                prepared,
                                admission,
                            });
                            continue;
                        }
                        if event_type.as_deref() == Some("response.cancel")
                            && let Some(pending_create) = pending.take()
                        {
                            let PendingResponsesWebSocketCreate {
                                prepared,
                                admission,
                            } = pending_create;
                            drop(admission);
                            finish_websocket_pre_upstream_failure(
                                &proxy,
                                &prepared,
                                StatusCode::REQUEST_TIMEOUT,
                                "response.create was canceled while waiting for capacity"
                                    .to_string(),
                                Vec::new(),
                            )
                            .await;
                            continue;
                        }

                        if let Err(error) = upstream_sender.send(axum_to_tungstenite_message(message)).await {
                            close_reason = error.to_string();
                            break;
                        }
                    }
                    Some(Err(error)) => {
                        close_reason = error.to_string();
                        break;
                    }
                    None => {
                        close_reason = "client WebSocket stream ended".to_string();
                        break;
                    }
                }
            }
            upstream_message = upstream_receiver.next() => {
                match upstream_message {
                    Some(Ok(message)) => {
                        if matches!(message, tungstenite::Message::Close(_)) {
                            let _ = client_sender.send(tungstenite_to_axum_message(message)).await;
                            close_reason = "upstream closed the WebSocket".to_string();
                            break;
                        }
                        let terminal = websocket_upstream_terminal(&message);
                        if let Some(terminal) = terminal
                            && let Some(active_create) = active.take()
                        {
                            let publish_ok = match terminal {
                                WebSocketTerminal::Completed => {
                                    finish_websocket_success(
                                        &proxy,
                                        &active_create.prepared,
                                        active_create.selected,
                                        active_create.attempt_handle,
                                        status_code,
                                        upstream_headers_ms,
                                        &message,
                                    )
                                    .await
                                }
                                WebSocketTerminal::LogicalFailure => {
                                    finish_websocket_logical_failure(
                                        &proxy,
                                        &active_create.prepared,
                                        active_create.selected,
                                        active_create.attempt_handle,
                                        upstream_headers_ms,
                                        websocket_message_text(&message),
                                    )
                                    .await
                                }
                                WebSocketTerminal::UpstreamFailure => {
                                    finish_websocket_failure(
                                        &proxy,
                                        &active_create.prepared,
                                        active_create.selected,
                                        active_create.attempt_handle,
                                        "upstream_response_error",
                                        websocket_message_text(&message),
                                    )
                                    .await
                                }
                            };
                            drop(active_create._concurrency_permit);
                            if !publish_ok {
                                let message = "failed to commit WebSocket request terminal".to_string();
                                send_client_ws_error_and_close(
                                    &mut client_sender,
                                    1011,
                                    "websocket_terminal_commit_failed",
                                    message.as_str(),
                                )
                                .await;
                                close_reason = message;
                                break;
                            }
                        }
                        if let Err(error) = client_sender.send(tungstenite_to_axum_message(message)).await {
                            close_reason = error.to_string();
                            break;
                        }
                    }
                    Some(Err(error)) => {
                        close_reason = error.to_string();
                        break;
                    }
                    None => {
                        close_reason = "upstream WebSocket stream ended".to_string();
                        break;
                    }
                }
            }
        }
    }

    if let Some(pending_create) = pending.take() {
        finish_websocket_pre_upstream_failure(
            &proxy,
            &pending_create.prepared,
            StatusCode::BAD_GATEWAY,
            close_reason.clone(),
            Vec::new(),
        )
        .await;
    }
    if let Some(active_create) = active.take() {
        let _ = finish_websocket_failure(
            &proxy,
            &active_create.prepared,
            active_create.selected,
            active_create.attempt_handle,
            CodexFailureKind::StreamError.helper_error(),
            close_reason,
        )
        .await;
    }
}

fn websocket_concurrency_admission(
    proxy: ProxyService,
    template: Arc<RoutePlanTemplate>,
    target: CapturedRouteCandidate,
    runtime_revision: u64,
    session_id: Option<String>,
) -> ConcurrencyAdmissionFuture {
    Box::pin(async move {
        acquire_candidate_concurrency_permit(
            &proxy,
            template.as_ref(),
            target.candidate(),
            runtime_revision,
            session_id.as_deref(),
        )
        .await
    })
}

async fn websocket_bound_candidate_is_current(
    proxy: &ProxyService,
    route: &ResponsesWebSocketHandshakeRoute,
    prepared: &ResponsesWebSocketPrepared,
    phase: WebSocketCandidateValidationPhase,
) -> bool {
    let reservation_guard =
        if route.route_template.affinity_policy != crate::config::RouteAffinityPolicy::Off {
            lock_session_route_reservation_selection(proxy, prepared.session_id.as_deref()).await
        } else {
            None
        };
    let Ok(mut runtime) = route_graph_runtime_for_request(
        proxy,
        route.route_template.as_ref(),
        route.routing_control_graph_key.as_str(),
        route.route_revision,
        route.runtime_snapshot.provider_policy().as_ref(),
        prepared.session_id.as_deref(),
    )
    .await
    else {
        return false;
    };
    if reservation_guard.is_some() {
        match apply_session_route_reservation_to_runtime(
            proxy,
            prepared.request_id,
            prepared.session_id.as_deref(),
            route.route_template.as_ref(),
            Some(route.route_graph_key.as_str()),
            &mut runtime,
        )
        .await
        {
            SessionRouteReservationDecision::Busy | SessionRouteReservationDecision::Failed => {
                return false;
            }
            SessionRouteReservationDecision::None
            | SessionRouteReservationDecision::Available(_) => {}
        }
    }
    let provider_endpoint = route
        .route_template
        .candidate_provider_endpoint_key(route.target.candidate());
    let is_remote_compaction_request = prepared.request_continuity.is_remote_compaction_v1_request
        || prepared.request_continuity.is_remote_compaction_v2_request;
    let continuity = RequestContinuityContract::from_route(RouteContinuityDecisionInput {
        is_remote_compaction_request,
        remote_compaction_requires_affinity: prepared
            .request_continuity
            .remote_compaction_requires_affinity,
        affinity_policy: Some(route.route_template.affinity_policy),
    });
    if route_graph_request_requires_existing_affinity(
        continuity,
        &runtime,
        route.route_template.as_ref(),
    ) {
        return false;
    }
    let mut route_state = RoutePlanAttemptState::default();
    restrict_route_state_to_affinity_continuity_domain(
        continuity,
        &mut route_state,
        route.route_template.as_ref(),
        &runtime,
    );
    let executor = RoutePlanExecutor::new(route.route_template.as_ref());
    let selection_runtime = match phase {
        WebSocketCandidateValidationPhase::BeforeAdmission => {
            runtime_for_capacity_wait_selection(route.route_template.as_ref(), &runtime)
        }
        WebSocketCandidateValidationPhase::AfterAdmission => {
            runtime_for_acquired_candidate_revalidation(
                route.route_template.as_ref(),
                &runtime,
                route.target.candidate(),
            )
        }
    };
    if matches!(
        route.binding_source,
        WebSocketHandshakeBindingSource::Explicit | WebSocketHandshakeBindingSource::Singleton
    ) {
        let validation_runtime = runtime_for_acquired_candidate_revalidation(
            route.route_template.as_ref(),
            &runtime,
            route.target.candidate(),
        );
        let candidate = validation_runtime
            .candidate_runtime_snapshot(route.route_template.as_ref(), route.target.candidate());
        if !candidate
            .skip_reasons_for_candidate(route.target.candidate(), prepared.request_model.as_deref())
            .is_empty()
        {
            return false;
        }
    } else {
        let selection = select_route_graph_candidate(
            &executor,
            &mut route_state,
            &selection_runtime,
            prepared.request_model.as_deref(),
            is_remote_compaction_request,
            continuity,
        );
        let Some(selected) = selection.selected else {
            let validation_runtime = runtime_for_acquired_candidate_revalidation(
                route.route_template.as_ref(),
                &runtime,
                route.target.candidate(),
            );
            let candidate = validation_runtime.candidate_runtime_snapshot(
                route.route_template.as_ref(),
                route.target.candidate(),
            );
            return candidate
                .skip_reasons_for_candidate(
                    route.target.candidate(),
                    prepared.request_model.as_deref(),
                )
                .is_empty();
        };
        if selected.provider_endpoint != provider_endpoint {
            return false;
        }
    }
    if reservation_guard.is_some() {
        match claim_session_route_reservation(
            proxy,
            prepared.request_id,
            prepared.session_id.as_deref(),
            prepared.session_identity_source,
            Some(route.route_graph_key.as_str()),
            &route.target,
        )
        .await
        {
            SessionRouteReservationDecision::Available(claimed) => {
                if claimed.provider_endpoint != provider_endpoint {
                    return false;
                }
                runtime.set_affinity_provider_endpoint_with_observed_at(
                    Some(claimed.provider_endpoint),
                    Some(claimed.last_selected_at_ms),
                    Some(claimed.last_changed_at_ms),
                );
            }
            SessionRouteReservationDecision::Busy | SessionRouteReservationDecision::Failed => {
                return false;
            }
            SessionRouteReservationDecision::None => {}
        }
    }
    drop(reservation_guard);
    let validation_runtime = runtime_for_acquired_candidate_revalidation(
        route.route_template.as_ref(),
        &runtime,
        route.target.candidate(),
    );
    let candidate = validation_runtime
        .candidate_runtime_snapshot(route.route_template.as_ref(), route.target.candidate());
    candidate
        .skip_reasons_for_candidate(route.target.candidate(), prepared.request_model.as_deref())
        .is_empty()
}

#[derive(Clone, Copy)]
enum WebSocketCandidateValidationPhase {
    BeforeAdmission,
    AfterAdmission,
}

async fn finish_websocket_success(
    proxy: &ProxyService,
    prepared: &ResponsesWebSocketPrepared,
    mut selected: ResponsesWebSocketSelected,
    attempt_handle: AttemptHandle,
    status_code: u16,
    upstream_headers_ms: u64,
    terminal_message: &tungstenite::Message,
) -> bool {
    let duration_ms = prepared.start.elapsed().as_millis() as u64;
    record_status_route_attempt(
        &mut selected.route_attempts,
        StatusRouteAttemptParams {
            target: &selected.target,
            route_attempt_index: selected.route_attempt_index,
            status_code,
            error_class: None,
            model_note: selected.model_note.as_str(),
            upstream_headers_ms,
            duration_ms,
            cooldown_secs: None,
            cooldown_reason: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
        },
    );
    let retry = retry_info_for_observed_attempts(&selected.route_attempts);
    let terminal_body = websocket_message_bytes(terminal_message);
    let reported_model = terminal_body.and_then(extract_model_from_response_body);
    let service_tier = ServiceTierLog {
        actual: terminal_body.and_then(extract_service_tier_from_response_body),
        ..prepared.base_service_tier.clone()
    };
    let codex_bridge = prepared
        .request_continuity
        .is_remote_compaction_v2_request
        .then(|| CodexBridgeLog {
            patch_mode: "request-dialect".to_string(),
            remote_compaction_v1_request: false,
            remote_compaction_v2_request: prepared
                .request_continuity
                .is_remote_compaction_v2_request,
            responses_websocket_request: true,
            strips_client_auth: false,
        });

    let mut publication = RequestPublication::new_terminal(
        prepared.request_id,
        status_code,
        duration_ms,
        prepared.started_at_ms,
        true,
    );
    publication.winning_attempt = Some(attempt_handle);
    publication.ttfb_ms = Some(upstream_headers_ms);
    publication.provider_id = selected
        .provider_id
        .or_else(|| Some(selected.target.provider_id().to_owned()));
    publication.endpoint_id = Some(selected.target.endpoint_id().to_owned());
    publication.provider_endpoint_key = Some(selected.target.provider_endpoint_key());
    publication.upstream_origin = crate::logging::upstream_origin(selected.target.base_url());
    publication.session_id = prepared.session_id.clone();
    publication.session_identity_source = prepared.session_identity_source;
    publication.cwd = prepared.cwd.clone();
    publication.reasoning_effort = prepared.effective_effort.clone();
    publication.service_tier = service_tier;
    publication.reported_model = reported_model;
    publication.codex_bridge = codex_bridge;
    publication.usage = terminal_body.and_then(crate::usage::extract_usage_from_bytes);
    publication.route_decision = Some(selected.route_decision.clone());
    publication.retry = retry;
    publication.route_affinity_success = prepare_session_route_affinity_success(
        prepared.request_id,
        prepared.session_id.as_deref(),
        prepared.session_identity_source,
        selected.route_graph_key.as_deref(),
        &selected.target,
        &selected.route_attempts,
        selected.route_attempt_index,
    );
    let observer = RequestObserver::new(proxy, &prepared.method, prepared.uri.path());
    let publication = publication.with_route_decision_model();
    if let Err(error) = proxy.state.finish_upstream_attempt(
        attempt_handle,
        AttemptOutcome::Succeeded,
        crate::logging::now_ms(),
        EconomicsState::Unknown,
    ) {
        tracing::error!(
            request_id = prepared.request_id,
            error = %error,
            "failed to commit durable WebSocket attempt success"
        );
        return false;
    }
    let published = if prepared.is_warmup {
        observer
            .publish_non_economic_terminal_once(publication)
            .await
    } else {
        observer.publish_terminal_once(publication).await
    };
    if !published {
        return false;
    }

    record_attempt_success(proxy.state.as_ref(), proxy.service_name, &selected.target).await;
    true
}

async fn finish_websocket_pre_upstream_failure(
    proxy: &ProxyService,
    prepared: &ResponsesWebSocketPrepared,
    status: StatusCode,
    message: String,
    route_attempts: Vec<RouteAttemptLog>,
) {
    let retry = retry_info_for_failed_attempts(&route_attempts);
    let params = FailedProxyRequestParams {
        proxy,
        method: &prepared.method,
        path: prepared.uri.path(),
        request_id: prepared.request_id,
        status,
        message: message.clone(),
        duration_ms: prepared.start.elapsed().as_millis() as u64,
        started_at_ms: prepared.started_at_ms,
        session_id: prepared.session_id.clone(),
        session_identity_source: prepared.session_identity_source,
        cwd: prepared.cwd.clone(),
        effective_effort: prepared.effective_effort.clone(),
        service_tier: prepared.base_service_tier.clone(),
        codex_bridge: None,
        retry,
        failure_route_attempts: route_attempts,
    };
    if prepared.is_warmup {
        let _ = finish_non_economic_failed_proxy_request(params).await;
    } else {
        let _ = finish_failed_proxy_request(params).await;
    }
}

#[derive(Clone, Copy)]
enum WebSocketTerminal {
    Completed,
    LogicalFailure,
    UpstreamFailure,
}

fn websocket_client_event_type(message: &AxumWsMessage) -> Result<Option<String>, String> {
    let body = match message {
        AxumWsMessage::Text(text) => text.as_bytes(),
        AxumWsMessage::Binary(bytes) => bytes.as_ref(),
        AxumWsMessage::Ping(_) | AxumWsMessage::Pong(_) | AxumWsMessage::Close(_) => {
            return Ok(None);
        }
    };
    let value = serde_json::from_slice::<serde_json::Value>(body)
        .map_err(|error| format!("WebSocket data frame is not valid JSON: {error}"))?;
    Ok(value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned))
}

fn websocket_upstream_terminal(message: &tungstenite::Message) -> Option<WebSocketTerminal> {
    let body = websocket_message_bytes(message)?;
    let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("response.completed") => Some(WebSocketTerminal::Completed),
        Some("response.incomplete" | "response.cancelled" | "response.canceled") => {
            Some(WebSocketTerminal::LogicalFailure)
        }
        Some("response.failed" | "error") => Some(WebSocketTerminal::UpstreamFailure),
        _ => None,
    }
}

fn websocket_message_bytes(message: &tungstenite::Message) -> Option<&[u8]> {
    match message {
        tungstenite::Message::Text(text) => Some(text.as_bytes()),
        tungstenite::Message::Binary(bytes) => Some(bytes.as_ref()),
        _ => None,
    }
}

fn websocket_message_text(message: &tungstenite::Message) -> String {
    websocket_message_bytes(message)
        .map(|body| String::from_utf8_lossy(body).into_owned())
        .unwrap_or_else(|| "upstream WebSocket request failed".to_string())
}

async fn send_client_ws_error_and_close(
    sender: &mut futures_util::stream::SplitSink<WebSocket, AxumWsMessage>,
    close_code: u16,
    error_code: &'static str,
    message: &str,
) {
    let payload = serde_json::json!({
        "type": "error",
        "code": error_code,
        "message": message,
        "param": null,
        "sequence_number": 1,
    });
    let _ = sender
        .send(AxumWsMessage::Text(payload.to_string().into()))
        .await;
    let _ = sender
        .send(AxumWsMessage::Close(Some(AxumCloseFrame {
            code: close_code,
            reason: Utf8Bytes::from(error_code),
        })))
        .await;
}

fn route_decision_from_model_note(model_note: &str) -> RouteDecisionProvenance {
    model_note
        .trim()
        .split_once("->")
        .map(|(_, mapped)| mapped.trim())
        .unwrap_or_else(|| model_note.trim())
        .split_whitespace()
        .next()
        .filter(|model| !model.is_empty() && *model != "-")
        .map(|model| RouteDecisionProvenance {
            effective_model: Some(ResolvedRouteValue {
                value: model.to_string(),
                source: if model_note.contains("->") {
                    RouteValueSource::ProviderMapping
                } else {
                    RouteValueSource::RequestPayload
                },
            }),
            ..RouteDecisionProvenance::default()
        })
        .unwrap_or_default()
}

async fn finish_websocket_failure(
    proxy: &ProxyService,
    prepared: &ResponsesWebSocketPrepared,
    mut selected: ResponsesWebSocketSelected,
    attempt_handle: AttemptHandle,
    error_class: &'static str,
    message: String,
) -> bool {
    record_error_route_attempt(
        &mut selected.route_attempts,
        ErrorRouteAttemptParams {
            target: &selected.target,
            route_attempt_index: selected.route_attempt_index,
            kind: RouteAttemptErrorKind::Transport,
            model_note: selected.model_note.as_str(),
            duration_ms: Some(prepared.start.elapsed().as_millis() as u64),
            cooldown_secs: Some(prepared.plan.transport_cooldown_secs),
            cooldown_reason: Some(error_class),
        },
    );
    let target = selected.target.clone();
    if !commit_websocket_failed_terminal(proxy, prepared, selected, attempt_handle, message.clone())
        .await
    {
        return false;
    }
    let cooldown_secs = prepared.plan.transport_cooldown_secs;
    let cooldown_backoff = prepared.cooldown_backoff;
    let mut avoid_set = HashSet::new();
    let mut avoided_total = 0;
    let mut last_err = None;
    apply_terminal_upstream_failure(TerminalUpstreamFailureParams {
        proxy,
        target: &target,
        penalize_endpoint: true,
        cooldown_secs,
        cooldown_backoff,
        error_message: message,
        avoid_set: &mut avoid_set,
        avoided_total: &mut avoided_total,
        last_err: &mut last_err,
    })
    .await;
    true
}

async fn finish_websocket_logical_failure(
    proxy: &ProxyService,
    prepared: &ResponsesWebSocketPrepared,
    mut selected: ResponsesWebSocketSelected,
    attempt_handle: AttemptHandle,
    upstream_headers_ms: u64,
    message: String,
) -> bool {
    record_status_route_attempt(
        &mut selected.route_attempts,
        StatusRouteAttemptParams {
            target: &selected.target,
            route_attempt_index: selected.route_attempt_index,
            status_code: StatusCode::BAD_GATEWAY.as_u16(),
            error_class: Some("upstream_response_logical_failure"),
            model_note: selected.model_note.as_str(),
            upstream_headers_ms,
            duration_ms: prepared.start.elapsed().as_millis() as u64,
            cooldown_secs: None,
            cooldown_reason: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
        },
    );
    commit_websocket_failed_terminal(proxy, prepared, selected, attempt_handle, message).await
}

async fn commit_websocket_failed_terminal(
    proxy: &ProxyService,
    prepared: &ResponsesWebSocketPrepared,
    selected: ResponsesWebSocketSelected,
    attempt_handle: AttemptHandle,
    message: String,
) -> bool {
    if let Err(error) = proxy.state.finish_upstream_attempt(
        attempt_handle,
        AttemptOutcome::Failed,
        crate::logging::now_ms(),
        EconomicsState::Unknown,
    ) {
        tracing::error!(
            request_id = prepared.request_id,
            error = %error,
            "failed to commit durable WebSocket attempt failure"
        );
        return false;
    }
    let retry = retry_info_for_failed_attempts(&selected.route_attempts);
    let params = FailedProxyRequestParams {
        proxy,
        method: &prepared.method,
        path: prepared.uri.path(),
        request_id: prepared.request_id,
        status: StatusCode::BAD_GATEWAY,
        message: message.clone(),
        duration_ms: prepared.start.elapsed().as_millis() as u64,
        started_at_ms: prepared.started_at_ms,
        session_id: prepared.session_id.clone(),
        session_identity_source: prepared.session_identity_source,
        cwd: prepared.cwd.clone(),
        effective_effort: prepared.effective_effort.clone(),
        service_tier: prepared.base_service_tier.clone(),
        codex_bridge: None,
        retry,
        failure_route_attempts: selected.route_attempts,
    };
    let (_, _, published) = if prepared.is_warmup {
        finish_non_economic_failed_proxy_request_with_publication_result(params).await
    } else {
        finish_failed_proxy_request_with_publication_result(params).await
    };
    if !published {
        return false;
    }
    true
}

fn first_message_body_bytes(message: &AxumWsMessage) -> Option<axum::body::Bytes> {
    match message {
        AxumWsMessage::Text(text) => Some(axum::body::Bytes::copy_from_slice(text.as_bytes())),
        AxumWsMessage::Binary(bytes) => Some(bytes.clone()),
        _ => None,
    }
}

fn replace_data_message_body(message: &AxumWsMessage, body: &[u8]) -> Option<AxumWsMessage> {
    match message {
        AxumWsMessage::Text(_) => String::from_utf8(body.to_vec())
            .ok()
            .map(|text| AxumWsMessage::Text(text.into())),
        AxumWsMessage::Binary(_) => Some(AxumWsMessage::Binary(
            axum::body::Bytes::copy_from_slice(body),
        )),
        _ => None,
    }
}

fn upstream_ws_handshake_headers(
    client_headers: &HeaderMap,
    target: &CapturedRouteCandidate,
    target_url: &str,
) -> Result<HeaderMap, UpstreamAuthResolutionError> {
    const ALLOWED_CLIENT_HEADERS: &[&str] = &[
        "authorization",
        "chatgpt-account-id",
        "openai-organization",
        "openai-project",
        "x-api-key",
        "x-openai-fedramp",
        "x-openai-organization",
        "x-openai-project",
        "x-organization-id",
        "x-project-id",
        "session_id",
        "x-session-id",
        "session-id",
        "conversation_id",
        "thread-id",
        "x-client-request-id",
        "originator",
        "user-agent",
        "accept-language",
        "x-codex-beta-features",
        "x-codex-window-id",
        "x-codex-parent-thread-id",
        "x-codex-turn-state",
        "x-openai-subagent",
        "x-openai-memgen-request",
        "x-openai-internal-codex-residency",
        "x-responsesapi-include-timing-metrics",
        "x-oai-attestation",
        "sec-websocket-protocol",
    ];

    let mut headers = copy_allowlisted_ws_headers(client_headers, ALLOWED_CLIENT_HEADERS);
    if let Some(session_identity) = extract_session_identity(client_headers)
        && let Ok(value) = HeaderValue::from_str(session_identity.value())
    {
        for name in ["x-session-id", "session-id", "thread-id"] {
            if !headers.contains_key(name) {
                headers.insert(HeaderName::from_static(name), value.clone());
            }
        }
    }
    inject_auth_headers("codex", target.credential(), target_url, &mut headers)?;
    headers.insert(
        HeaderName::from_static("openai-beta"),
        HeaderValue::from_static(RESPONSES_WS_BETA_HEADER),
    );
    Ok(headers)
}

fn filter_upstream_ws_success_headers(headers: &HeaderMap) -> HeaderMap {
    copy_allowlisted_ws_headers(
        headers,
        &[
            "x-codex-turn-state",
            "x-reasoning-included",
            "x-models-etag",
            "openai-model",
            "x-request-id",
            "x-oai-request-id",
            "cf-ray",
        ],
    )
}

fn filter_upstream_ws_failure_headers(headers: &HeaderMap) -> HeaderMap {
    copy_allowlisted_ws_headers(
        headers,
        &[
            "content-type",
            "retry-after",
            "retry-after-ms",
            "x-request-id",
            "x-oai-request-id",
            "cf-ray",
            "upgrade",
            "sec-websocket-version",
        ],
    )
}

fn copy_allowlisted_ws_headers(headers: &HeaderMap, allowlist: &[&str]) -> HeaderMap {
    let mut filtered = HeaderMap::new();
    for name in allowlist {
        let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        for value in headers.get_all(&header_name).iter() {
            filtered.append(header_name.clone(), value.clone());
        }
    }
    filtered
}

fn upstream_ws_handshake_error_response(error: tungstenite::Error) -> Response {
    let tungstenite::Error::Http(response) = error else {
        tracing::warn!(
            error_class = "upstream_websocket_transport_error",
            "upstream WebSocket handshake transport failed"
        );
        return (
            StatusCode::BAD_GATEWAY,
            "upstream WebSocket handshake failed",
        )
            .into_response();
    };
    let (parts, body) = (*response).into_parts();
    let status = StatusCode::from_u16(parts.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut filtered_headers = filter_upstream_ws_failure_headers(&parts.headers);
    let body = match body.filter(|body| !body.is_empty()) {
        Some(body) => body,
        None => {
            filtered_headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            b"upstream WebSocket handshake rejected".to_vec()
        }
    };
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    for (name, value) in filtered_headers {
        if let Some(name) = name {
            response.headers_mut().append(name, value);
        }
    }
    response
}

fn upstream_ws_request(
    url: &str,
    headers: &HeaderMap,
) -> Result<tungstenite_http::Request<()>, tungstenite::Error> {
    let mut request = url.into_client_request()?;
    for (name, value) in headers {
        if let (Ok(name), Ok(mut converted_value)) = (
            tungstenite_http::HeaderName::from_bytes(name.as_str().as_bytes()),
            tungstenite_http::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            converted_value.set_sensitive(value.is_sensitive());
            request.headers_mut().append(name, converted_value);
        }
    }
    Ok(request)
}

fn http_url_to_ws(mut url: reqwest::Url) -> Result<reqwest::Url, String> {
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        "ws" | "wss" => return Ok(url),
        other => {
            return Err(format!(
                "unsupported upstream websocket URL scheme: {other}"
            ));
        }
    };
    url.set_scheme(scheme)
        .map_err(|_| format!("failed to convert upstream URL to {scheme}"))?;
    Ok(url)
}

fn axum_to_tungstenite_message(message: AxumWsMessage) -> tungstenite::Message {
    match message {
        AxumWsMessage::Text(text) => tungstenite::Message::Text(text.to_string().into()),
        AxumWsMessage::Binary(bytes) => tungstenite::Message::Binary(bytes),
        AxumWsMessage::Ping(bytes) => tungstenite::Message::Ping(bytes),
        AxumWsMessage::Pong(bytes) => tungstenite::Message::Pong(bytes),
        AxumWsMessage::Close(frame) => tungstenite::Message::Close(frame.map(|frame| {
            tungstenite::protocol::frame::CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::from(frame.code),
                reason: frame.reason.to_string().into(),
            }
        })),
    }
}

fn tungstenite_to_axum_message(message: tungstenite::Message) -> AxumWsMessage {
    match message {
        tungstenite::Message::Text(text) => AxumWsMessage::Text(text.to_string().into()),
        tungstenite::Message::Binary(bytes) => AxumWsMessage::Binary(bytes),
        tungstenite::Message::Ping(bytes) => AxumWsMessage::Ping(bytes),
        tungstenite::Message::Pong(bytes) => AxumWsMessage::Pong(bytes),
        tungstenite::Message::Close(frame) => {
            AxumWsMessage::Close(frame.map(|frame| AxumCloseFrame {
                code: frame.code.into(),
                reason: Utf8Bytes::from(frame.reason.to_string()),
            }))
        }
        tungstenite::Message::Frame(_) => AxumWsMessage::Close(None),
    }
}

fn close_code_for_status(status: StatusCode) -> u16 {
    if status == StatusCode::BAD_REQUEST {
        1008
    } else {
        1011
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    fn ws_target(base_url: &str, auth: crate::config::UpstreamAuth) -> CapturedRouteCandidate {
        let candidate = RouteCandidate {
            provider_id: "test".to_string(),
            provider_alias: None,
            endpoint_id: "default".to_string(),
            base_url: base_url.to_string(),
            continuity_domain: None,
            auth,
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            model_rules: std::sync::Arc::default(),
            route_path: vec!["root".to_string(), "test".to_string()],
            preference_group: 0,
            stable_index: 0,
            concurrency: crate::routing_ir::RouteCandidateConcurrency::default(),
        };
        CapturedRouteCandidate::capture_for_service("codex", &candidate)
    }

    fn codex_account_headers() -> HeaderMap {
        HeaderMap::from_iter([
            (
                HeaderName::from_static("authorization"),
                HeaderValue::from_static("Bearer client-token"),
            ),
            (
                HeaderName::from_static("chatgpt-account-id"),
                HeaderValue::from_static("account-id"),
            ),
            (
                HeaderName::from_static("openai-organization"),
                HeaderValue::from_static("org-id"),
            ),
            (
                HeaderName::from_static("openai-project"),
                HeaderValue::from_static("project-id"),
            ),
            (
                HeaderName::from_static("x-api-key"),
                HeaderValue::from_static("client-api-key"),
            ),
            (
                HeaderName::from_static("x-oai-attestation"),
                HeaderValue::from_static("device-attestation"),
            ),
            (
                HeaderName::from_static("originator"),
                HeaderValue::from_static("codex_cli_rs"),
            ),
            (
                HeaderName::from_static("cookie"),
                HeaderValue::from_static("session=secret"),
            ),
        ])
    }

    #[test]
    fn websocket_handshake_requires_explicit_anonymous_opt_in_for_remote_relay() {
        let client_headers = codex_account_headers();
        let official_target = ws_target(
            "https://api.openai.com/v1",
            crate::config::UpstreamAuth::default(),
        );

        let official = upstream_ws_handshake_headers(
            &client_headers,
            &official_target,
            "https://api.openai.com/v1/responses",
        )
        .expect("build official WebSocket handshake headers");

        for header in [
            "authorization",
            "chatgpt-account-id",
            "openai-organization",
            "openai-project",
            "x-api-key",
            "x-oai-attestation",
        ] {
            assert_eq!(official.get(header), client_headers.get(header), "{header}");
        }
        assert!(!official.contains_key("cookie"));

        let relay_target = ws_target(
            "https://relay.example/v1",
            crate::config::UpstreamAuth::default(),
        );
        let relay_error = upstream_ws_handshake_headers(
            &client_headers,
            &relay_target,
            "https://relay.example/v1/responses",
        )
        .expect_err("remote relay must not receive an anonymous handshake by default");
        assert!(matches!(
            relay_error,
            UpstreamAuthResolutionError::AnonymousNotAllowed
        ));

        let anonymous_relay_target = ws_target(
            "https://relay.example/v1",
            crate::config::UpstreamAuth {
                allow_anonymous: Some(true),
                ..crate::config::UpstreamAuth::default()
            },
        );
        let relay = upstream_ws_handshake_headers(
            &client_headers,
            &anonymous_relay_target,
            "https://relay.example/v1/responses",
        )
        .expect("build explicitly anonymous relay WebSocket handshake headers");

        for header in [
            "authorization",
            "chatgpt-account-id",
            "openai-organization",
            "openai-project",
            "x-api-key",
            "x-oai-attestation",
        ] {
            assert!(!relay.contains_key(header), "{header}");
        }
        assert_eq!(relay.get("originator"), client_headers.get("originator"));

        let helper_target = ws_target(
            "https://api.openai.com/v1",
            crate::config::UpstreamAuth {
                auth_token: Some("helper-token".to_string().into()),
                ..crate::config::UpstreamAuth::default()
            },
        );
        let helper = upstream_ws_handshake_headers(
            &client_headers,
            &helper_target,
            "https://api.openai.com/v1/responses",
        )
        .expect("build helper-authenticated WebSocket handshake headers");
        assert_eq!(
            helper.get("authorization"),
            Some(&HeaderValue::from_static("Bearer helper-token"))
        );
        assert!(
            helper
                .get("authorization")
                .expect("helper authorization header")
                .is_sensitive()
        );
        let upstream_request = upstream_ws_request("wss://api.openai.com/v1/responses", &helper)
            .expect("build upstream WebSocket request");
        assert!(
            upstream_request
                .headers()
                .get("authorization")
                .expect("upstream authorization header")
                .is_sensitive()
        );
        for header in [
            "chatgpt-account-id",
            "openai-organization",
            "openai-project",
            "x-api-key",
            "x-oai-attestation",
        ] {
            assert!(!helper.contains_key(header), "{header}");
        }
    }

    #[test]
    fn websocket_handshake_rejects_missing_explicit_auth_reference() {
        let missing_reference = format!(
            "CODEX_HELPER_TEST_MISSING_WS_AUTH_{}",
            uuid::Uuid::new_v4().simple()
        );
        let target = ws_target(
            "https://relay.example/v1",
            crate::config::UpstreamAuth {
                auth_token_env: Some(missing_reference.clone()),
                ..crate::config::UpstreamAuth::default()
            },
        );

        let error = upstream_ws_handshake_headers(
            &codex_account_headers(),
            &target,
            "https://relay.example/v1/responses",
        )
        .expect_err("missing explicit auth reference must fail closed");

        assert!(matches!(
            error,
            UpstreamAuthResolutionError::MissingReference { name, .. }
                if name == missing_reference
        ));
    }

    #[tokio::test]
    async fn upstream_handshake_failures_preserve_status_body_and_safe_headers() {
        for status in [401_u16, 429, 500, 503] {
            let mut upstream =
                tungstenite_http::Response::new(Some(br#"{"error":"denied"}"#.to_vec()));
            *upstream.status_mut() =
                tungstenite_http::StatusCode::from_u16(status).expect("status");
            upstream.headers_mut().insert(
                "content-type",
                tungstenite_http::HeaderValue::from_static("application/json"),
            );
            upstream.headers_mut().insert(
                "retry-after",
                tungstenite_http::HeaderValue::from_static("5"),
            );
            upstream.headers_mut().insert(
                "x-request-id",
                tungstenite_http::HeaderValue::from_static("request-safe"),
            );
            for sensitive in ["set-cookie", "location", "authorization"] {
                upstream.headers_mut().insert(
                    tungstenite_http::HeaderName::from_bytes(sensitive.as_bytes())
                        .expect("header name"),
                    tungstenite_http::HeaderValue::from_static("must-not-cross-boundary"),
                );
            }

            let downstream =
                upstream_ws_handshake_error_response(tungstenite::Error::Http(Box::new(upstream)));

            assert_eq!(downstream.status().as_u16(), status);
            assert_eq!(
                downstream
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok()),
                Some("application/json")
            );
            assert_eq!(
                downstream
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok()),
                Some("5")
            );
            assert_eq!(
                downstream
                    .headers()
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok()),
                Some("request-safe")
            );
            for sensitive in ["set-cookie", "location", "authorization"] {
                assert!(!downstream.headers().contains_key(sensitive));
            }
            assert_eq!(
                to_bytes(downstream.into_body(), 1024)
                    .await
                    .expect("response body")
                    .as_ref(),
                br#"{"error":"denied"}"#
            );
        }
    }
}
