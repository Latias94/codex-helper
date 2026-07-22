use axum::http::StatusCode;

use crate::config::RouteStrategy;
use crate::dashboard_core::OperatorSessionRouteAffinitySummary;
use crate::routing_ir::{RouteCandidate, RoutePlanTemplate};
use crate::state::{
    SessionRouteAffinity, SessionRouteAffinityControlCommand, SessionRouteAffinityControlStatus,
    SessionRouteAffinityTarget,
};

use super::{ProxyControlError, ProxyService};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum OperatorSessionAffinityCommand {
    Rebind {
        provider_id: String,
        endpoint_id: String,
    },
    Clear,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OperatorSessionAffinityMutationRequest {
    pub session_key: String,
    pub expected_affinity_revision: Option<String>,
    pub command: OperatorSessionAffinityCommand,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorSessionAffinityMutationStatus {
    Applied,
    Unchanged,
    Conflict,
    Busy,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OperatorSessionAffinityMutationResponse {
    pub status: OperatorSessionAffinityMutationStatus,
    pub session_key: String,
    pub route_affinity: Option<OperatorSessionRouteAffinitySummary>,
}

pub(super) async fn mutate_operator_session_affinity(
    proxy: &ProxyService,
    request: OperatorSessionAffinityMutationRequest,
) -> Result<OperatorSessionAffinityMutationResponse, ProxyControlError> {
    let session_key = required_value(request.session_key.as_str(), "session_key")?;
    if request
        .expected_affinity_revision
        .as_deref()
        .is_some_and(|revision| revision.trim().is_empty())
    {
        return Err(bad_request("expected_affinity_revision is empty"));
    }

    let operator_capture = proxy.operator_read_capture().await?;
    let session_id = operator_capture
        .local_sessions
        .get(session_key)
        .map(|session| session.raw_session_id.clone())
        .ok_or_else(|| {
            ProxyControlError::new(
                StatusCode::NOT_FOUND,
                "operator session key is not present in the current local read model",
            )
        })?;

    let route_control_guard = proxy.state.lock_session_route_control(&session_id).await;
    let runtime_snapshot = proxy.config.capture().await;
    let graph = runtime_snapshot
        .route_graph(proxy.service_name)
        .ok_or_else(|| conflict("runtime snapshot has no route graph for the service"))?;
    let template = graph.handshake_plan();
    let current = proxy.state.get_session_route_affinity(&session_id).await;

    let command = match &request.command {
        OperatorSessionAffinityCommand::Clear => SessionRouteAffinityControlCommand::Clear,
        OperatorSessionAffinityCommand::Rebind {
            provider_id,
            endpoint_id,
        } => {
            let route_graph_key = template.route_graph_key();
            if template
                .nodes
                .values()
                .any(|node| node.strategy == RouteStrategy::Conditional)
            {
                return Err(conflict(
                    "ambiguous_conditional_topology: session affinity rebind is unavailable for conditional routes",
                ));
            }
            let current = current.as_ref().ok_or_else(|| {
                conflict("session has no route affinity to rebind; allow automatic first selection")
            })?;
            if current.route_graph_key != route_graph_key {
                return Ok(response(
                    OperatorSessionAffinityMutationStatus::Conflict,
                    session_key,
                    Some(current),
                ));
            }
            let target_candidate = candidate(
                &template,
                required_value(provider_id, "provider_id")?,
                required_value(endpoint_id, "endpoint_id")?,
            )?;
            validate_rebind_continuity(&template, current, target_candidate)?;
            validate_target_available(
                proxy,
                runtime_snapshot.as_ref(),
                &template,
                target_candidate,
            )
            .await?;
            SessionRouteAffinityControlCommand::Rebind(SessionRouteAffinityTarget {
                route_graph_key,
                session_identity_source: current.session_identity_source,
                provider_endpoint: template.candidate_provider_endpoint_key(target_candidate),
                upstream_base_url: target_candidate.base_url.clone(),
                route_path: target_candidate.route_path.clone(),
            })
        }
    };

    let commit = proxy
        .state
        .compare_and_mutate_session_route_affinity_with_control(
            &route_control_guard,
            request.expected_affinity_revision.as_deref(),
            command,
            crate::logging::now_ms(),
        )
        .await
        .map_err(|error| {
            tracing::error!(
                session_key,
                error = %error,
                "failed to commit operator session affinity mutation"
            );
            ProxyControlError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session affinity mutation unavailable",
            )
        })?;
    let status = commit.status.into();
    Ok(response(status, session_key, commit.affinity.as_ref()))
}

fn candidate<'a>(
    template: &'a RoutePlanTemplate,
    provider_id: &str,
    endpoint_id: &str,
) -> Result<&'a RouteCandidate, ProxyControlError> {
    template
        .candidates
        .iter()
        .find(|candidate| {
            candidate.provider_id == provider_id && candidate.endpoint_id == endpoint_id
        })
        .ok_or_else(|| conflict("session affinity target is not a compiled route candidate"))
}

