use std::net::SocketAddr;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Request, Response, StatusCode};
use axum::middleware::{self, Next};
use axum::routing::post;
use axum::{Json, Router};

use super::admin::{AdminAccessConfig, require_admin_access};
use super::admin_api_error::{AdminApiHttpError, AdminApiResult};
use super::control_plane_manifest::{
    LOCAL_V1_BALANCE_REFRESH, LOCAL_V1_CREDENTIAL_REFRESH, LOCAL_V1_DEFAULT_PROFILE_MUTATION,
    LOCAL_V1_OPERATOR_SESSION, LOCAL_V1_RELAY_CAPABILITIES, LOCAL_V1_RELAY_LIVE_SMOKE,
    LOCAL_V1_ROUTING_MUTATION, LOCAL_V1_RUNTIME_RELOAD, LOCAL_V1_RUNTIME_SHUTDOWN,
    LOCAL_V1_SERVICE_RUNTIME_READ, LOCAL_V1_SESSION_AFFINITY_MUTATION,
    LOCAL_V1_SESSION_BINDING_MUTATION, LOCAL_V1_SESSION_METADATA_READ,
};
use super::{
    CodexRelayCapabilitiesRequest, CodexRelayLiveSmokeRequest,
    OperatorDefaultProfileMutationRequest, OperatorRoutingMutationRequest,
    OperatorRuntimeReloadRequest, OperatorSessionAffinityMutationRequest,
    OperatorSessionBindingMutationRequest, ProviderBalanceRefreshResponse, ProxyService,
};

pub(crate) const LOCAL_OPERATOR_SESSION_HEADER: &str = "x-codex-helper-local-session";
pub(crate) const LOCAL_OPERATOR_NONCE_HEADER: &str = "x-codex-helper-local-nonce";
pub(crate) const LOCAL_OPERATOR_TIMESTAMP_HEADER: &str = "x-codex-helper-local-timestamp";
pub(crate) const LOCAL_OPERATOR_SIGNATURE_HEADER: &str = "x-codex-helper-local-signature";

const MAX_LOCAL_SESSION_KEY_BYTES: usize = 128;

#[derive(Debug, serde::Deserialize)]
pub(crate) struct LocalBalanceRefreshRequest {
    pub force: bool,
}

#[derive(Clone)]
struct LocalOperatorRouteState {
    proxy: ProxyService,
    sessions: crate::local_operator::LocalOperatorSessionStore,
}

pub(super) fn local_operator_routes(proxy: ProxyService) -> Router {
    let state = LocalOperatorRouteState {
        proxy,
        sessions: crate::local_operator::LocalOperatorSessionStore::default(),
    };
    Router::new()
        .route(LOCAL_V1_OPERATOR_SESSION, post(begin_session))
        .route(LOCAL_V1_BALANCE_REFRESH, post(refresh_balances))
        .route(LOCAL_V1_CREDENTIAL_REFRESH, post(refresh_credential))
        .route(LOCAL_V1_SERVICE_RUNTIME_READ, post(read_service_runtime))
        .route(LOCAL_V1_SESSION_METADATA_READ, post(read_session_metadata))
        .route(LOCAL_V1_ROUTING_MUTATION, post(mutate_routing))
        .route(
            LOCAL_V1_SESSION_AFFINITY_MUTATION,
            post(mutate_session_affinity),
        )
        .route(
            LOCAL_V1_SESSION_BINDING_MUTATION,
            post(mutate_session_binding),
        )
        .route(
            LOCAL_V1_DEFAULT_PROFILE_MUTATION,
            post(mutate_default_profile),
        )
        .route(LOCAL_V1_RUNTIME_RELOAD, post(reload_runtime))
        .route(LOCAL_V1_RUNTIME_SHUTDOWN, post(shutdown_runtime))
        .route(
            LOCAL_V1_RELAY_CAPABILITIES,
            post(inspect_relay_capabilities),
        )
        .route(LOCAL_V1_RELAY_LIVE_SMOKE, post(run_relay_live_smoke))
        .with_state(state)
        .layer(middleware::from_fn(require_local_operator_loopback))
        .layer(middleware::from_fn_with_state(
            AdminAccessConfig::from_env(),
            require_admin_access,
        ))
}

async fn read_session_metadata(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<crate::dashboard_core::LocalOperatorSessionMetadataResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_SESSION_METADATA_READ, &body)?;
    let request =
        serde_json::from_slice::<crate::dashboard_core::LocalOperatorSessionMetadataRequest>(&body)
            .map_err(|_| {
                AdminApiHttpError::bad_request(
                    "local_operator_invalid_json",
                    "invalid local operator session metadata request",
                )
            })?;
    if request.session_keys.len() > crate::dashboard_core::LOCAL_OPERATOR_SESSION_METADATA_BATCH_MAX
        || request.session_keys.iter().any(|key| {
            key.is_empty()
                || key.len() > MAX_LOCAL_SESSION_KEY_BYTES
                || !key.starts_with("session:")
        })
    {
        return Err(AdminApiHttpError::bad_request(
            "local_operator_invalid_session_keys",
            "local operator session metadata request contains invalid session keys",
        ));
    }
    let requested = request
        .session_keys
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let capture = state
        .proxy
        .operator_read_capture()
        .await
        .map_err(AdminApiHttpError::from)?;
    let sessions = capture
        .local_sessions
        .into_iter()
        .filter(|(key, _)| requested.contains(key))
        .collect();
    Ok(Json(
        crate::dashboard_core::LocalOperatorSessionMetadataResponse {
            service_name: state.proxy.service_name.to_string(),
            sessions,
        },
    ))
}

