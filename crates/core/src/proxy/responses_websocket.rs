use std::collections::HashSet;
use std::time::Instant;

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{
    CloseFrame as AxumCloseFrame, Message as AxumWsMessage, Utf8Bytes, WebSocket, WebSocketUpgrade,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http as tungstenite_http;

use crate::codex_integration::CodexPatchMode;
use crate::lb::{COOLDOWN_SECS, LoadBalancer};
use crate::logging::{CodexBridgeLog, RouteAttemptLog, ServiceTierLog, log_retry_trace};
use crate::routing_ir::{
    RouteCandidate, RoutePlanAttemptState, RoutePlanExecutor, RoutePlanRuntimeState,
    RoutePlanTemplate,
};
use crate::state::{
    FinishRequestParams, ResolvedRouteValue, RouteDecisionProvenance, RouteValueSource,
    SessionIdentitySource,
};

use super::attempt_failures::{TerminalUpstreamFailureParams, apply_terminal_upstream_failure};
use super::attempt_health::record_attempt_success;
use super::attempt_request::inject_auth_headers;
use super::attempt_target::AttemptTarget;
use super::concurrency_limits::ConcurrencyPermit;
use super::headers::filter_request_headers;
use super::passive_health::{record_passive_upstream_failure, record_passive_upstream_success};
use super::request_body::codex_session_identity_and_completed_body;
use super::request_continuity::{
    RequestContinuityClassification, RequestContinuityClassificationInput, RequestTransport,
    classify_request_continuity,
};
use super::request_failures::{FailedProxyRequestParams, finish_failed_proxy_request};
use super::request_preparation::{
    CommonRequestPreparationError, CommonRequestPreparationParams, load_request_config_context,
    prepare_common_request,
};
use super::request_routing::RequestRouteSelection;
use super::retry::{RetryPlan, retry_info_for_failed_attempts, retry_info_for_observed_attempts};
use super::route_affinity::{
    apply_session_route_affinity_for_template, record_session_route_affinity_success,
};
use super::route_attempts::{
    ErrorRouteAttemptParams, RouteAttemptErrorKind, StartRouteAttemptParams,
    StatusRouteAttemptParams, record_error_route_attempt, record_status_route_attempt,
    start_selected_route_attempt,
};
use super::route_executor_runtime::route_plan_runtime_state_from_lbs_with_overrides;
use super::route_metadata::{
    ENDPOINT_ID_TAG, PREFERENCE_GROUP_TAG, PROVIDER_ENDPOINT_KEY_TAG, PROVIDER_ID_TAG,
    ROUTE_PATH_TAG,
};
use super::route_unavailability::route_unavailable_report;
use super::selected_upstream_request::apply_selected_model_mapping;
use super::{CLIENT_NAME_HEADER, ProxyService};

const RESPONSES_WS_BETA_HEADER: &str = "responses_websockets=2026-02-06";

struct ResponsesWebSocketPrepared {
    method: Method,
    uri: Uri,
    client_headers: HeaderMap,
    session_id: Option<String>,
    session_identity_source: Option<SessionIdentitySource>,
    cwd: Option<String>,
    request_id: u64,
    started_at_ms: u64,
    start: Instant,
    route_selection: RequestRouteSelection,
    first_message: AxumWsMessage,
    body_for_upstream: axum::body::Bytes,
    request_model: Option<String>,
    effective_effort: Option<String>,
    base_service_tier: ServiceTierLog,
    codex_patch_mode: CodexPatchMode,
    request_continuity: RequestContinuityClassification,
    plan: RetryPlan,
    cooldown_backoff: crate::lb::CooldownBackoff,
}

struct ResponsesWebSocketSelected {
    target: AttemptTarget,
    legacy_lb: Option<LoadBalancer>,
    upstream_url: reqwest::Url,
    upstream_headers: HeaderMap,
    upstream_first_message: AxumWsMessage,
    provider_id: Option<String>,
    model_note: String,
    route_decision: RouteDecisionProvenance,
    route_attempts: Vec<RouteAttemptLog>,
    route_attempt_index: usize,
    route_graph_key: Option<String>,
    _concurrency_permit: Option<ConcurrencyPermit>,
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

    if !codex_provider_supports_websocket() {
        return (
            StatusCode::UPGRADE_REQUIRED,
            "Responses WebSocket is not enabled; run codex-helper switch on --mode official-relay-bridge --responses-websocket",
        )
            .into_response();
    }

    match ws {
        Ok(ws) => ws
            .on_upgrade(move |socket| async move {
                serve_responses_websocket(proxy, socket, headers, uri).await;
            })
            .into_response(),
        Err(_) => (
            StatusCode::UPGRADE_REQUIRED,
            "WebSocket upgrade required (Upgrade: websocket)",
        )
            .into_response(),
    }
}

async fn serve_responses_websocket(
    proxy: ProxyService,
    mut client_socket: WebSocket,
    client_headers: HeaderMap,
    uri: Uri,
) {
    let started_at_ms = crate::logging::now_ms();
    let start = Instant::now();
    let first_message = match read_first_data_message(&mut client_socket).await {
        Ok(message) => message,
        Err(reason) => {
            close_client_ws(client_socket, 1008, reason).await;
            return;
        }
    };

    let prepared = match prepare_responses_websocket(
        &proxy,
        uri,
        client_headers,
        first_message,
        start,
        started_at_ms,
    )
    .await
    {
        Ok(prepared) => prepared,
        Err((status, message)) => {
            close_client_ws(client_socket, close_code_for_status(status), message).await;
            return;
        }
    };

    let selected = match select_responses_websocket_target(&proxy, &prepared).await {
        Ok(selected) => selected,
        Err(failure) => {
            let _ = finish_failed_proxy_request(FailedProxyRequestParams {
                proxy: &proxy,
                method: &prepared.method,
                path: prepared.uri.path(),
                request_id: prepared.request_id,
                status: failure.status,
                message: failure.message.clone(),
                duration_ms: prepared.start.elapsed().as_millis() as u64,
                started_at_ms: prepared.started_at_ms,
                session_id: prepared.session_id.clone(),
                session_identity_source: prepared.session_identity_source,
                cwd: prepared.cwd.clone(),
                effective_effort: prepared.effective_effort.clone(),
                service_tier: prepared.base_service_tier.clone(),
                codex_bridge: None,
                retry: retry_info_for_failed_attempts(&[], &failure.route_attempts),
                failure_route_attempts: failure.route_attempts,
            })
            .await;
            close_client_ws(
                client_socket,
                close_code_for_status(failure.status),
                failure.message,
            )
            .await;
            return;
        }
    };

    proxy
        .state
        .update_request_route(
            prepared.request_id,
            selected
                .target
                .compatibility_station_name()
                .map(ToOwned::to_owned),
            selected
                .provider_id
                .clone()
                .or_else(|| selected.target.provider_id().map(ToOwned::to_owned)),
            selected.target.upstream().base_url.clone(),
            Some(selected.route_decision.clone()),
        )
        .await;

    let upstream_start = Instant::now();
    let upstream_request =
        match upstream_ws_request(selected.upstream_url.as_str(), &selected.upstream_headers) {
            Ok(request) => request,
            Err(error) => {
                let message = error.to_string();
                finish_websocket_failure(
                    &proxy,
                    &prepared,
                    selected,
                    "target_build_error",
                    message.clone(),
                )
                .await;
                close_client_ws(client_socket, 1011, message).await;
                return;
            }
        };

    let (mut upstream_socket, upstream_response) = match connect_async(upstream_request).await {
        Ok(value) => value,
        Err(error) => {
            let message = error.to_string();
            finish_websocket_failure(
                &proxy,
                &prepared,
                selected,
                "upstream_transport_error",
                message.clone(),
            )
            .await;
            close_client_ws(client_socket, 1011, message).await;
            return;
        }
    };

    let upstream_status_code = if upstream_response.status().is_success() {
        StatusCode::OK.as_u16()
    } else {
        upstream_response.status().as_u16()
    };
    let upstream_headers_ms = upstream_start.elapsed().as_millis() as u64;
    if let Err(error) = upstream_socket
        .send(axum_to_tungstenite_message(
            selected.upstream_first_message.clone(),
        ))
        .await
    {
        let message = error.to_string();
        finish_websocket_failure(
            &proxy,
            &prepared,
            selected,
            "upstream_transport_error",
            message.clone(),
        )
        .await;
        close_client_ws(client_socket, 1011, message).await;
        return;
    }

    relay_websocket_streams(
        proxy,
        prepared,
        selected,
        client_socket,
        upstream_socket,
        upstream_status_code,
        upstream_headers_ms,
    )
    .await;
}

async fn prepare_responses_websocket(
    proxy: &ProxyService,
    uri: Uri,
    client_headers: HeaderMap,
    first_message: AxumWsMessage,
    start: Instant,
    started_at_ms: u64,
) -> Result<ResponsesWebSocketPrepared, (StatusCode, String)> {
    let method = Method::GET;
    let mut client_headers = client_headers;
    let client_name = client_headers
        .get(CLIENT_NAME_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let client_addr = None;

    let config = load_request_config_context(proxy).await;

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
    let (session_identity_hint, raw_body) =
        codex_session_identity_and_completed_body(&mut client_headers, &raw_body);
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
        compact_request: false,
        session_identity_hint,
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
        Err(CommonRequestPreparationError::NoRoutableStation {
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

    Ok(ResponsesWebSocketPrepared {
        method,
        uri,
        client_headers,
        session_id: prepared.session_id,
        session_identity_source: prepared.session_identity_source,
        cwd: prepared.cwd,
        request_id: prepared.request_id,
        started_at_ms,
        start,
        route_selection: prepared.route_selection,
        first_message,
        body_for_upstream: prepared.body_for_upstream,
        request_model: prepared.request_model,
        effective_effort: prepared.effective_effort,
        base_service_tier: prepared.base_service_tier,
        codex_patch_mode: config.codex_patch_mode,
        request_continuity,
        plan: prepared.plan,
        cooldown_backoff: prepared.cooldown_backoff,
    })
}

async fn select_responses_websocket_target(
    proxy: &ProxyService,
    prepared: &ResponsesWebSocketPrepared,
) -> Result<ResponsesWebSocketSelected, ResponsesWebSocketSelectionFailure> {
    match &prepared.route_selection {
        RequestRouteSelection::RouteGraph { template } => {
            let executor = RoutePlanExecutor::new(template);
            let route_graph_key = template.route_graph_key();
            let mut runtime = proxy
                .state
                .route_plan_runtime_state_for_provider_endpoints(proxy.service_name)
                .await;
            super::provider_execution::apply_concurrency_snapshots_to_runtime(
                proxy,
                template,
                &mut runtime,
            );
            apply_session_route_affinity_for_template(
                proxy,
                prepared.session_id.as_deref(),
                template,
                &mut runtime,
            )
            .await;
            let mut route_state = RoutePlanAttemptState::default();

            if websocket_state_bound_request_requires_existing_affinity(
                prepared, &runtime, template,
            ) {
                log_websocket_route_continuity_blocked(proxy, prepared);
                return Err(ResponsesWebSocketSelectionFailure::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "state-bound compact request requires existing route affinity",
                ));
            }

            loop {
                let selection = executor.select_supported_candidate_with_runtime_state(
                    &mut route_state,
                    &runtime,
                    prepared.request_model.as_deref(),
                );
                if let Some(selected) = selection.selected {
                    let candidate = selected.candidate;
                    let concurrency_permit =
                        match super::provider_execution::try_acquire_candidate_concurrency_permit(
                            proxy,
                            executor.template(),
                            candidate,
                        ) {
                            Ok(permit) => permit,
                            Err(error) => {
                                let provider_endpoint = executor
                                    .template()
                                    .candidate_provider_endpoint_key(candidate);
                                route_state.avoid_provider_endpoint(provider_endpoint);
                                tracing::debug!(
                                    ?error,
                                    "responses websocket route candidate concurrency saturated"
                                );
                                continue;
                            }
                        };

                    let target = AttemptTarget::from_candidate(proxy.service_name, candidate);
                    return build_selected(BuildSelectedParams {
                        proxy,
                        prepared,
                        target,
                        legacy_lb: None,
                        route_graph_key: Some(route_graph_key),
                        avoided_indices: selection.avoided_candidate_indices,
                        avoided_total: selection.avoided_total,
                        total_upstreams: selection.total_upstreams,
                        concurrency_permit,
                    })
                    .await;
                }

                let mut route_attempts = Vec::new();
                if let Some(report) = route_unavailable_report(
                    proxy.service_name,
                    prepared.request_id,
                    &executor,
                    &runtime,
                    &route_state,
                    prepared.request_model.as_deref(),
                ) {
                    route_attempts.extend(report.route_attempts.clone());
                    let (status, message) = report.failure_status_message();
                    return Err(ResponsesWebSocketSelectionFailure {
                        status,
                        message,
                        route_attempts,
                    });
                }
                return Err(ResponsesWebSocketSelectionFailure::new(
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "no route candidate supports model {:?}",
                        prepared.request_model
                    ),
                ));
            }
        }
        RequestRouteSelection::Legacy { lbs } => {
            let upstream_overrides = proxy
                .state
                .get_upstream_meta_overrides(proxy.service_name)
                .await;
            let legacy_template = crate::routing_ir::compile_legacy_route_plan_template(
                proxy.service_name,
                lbs.iter().map(|lb| lb.service.as_ref()),
            );
            let executor = RoutePlanExecutor::new(&legacy_template);
            let route_graph_key = legacy_template.route_graph_key();
            let mut runtime = route_plan_runtime_state_from_lbs_with_overrides(
                proxy.service_name,
                lbs,
                &upstream_overrides,
            );
            apply_session_route_affinity_for_template(
                proxy,
                prepared.session_id.as_deref(),
                &legacy_template,
                &mut runtime,
            )
            .await;
            let mut route_state = RoutePlanAttemptState::default();

            if websocket_state_bound_request_requires_existing_affinity(
                prepared,
                &runtime,
                &legacy_template,
            ) {
                log_websocket_route_continuity_blocked(proxy, prepared);
                return Err(ResponsesWebSocketSelectionFailure::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "state-bound compact request requires existing route affinity",
                ));
            }

            let total_upstreams = legacy_template.candidates.len();
            let mut tried_stations = HashSet::new();
            let provider_attempt_limit = if lbs.len() > 1 {
                prepared.plan.route.max_attempts
            } else {
                1
            };

            for _ in 0..provider_attempt_limit {
                let lb = lbs
                    .iter()
                    .find(|lb| !tried_stations.contains(lb.service.name.as_str()));
                let Some(lb) = lb else {
                    break;
                };
                let station_name = lb.service.name.clone();

                loop {
                    let selection = executor.select_supported_station_candidate_with_runtime_state(
                        &mut route_state,
                        &runtime,
                        station_name.as_str(),
                        prepared.request_model.as_deref(),
                    );
                    let Some(selected) = selection.selected else {
                        break;
                    };

                    let selected_candidate = selected.candidate;
                    let avoided_indices =
                        route_state.route_avoid_candidate_indices(&legacy_template);
                    let selected_upstream = selected.selected_upstream;
                    let selected_index = selected_upstream.index;
                    match build_selected(BuildSelectedParams {
                        proxy,
                        prepared,
                        target: AttemptTarget::legacy(selected_upstream_with_route_metadata(
                            proxy.service_name,
                            selected_upstream.clone(),
                            selected_candidate,
                        )),
                        legacy_lb: Some(lb.clone()),
                        route_graph_key: Some(route_graph_key.clone()),
                        avoided_indices,
                        avoided_total: selection.avoided_total,
                        total_upstreams,
                        concurrency_permit: None,
                    })
                    .await
                    {
                        Ok(selected) => return Ok(selected),
                        Err(_) => {
                            route_state.avoid_upstream(station_name.as_str(), selected_index);
                            route_state.avoid_candidate(&legacy_template, selected_candidate);
                        }
                    }
                }

                tried_stations.insert(station_name);
            }

            Err(ResponsesWebSocketSelectionFailure::new(
                StatusCode::BAD_GATEWAY,
                format!("no upstream supports model {:?}", prepared.request_model),
            ))
        }
    }
}

fn websocket_state_bound_request_requires_existing_affinity(
    prepared: &ResponsesWebSocketPrepared,
    runtime: &RoutePlanRuntimeState,
    template: &RoutePlanTemplate,
) -> bool {
    prepared
        .request_continuity
        .remote_compaction_requires_affinity
        && runtime.affinity_provider_endpoint().is_none()
        && template
            .continuity_topology()
            .configured_provider_endpoint_count()
            > 1
}

fn log_websocket_route_continuity_blocked(
    proxy: &ProxyService,
    prepared: &ResponsesWebSocketPrepared,
) {
    let reason = "state_bound_compact_missing_affinity";
    log_retry_trace(serde_json::json!({
        "event": "route_continuity_blocked",
        "service": proxy.service_name,
        "request_id": prepared.request_id,
        "reason": reason,
        "continuity_class": prepared.request_continuity.class.trace_label(),
        "affinity_source": "none",
        "provider_failover_allowed": false,
        "provider_failover_blocked_reason": reason,
        "transport": "responses_websocket",
        "balance_signal_authoritative": false,
    }));
}

fn selected_upstream_with_route_metadata(
    service_name: &str,
    mut selected: crate::lb::SelectedUpstream,
    candidate: &RouteCandidate,
) -> crate::lb::SelectedUpstream {
    let provider_endpoint = crate::runtime_identity::ProviderEndpointKey::new(
        service_name,
        candidate.provider_id.clone(),
        candidate.endpoint_id.clone(),
    );
    selected
        .upstream
        .tags
        .insert(PROVIDER_ID_TAG.to_string(), candidate.provider_id.clone());
    selected
        .upstream
        .tags
        .insert(ENDPOINT_ID_TAG.to_string(), candidate.endpoint_id.clone());
    selected.upstream.tags.insert(
        PROVIDER_ENDPOINT_KEY_TAG.to_string(),
        provider_endpoint.stable_key(),
    );
    selected.upstream.tags.insert(
        PREFERENCE_GROUP_TAG.to_string(),
        candidate.preference_group.to_string(),
    );
    if let Ok(route_path) = serde_json::to_string(&candidate.route_path) {
        selected
            .upstream
            .tags
            .insert(ROUTE_PATH_TAG.to_string(), route_path);
    }
    selected
}

struct BuildSelectedParams<'a> {
    proxy: &'a ProxyService,
    prepared: &'a ResponsesWebSocketPrepared,
    target: AttemptTarget,
    legacy_lb: Option<LoadBalancer>,
    route_graph_key: Option<String>,
    avoided_indices: Vec<usize>,
    avoided_total: usize,
    total_upstreams: usize,
    concurrency_permit: Option<ConcurrencyPermit>,
}

