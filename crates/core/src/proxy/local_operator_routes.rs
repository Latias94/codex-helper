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
    LOCAL_V1_BALANCE_REFRESH, LOCAL_V1_CREDENTIAL_REFRESH, LOCAL_V1_OPERATOR_SESSION,
    LOCAL_V1_ROUTING_MUTATION, LOCAL_V1_SERVICE_RUNTIME_READ, LOCAL_V1_SESSION_AFFINITY_MUTATION,
};
use super::{
    OperatorRoutingMutationRequest, OperatorSessionAffinityMutationRequest,
    ProviderBalanceRefreshResponse, ProxyService,
};

pub(crate) const LOCAL_OPERATOR_SESSION_HEADER: &str = "x-codex-helper-local-session";
pub(crate) const LOCAL_OPERATOR_NONCE_HEADER: &str = "x-codex-helper-local-nonce";
pub(crate) const LOCAL_OPERATOR_TIMESTAMP_HEADER: &str = "x-codex-helper-local-timestamp";
pub(crate) const LOCAL_OPERATOR_SIGNATURE_HEADER: &str = "x-codex-helper-local-signature";

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
        .route(LOCAL_V1_ROUTING_MUTATION, post(mutate_routing))
        .route(
            LOCAL_V1_SESSION_AFFINITY_MUTATION,
            post(mutate_session_affinity),
        )
        .with_state(state)
        .layer(middleware::from_fn(require_local_operator_loopback))
        .layer(middleware::from_fn_with_state(
            AdminAccessConfig::from_env(),
            require_admin_access,
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