async fn read_service_runtime(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<crate::service_target::LocalServiceRuntimeReadResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_SERVICE_RUNTIME_READ, &body)?;
    let request =
        serde_json::from_slice::<crate::service_target::LocalServiceRuntimeReadRequest>(&body)
            .map_err(|_| {
                AdminApiHttpError::bad_request(
                    "local_operator_invalid_json",
                    "invalid local operator service runtime request",
                )
            })?;
    let Some(identity) = state.proxy.service_runtime_identity() else {
        return Err(service_generation_conflict());
    };
    if request.service != identity.service
        || request.install_generation != identity.install_generation
    {
        return Err(service_generation_conflict());
    }
    let operator = state
        .proxy
        .operator_read_model()
        .await
        .map_err(AdminApiHttpError::from)?;
    let credential_readiness = operator
        .data
        .as_ref()
        .and_then(|data| data.summary.credential_readiness)
        .unwrap_or(crate::credentials::CredentialAggregateReadiness::Blocked);
    Ok(Json(
        crate::service_target::LocalServiceRuntimeReadResponse {
            identity: identity.clone(),
            credential_readiness,
            operator,
        },
    ))
}

async fn refresh_credential(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<crate::service_target::LocalCredentialRefreshResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_CREDENTIAL_REFRESH, &body)?;
    let request =
        serde_json::from_slice::<crate::service_target::LocalCredentialRefreshRequest>(&body)
            .map_err(|_| {
                AdminApiHttpError::bad_request(
                    "local_operator_invalid_json",
                    "invalid local operator credential refresh request",
                )
            })?;
    let service_matches =
        crate::runtime_host::service_name_for_kind(request.service) == state.proxy.service_name;
    let Some(install_generation) = state.proxy.service_install_generation() else {
        return Err(service_generation_conflict());
    };
    if !service_matches || request.install_generation != *install_generation {
        return Err(service_generation_conflict());
    }

    let (status, runtime_revision) = state
        .proxy
        .refresh_native_credential(&request.credential_name, request.action)
        .await
        .map_err(AdminApiHttpError::from)?;
    Ok(Json(
        crate::service_target::LocalCredentialRefreshResponse {
            service: request.service,
            install_generation: install_generation.clone(),
            status,
            runtime_revision,
        },
    ))
}

async fn begin_session(
    State(state): State<LocalOperatorRouteState>,
    Json(request): Json<crate::local_operator::LocalOperatorSessionRequest>,
) -> AdminApiResult<crate::local_operator::LocalOperatorSessionResponse> {
    let token = required_local_operator_token()?;
    state
        .sessions
        .issue(&token, &request)
        .map(Json)
        .map_err(|error| {
            AdminApiHttpError::bad_request(
                "local_operator_session_rejected",
                format!("local operator session rejected: {error}"),
            )
        })
}

async fn refresh_balances(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<ProviderBalanceRefreshResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_BALANCE_REFRESH, &body)?;
    let request = serde_json::from_slice::<LocalBalanceRefreshRequest>(&body).map_err(|error| {
        AdminApiHttpError::bad_request(
            "local_operator_invalid_json",
            format!("invalid local operator balance request: {error}"),
        )
    })?;
    state
        .proxy
        .refresh_provider_balances(None, None, request.force)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn mutate_routing(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<super::OperatorRoutingMutationResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_ROUTING_MUTATION, &body)?;
    let request =
        serde_json::from_slice::<OperatorRoutingMutationRequest>(&body).map_err(|error| {
            AdminApiHttpError::bad_request(
                "local_operator_invalid_json",
                format!("invalid local operator routing request: {error}"),
            )
        })?;
    state
        .proxy
        .mutate_operator_routing(request)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn mutate_session_affinity(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<super::OperatorSessionAffinityMutationResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_SESSION_AFFINITY_MUTATION, &body)?;
    let request = serde_json::from_slice::<OperatorSessionAffinityMutationRequest>(&body).map_err(
        |error| {
            AdminApiHttpError::bad_request(
                "local_operator_invalid_json",
                format!("invalid local operator session affinity request: {error}"),
            )
        },
    )?;
    state
        .proxy
        .mutate_operator_session_affinity(request)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn mutate_session_binding(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<super::OperatorSessionBindingMutationResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_SESSION_BINDING_MUTATION, &body)?;
    let request = serde_json::from_slice::<OperatorSessionBindingMutationRequest>(&body).map_err(
        |error| {
            AdminApiHttpError::bad_request(
                "local_operator_invalid_json",
                format!("invalid local operator session binding request: {error}"),
            )
        },
    )?;
    state
        .proxy
        .mutate_operator_session_binding(request)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn mutate_default_profile(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<super::OperatorDefaultProfileMutationResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_DEFAULT_PROFILE_MUTATION, &body)?;
    let request = serde_json::from_slice::<OperatorDefaultProfileMutationRequest>(&body).map_err(
        |error| {
            AdminApiHttpError::bad_request(
                "local_operator_invalid_json",
                format!("invalid local operator default profile request: {error}"),
            )
        },
    )?;
    state
        .proxy
        .mutate_operator_default_profile(request)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn reload_runtime(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<super::OperatorRuntimeReloadResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_RUNTIME_RELOAD, &body)?;
    let request =
        serde_json::from_slice::<OperatorRuntimeReloadRequest>(&body).map_err(|error| {
            AdminApiHttpError::bad_request(
                "local_operator_invalid_json",
                format!("invalid local operator runtime reload request: {error}"),
            )
        })?;
    state
        .proxy
        .operator_runtime_reload(request)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn shutdown_runtime(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<crate::local_operator::LocalRuntimeShutdownResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_RUNTIME_SHUTDOWN, &body)?;
    let request =
        serde_json::from_slice::<crate::local_operator::LocalRuntimeShutdownRequest>(&body)
            .map_err(|error| {
                AdminApiHttpError::bad_request(
                    "local_operator_invalid_json",
                    format!("invalid local operator runtime shutdown request: {error}"),
                )
            })?;
    state
        .proxy
        .request_local_runtime_shutdown(&request)
        .map(Json)
        .map_err(Into::into)
}