async fn build_selected(
    params: BuildSelectedParams<'_>,
) -> Result<ResponsesWebSocketSelected, ResponsesWebSocketSelectionFailure> {
    let BuildSelectedParams {
        proxy,
        prepared,
        target,
        legacy_lb,
        route_graph_key,
        avoided_indices,
        avoided_total,
        total_upstreams,
        concurrency_permit,
    } = params;
    let (model_note, mapped_body) = apply_selected_model_mapping(
        &target,
        &prepared.body_for_upstream,
        prepared.request_model.as_deref(),
    );
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
    let mut upstream_headers = filter_request_headers(&prepared.client_headers);
    inject_auth_headers(
        proxy.service_name,
        &target.upstream().auth,
        prepared.codex_patch_mode,
        &mut upstream_headers,
    );
    upstream_headers.insert(
        HeaderName::from_static("openai-beta"),
        HeaderValue::from_static(RESPONSES_WS_BETA_HEADER),
    );
    remove_websocket_handshake_headers(&mut upstream_headers);

    let upstream_url = proxy
        .build_target(&target, &prepared.uri)
        .map_err(|error| {
            ResponsesWebSocketSelectionFailure::new(
                StatusCode::BAD_GATEWAY,
                format!("invalid upstream websocket target: {error}"),
            )
        })?
        .0;
    let upstream_url = http_url_to_ws(upstream_url).map_err(|message| {
        ResponsesWebSocketSelectionFailure::new(StatusCode::BAD_GATEWAY, message)
    })?;

    let avoid_set = avoided_indices.into_iter().collect::<HashSet<_>>();
    let mut route_attempts = Vec::new();
    let route_attempt_index = start_selected_route_attempt(
        &mut route_attempts,
        StartRouteAttemptParams {
            target: &target,
            provider_id: target.provider_id(),
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

    let route_decision = route_decision_from_model_note(model_note.as_str());

    Ok(ResponsesWebSocketSelected {
        target,
        legacy_lb,
        upstream_url,
        upstream_headers,
        upstream_first_message,
        provider_id: None,
        model_note,
        route_decision,
        route_attempts,
        route_attempt_index,
        route_graph_key,
        _concurrency_permit: concurrency_permit,
    })
}

async fn relay_websocket_streams(
    proxy: ProxyService,
    prepared: ResponsesWebSocketPrepared,
    mut selected: ResponsesWebSocketSelected,
    client_socket: WebSocket,
    upstream_socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    status_code: u16,
    upstream_headers_ms: u64,
) {
    record_attempt_success(
        proxy.state.as_ref(),
        proxy.service_name,
        selected.legacy_lb.as_ref(),
        &selected.target,
        COOLDOWN_SECS,
        prepared.cooldown_backoff,
    )
    .await;
    if let Some(station_name) = selected.target.compatibility_station_name() {
        record_passive_upstream_success(
            proxy.state.as_ref(),
            proxy.service_name,
            station_name,
            &selected.target.upstream().base_url,
            status_code,
        )
        .await;
    }

    let (mut client_sender, mut client_receiver) = client_socket.split();
    let (mut upstream_sender, mut upstream_receiver) = upstream_socket.split();
    let mut stream_error: Option<String> = None;

    loop {
        tokio::select! {
            client_message = client_receiver.next() => {
                match client_message {
                    Some(Ok(message)) => {
                        if matches!(message, AxumWsMessage::Close(_)) {
                            let _ = upstream_sender.send(axum_to_tungstenite_message(message)).await;
                            break;
                        }
                        if let Err(error) = upstream_sender.send(axum_to_tungstenite_message(message)).await {
                            stream_error = Some(error.to_string());
                            break;
                        }
                    }
                    Some(Err(error)) => {
                        stream_error = Some(error.to_string());
                        break;
                    }
                    None => break,
                }
            }
            upstream_message = upstream_receiver.next() => {
                match upstream_message {
                    Some(Ok(message)) => {
                        if matches!(message, tungstenite::Message::Close(_)) {
                            let _ = client_sender.send(tungstenite_to_axum_message(message)).await;
                            break;
                        }
                        if let Err(error) = client_sender.send(tungstenite_to_axum_message(message)).await {
                            stream_error = Some(error.to_string());
                            break;
                        }
                    }
                    Some(Err(error)) => {
                        stream_error = Some(error.to_string());
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    let duration_ms = prepared.start.elapsed().as_millis() as u64;
    let error_class = stream_error.as_ref().map(|_| "upstream_stream_error");
    let mut upstream_chain = Vec::new();
    record_status_route_attempt(
        &mut upstream_chain,
        &mut selected.route_attempts,
        StatusRouteAttemptParams {
            target: &selected.target,
            route_attempt_index: selected.route_attempt_index,
            status_code,
            error_class,
            model_note: selected.model_note.as_str(),
            upstream_headers_ms,
            duration_ms,
            cooldown_secs: None,
            cooldown_reason: None,
        },
    );
    record_session_route_affinity_success(
        &proxy,
        prepared.session_id.as_deref(),
        prepared.session_identity_source,
        selected.route_graph_key.as_deref(),
        &selected.target,
        &selected.route_attempts,
        selected.route_attempt_index,
    )
    .await;
    let retry = retry_info_for_observed_attempts(&upstream_chain, &selected.route_attempts);
    let service_tier = ServiceTierLog {
        actual: None,
        ..prepared.base_service_tier.clone()
    };
    let codex_bridge = (!prepared.codex_patch_mode.is_default()
        || prepared.request_continuity.is_remote_compaction_v2_request)
        .then(|| CodexBridgeLog {
            patch_mode: prepared.codex_patch_mode.as_str().to_string(),
            remote_compaction_v1_request: false,
            remote_compaction_v2_request: prepared
                .request_continuity
                .is_remote_compaction_v2_request,
            responses_websocket_request: true,
            strips_client_auth: prepared.codex_patch_mode.strips_codex_client_auth(),
        });

    crate::logging::log_request_with_debug(
        Some(prepared.request_id),
        proxy.service_name,
        prepared.method.as_str(),
        prepared.uri.path(),
        status_code,
        duration_ms,
        Some(upstream_headers_ms),
        selected.target.compatibility_station_name(),
        selected
            .provider_id
            .or_else(|| selected.target.provider_id().map(ToOwned::to_owned)),
        selected.target.endpoint_id(),
        selected.target.provider_endpoint_key(),
        selected.target.upstream().base_url.as_str(),
        prepared.session_id.clone(),
        prepared.session_identity_source,
        prepared.cwd.clone(),
        selected
            .route_decision
            .effective_model
            .as_ref()
            .map(|model| model.value.clone()),
        prepared.effective_effort.clone(),
        service_tier.clone(),
        codex_bridge,
        None,
        Some(selected.route_decision.clone()),
        retry.clone(),
        None,
    );

    proxy
        .state
        .finish_request(FinishRequestParams {
            id: prepared.request_id,
            status_code,
            duration_ms,
            ended_at_ms: prepared.started_at_ms + duration_ms,
            observed_service_tier: service_tier.actual,
            usage: None,
            retry,
            ttfb_ms: Some(upstream_headers_ms),
            streaming: true,
        })
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
                    RouteValueSource::StationMapping
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
    error_class: &'static str,
    message: String,
) {
    let mut upstream_chain = Vec::new();
    record_error_route_attempt(
        &mut upstream_chain,
        &mut selected.route_attempts,
        ErrorRouteAttemptParams {
            target: &selected.target,
            route_attempt_index: selected.route_attempt_index,
            kind: RouteAttemptErrorKind::Transport,
            reason: message.as_str(),
            model_note: selected.model_note.as_str(),
            duration_ms: Some(prepared.start.elapsed().as_millis() as u64),
            cooldown_secs: Some(prepared.plan.transport_cooldown_secs),
            cooldown_reason: Some(error_class),
        },
    );
    let mut avoid_set = HashSet::new();
    let mut avoided_total = 0;
    let mut last_err = None;
    apply_terminal_upstream_failure(TerminalUpstreamFailureParams {
        proxy,
        lb: selected.legacy_lb.as_ref(),
        target: &selected.target,
        error_class,
        penalize_reason: Some(error_class),
        cooldown_secs: prepared.plan.transport_cooldown_secs,
        cooldown_backoff: prepared.cooldown_backoff,
        error_message: message.clone(),
        avoid_set: &mut avoid_set,
        avoided_total: &mut avoided_total,
        last_err: &mut last_err,
    })
    .await;
    if let Some(station_name) = selected.target.compatibility_station_name() {
        record_passive_upstream_failure(
            proxy.state.as_ref(),
            proxy.service_name,
            station_name,
            &selected.target.upstream().base_url,
            Some(StatusCode::BAD_GATEWAY.as_u16()),
            Some(error_class),
            Some(message.clone()),
        )
        .await;
    }
    let retry = retry_info_for_failed_attempts(&upstream_chain, &selected.route_attempts);
    let _ = finish_failed_proxy_request(FailedProxyRequestParams {
        proxy,
        method: &prepared.method,
        path: prepared.uri.path(),
        request_id: prepared.request_id,
        status: StatusCode::BAD_GATEWAY,
        message,
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
    })
    .await;
}

async fn read_first_data_message(socket: &mut WebSocket) -> Result<AxumWsMessage, String> {
    loop {
        let Some(message) = socket.recv().await else {
            return Err("websocket closed before first response.create".to_string());
        };
        let message = message.map_err(|error| error.to_string())?;
        match message {
            AxumWsMessage::Text(_) | AxumWsMessage::Binary(_) => return Ok(message),
            AxumWsMessage::Close(_) => {
                return Err("websocket closed before first response.create".to_string());
            }
            AxumWsMessage::Ping(_) | AxumWsMessage::Pong(_) => continue,
        }
    }
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

fn remove_websocket_handshake_headers(headers: &mut HeaderMap) {
    for name in [
        header::CONNECTION,
        header::UPGRADE,
        header::HOST,
        HeaderName::from_static("sec-websocket-key"),
        HeaderName::from_static("sec-websocket-version"),
        HeaderName::from_static("sec-websocket-accept"),
        HeaderName::from_static("sec-websocket-extensions"),
    ] {
        headers.remove(name);
    }
}

fn upstream_ws_request(
    url: &str,
    headers: &HeaderMap,
) -> Result<tungstenite_http::Request<()>, tungstenite::Error> {
    let mut request = url.into_client_request()?;
    for (name, value) in headers {
        if let (Ok(name), Ok(value)) = (
            tungstenite_http::HeaderName::from_bytes(name.as_str().as_bytes()),
            tungstenite_http::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            request.headers_mut().append(name, value);
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

async fn close_client_ws(mut socket: WebSocket, code: u16, reason: String) {
    let _ = socket
        .send(AxumWsMessage::Close(Some(AxumCloseFrame {
            code,
            reason: Utf8Bytes::from(reason),
        })))
        .await;
}

fn close_code_for_status(status: StatusCode) -> u16 {
    if status == StatusCode::BAD_REQUEST {
        1008
    } else {
        1011
    }
}

fn codex_provider_supports_websocket() -> bool {
    crate::codex_integration::codex_switch_status()
        .ok()
        .and_then(|status| status.supports_websockets)
        .unwrap_or(false)
}