fn validate_rebind_continuity(
    template: &RoutePlanTemplate,
    current: &SessionRouteAffinity,
    target: &RouteCandidate,
) -> Result<(), ProxyControlError> {
    let target_key = template.candidate_provider_endpoint_key(target);
    if target_key == current.provider_endpoint {
        return Ok(());
    }
    let current_candidate = template
        .candidates
        .iter()
        .find(|candidate| {
            template.candidate_provider_endpoint_key(candidate) == current.provider_endpoint
                && candidate.base_url == current.upstream_base_url
        })
        .or_else(|| {
            template.candidates.iter().find(|candidate| {
                template.candidate_provider_endpoint_key(candidate) == current.provider_endpoint
            })
        })
        .ok_or_else(|| conflict("current session affinity is not present in the route graph"))?;
    let current_domain = template.candidate_continuity_domain_key(current_candidate);
    let target_domain = template.candidate_continuity_domain_key(target);
    if !current_domain.is_explicit() || current_domain != target_domain {
        return Err(conflict(
            "continuity_domain_mismatch: cross-endpoint rebind requires the same explicit continuity domain",
        ));
    }
    Ok(())
}

async fn validate_target_available(
    proxy: &ProxyService,
    runtime_snapshot: &super::runtime_config::RuntimeSnapshot,
    template: &RoutePlanTemplate,
    candidate: &RouteCandidate,
) -> Result<(), ProxyControlError> {
    let provider_policy = runtime_snapshot.provider_policy();
    let runtime_identities = template.candidate_identities().map_err(|error| {
        ProxyControlError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("captured runtime credential binding is invalid: {error}"),
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
    super::route_target_selection::apply_auth_resolution_to_runtime(
        proxy.service_name,
        template,
        &mut runtime,
    )
    .map_err(|error| {
        ProxyControlError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("captured runtime credential binding is invalid: {error}"),
        )
    })?;
    super::route_target_selection::apply_concurrency_snapshots_to_runtime(
        proxy,
        template,
        runtime_snapshot.revision(),
        &mut runtime,
    );
    if !runtime
        .candidate_runtime_snapshot(template, candidate)
        .runtime_available
    {
        return Err(conflict(
            "session affinity target is not currently available for a new binding",
        ));
    }
    Ok(())
}

fn response(
    status: OperatorSessionAffinityMutationStatus,
    session_key: &str,
    affinity: Option<&SessionRouteAffinity>,
) -> OperatorSessionAffinityMutationResponse {
    OperatorSessionAffinityMutationResponse {
        status,
        session_key: session_key.to_string(),
        route_affinity: affinity.and_then(OperatorSessionRouteAffinitySummary::from_affinity),
    }
}

fn required_value<'a>(value: &'a str, field: &str) -> Result<&'a str, ProxyControlError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(bad_request(format!("{field} is empty")));
    }
    Ok(value)
}

fn bad_request(message: impl Into<String>) -> ProxyControlError {
    ProxyControlError::new(StatusCode::BAD_REQUEST, message)
}

fn conflict(message: impl Into<String>) -> ProxyControlError {
    ProxyControlError::new(StatusCode::CONFLICT, message)
}

impl From<SessionRouteAffinityControlStatus> for OperatorSessionAffinityMutationStatus {
    fn from(value: SessionRouteAffinityControlStatus) -> Self {
        match value {
            SessionRouteAffinityControlStatus::Applied => Self::Applied,
            SessionRouteAffinityControlStatus::Unchanged => Self::Unchanged,
            SessionRouteAffinityControlStatus::Conflict => Self::Conflict,
            SessionRouteAffinityControlStatus::Busy => Self::Busy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_expected_route_key_is_ignored_during_deserialization() {
        let request: OperatorSessionAffinityMutationRequest =
            serde_json::from_value(serde_json::json!({
                "session_key": "session:sha256:test",
                "expected_affinity_route_key": "legacy-internal-route-key",
                "expected_affinity_revision": "affinity:v1:test",
                "command": {
                    "command": "clear"
                }
            }))
            .expect("deserialize request with retired route key");

        assert_eq!(request.session_key, "session:sha256:test");
        assert_eq!(
            request.expected_affinity_revision.as_deref(),
            Some("affinity:v1:test")
        );
        assert_eq!(request.command, OperatorSessionAffinityCommand::Clear);
    }
}