async fn inspect_relay_capabilities(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<super::CodexRelayCapabilitiesResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_RELAY_CAPABILITIES, &body)?;
    let request =
        serde_json::from_slice::<CodexRelayCapabilitiesRequest>(&body).map_err(|error| {
            AdminApiHttpError::bad_request(
                "local_operator_invalid_json",
                format!("invalid local operator relay capabilities request: {error}"),
            )
        })?;
    state
        .proxy
        .codex_relay_capabilities(request)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn run_relay_live_smoke(
    State(state): State<LocalOperatorRouteState>,
    headers: HeaderMap,
    body: Bytes,
) -> AdminApiResult<super::CodexRelayLiveSmokeResponse> {
    authorize_local_operator_action(&state, &headers, LOCAL_V1_RELAY_LIVE_SMOKE, &body)?;
    let request = serde_json::from_slice::<CodexRelayLiveSmokeRequest>(&body).map_err(|error| {
        AdminApiHttpError::bad_request(
            "local_operator_invalid_json",
            format!("invalid local operator relay live-smoke request: {error}"),
        )
    })?;
    state
        .proxy
        .codex_relay_live_smoke(request)
        .await
        .map(Json)
        .map_err(Into::into)
}

fn authorize_local_operator_action(
    state: &LocalOperatorRouteState,
    headers: &HeaderMap,
    path: &str,
    body: &[u8],
) -> Result<(), AdminApiHttpError> {
    let session_id = required_header(headers, LOCAL_OPERATOR_SESSION_HEADER)?;
    let request_nonce = required_header(headers, LOCAL_OPERATOR_NONCE_HEADER)?;
    let timestamp = required_header(headers, LOCAL_OPERATOR_TIMESTAMP_HEADER)?
        .parse::<u64>()
        .map_err(|_| forbidden_local_operator_action())?;
    let signature = required_header(headers, LOCAL_OPERATOR_SIGNATURE_HEADER)?;
    let token = required_local_operator_token()?;
    state
        .sessions
        .verify_and_consume(
            &token,
            session_id,
            request_nonce,
            timestamp,
            path,
            body,
            signature,
        )
        .map_err(|_| forbidden_local_operator_action())
}

fn required_header<'a>(
    headers: &'a HeaderMap,
    name: &'static str,
) -> Result<&'a str, AdminApiHttpError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(forbidden_local_operator_action)
}

fn required_local_operator_token() -> Result<String, AdminApiHttpError> {
    crate::local_operator::read_local_operator_token()
        .map_err(|_| {
            AdminApiHttpError::internal(
                "local_operator_unavailable",
                "local operator capability is unavailable",
            )
        })?
        .ok_or_else(|| {
            AdminApiHttpError::internal(
                "local_operator_unavailable",
                "local operator capability is unavailable",
            )
        })
}

fn forbidden_local_operator_action() -> AdminApiHttpError {
    AdminApiHttpError::new(
        StatusCode::FORBIDDEN,
        "local_operator_forbidden",
        "valid local operator proof is required",
    )
}

fn service_generation_conflict() -> AdminApiHttpError {
    AdminApiHttpError::new(
        StatusCode::CONFLICT,
        "local_operator_service_generation_mismatch",
        "local operator target does not match this service generation",
    )
}

async fn require_local_operator_loopback(
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: Next,
) -> Result<Response<Body>, (StatusCode, &'static str)> {
    if !peer_addr.ip().is_loopback() {
        return Err((
            StatusCode::FORBIDDEN,
            "local operator actions require a loopback peer",
        ));
    }
    Ok(next.run(request).await)
}
