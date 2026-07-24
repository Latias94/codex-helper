use std::sync::Arc;

use anyhow::{Context, Result};
use axum::http::StatusCode;

use crate::dashboard_core::{
    OperatorRoutingControlView, OperatorRoutingSummary, build_operator_routing_summary,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::runtime_store::{ProviderManualEligibility, ProviderPolicySnapshot};
use crate::state::{
    ProviderManualEligibilityUpdate, RoutingOperatorControlSnapshot, RoutingOperatorControlUpdate,
};

use super::{ProxyControlError, ProxyService};

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorEndpointMode {
    Enabled,
    Draining,
    Disabled,
}

impl OperatorEndpointMode {
    fn manual_eligibility(self) -> ProviderManualEligibility {
        match self {
            Self::Enabled => ProviderManualEligibility::Enabled,
            Self::Draining => ProviderManualEligibility::Draining,
            Self::Disabled => ProviderManualEligibility::Disabled,
        }
    }

    fn audit_reason(self) -> Option<String> {
        match self {
            Self::Enabled => None,
            Self::Draining => Some("local operator requested endpoint drain".to_string()),
            Self::Disabled => Some("local operator disabled endpoint".to_string()),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum OperatorRoutingCommand {
    SetNewSessionPreference {
        provider_id: String,
        endpoint_id: String,
    },
    ClearNewSessionPreference,
    SetEndpointMode {
        provider_id: String,
        endpoint_id: String,
        mode: OperatorEndpointMode,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OperatorRoutingMutationRequest {
    pub expected_route_graph_key: String,
    pub expected_control_revision: u64,
    pub expected_policy_revision: u64,
    pub command: OperatorRoutingCommand,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRoutingMutationStatus {
    Applied,
    Unchanged,
    Conflict,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct OperatorRoutingMutationResponse {
    pub status: OperatorRoutingMutationStatus,
    pub routing: OperatorRoutingSummary,
}

pub(super) async fn mutate_operator_routing(
    proxy: &ProxyService,
    request: OperatorRoutingMutationRequest,
) -> Result<OperatorRoutingMutationResponse, ProxyControlError> {
    let runtime_snapshot = proxy.config.capture().await;
    let config = runtime_snapshot.config();
    let view =
        super::control_plane_service::service_route_config(config.as_ref(), proxy.service_name);
    let graph = runtime_snapshot
        .route_graph(proxy.service_name)
        .ok_or_else(|| control_error("runtime snapshot has no route graph for the service"))?;
    let route_graph_key = graph.digest();
    let template = graph.handshake_plan();
    let control = proxy.state.capture_routing_operator_control().await;
    let provider_policy = proxy.state.capture_provider_policy_snapshot().await;
    let current = summarize_routing(
        view,
        &template,
        route_graph_key,
        &control,
        provider_policy.as_ref(),
        proxy.service_name,
    )?;

    if request.expected_route_graph_key != route_graph_key {
        return Ok(OperatorRoutingMutationResponse {
            status: OperatorRoutingMutationStatus::Conflict,
            routing: current,
        });
    }

    let status = match &request.command {
        OperatorRoutingCommand::SetNewSessionPreference {
            provider_id,
            endpoint_id,
        } => {
            if request.expected_policy_revision != provider_policy.policy_revision {
                return Ok(OperatorRoutingMutationResponse {
                    status: OperatorRoutingMutationStatus::Conflict,
                    routing: current,
                });
            }
            let target = candidate_key(proxy.service_name, &template, provider_id, endpoint_id)?;
            proxy
                .state
                .compare_and_set_new_session_preference_for_policy(
                    proxy.service_name,
                    route_graph_key,
                    request.expected_control_revision,
                    request.expected_policy_revision,
                    Some(target),
                )
                .await
                .map_err(|error| control_error(error.to_string()))?
                .status
                .into()
        }
        OperatorRoutingCommand::ClearNewSessionPreference => proxy
            .state
            .compare_and_set_new_session_preference(
                proxy.service_name,
                route_graph_key,
                request.expected_control_revision,
                None,
            )
            .await
            .map_err(|error| control_error(error.to_string()))?
            .status
            .into(),
        OperatorRoutingCommand::SetEndpointMode {
            provider_id,
            endpoint_id,
            mode,
        } => {
            let target = candidate_key(proxy.service_name, &template, provider_id, endpoint_id)?;
            let commit = proxy
                .state
                .compare_and_set_provider_manual_eligibility(
                    request.expected_policy_revision,
                    target,
                    mode.manual_eligibility(),
                    mode.audit_reason(),
                    crate::logging::now_ms(),
                )
                .await
                .map_err(|error| {
                    control_error(format!("provider policy mutation failed: {error}"))
                })?;
            if commit.status == ProviderManualEligibilityUpdate::Applied {
                proxy
                    .config
                    .publish_provider_policy(Arc::clone(&commit.snapshot))
                    .await
                    .map_err(|error| {
                        control_error(format!("publish provider policy failed: {error:#}"))
                    })?;
            }
            commit.status.into()
        }
    };

    let refreshed = current_routing_summary(proxy).await?;
    let status = if refreshed.route_graph_key == request.expected_route_graph_key {
        status
    } else {
        OperatorRoutingMutationStatus::Conflict
    };
    Ok(OperatorRoutingMutationResponse {
        status,
        routing: refreshed,
    })
}

async fn current_routing_summary(
    proxy: &ProxyService,
) -> Result<OperatorRoutingSummary, ProxyControlError> {
    let runtime_snapshot = proxy.config.capture().await;
    let config = runtime_snapshot.config();
    let view =
        super::control_plane_service::service_route_config(config.as_ref(), proxy.service_name);
    let graph = runtime_snapshot
        .route_graph(proxy.service_name)
        .ok_or_else(|| control_error("runtime snapshot has no route graph for the service"))?;
    let template = graph.handshake_plan();
    let control = proxy.state.capture_routing_operator_control().await;
    let provider_policy = proxy.state.capture_provider_policy_snapshot().await;
    summarize_routing(
        view,
        &template,
        graph.digest(),
        &control,
        provider_policy.as_ref(),
        proxy.service_name,
    )
}

fn summarize_routing(
    view: &crate::config::ServiceRouteConfig,
    template: &crate::routing_ir::RoutePlanTemplate,
    route_graph_key: &str,
    control: &RoutingOperatorControlSnapshot,
    provider_policy: &ProviderPolicySnapshot,
    service_name: &str,
) -> Result<OperatorRoutingSummary, ProxyControlError> {
    build_operator_routing_summary(
        view,
        template,
        OperatorRoutingControlView {
            route_graph_key,
            control_revision: control.revision(),
            provider_policy_revision: provider_policy.policy_revision,
            new_session_preference: control.new_session_preference(service_name, route_graph_key),
        },
    )
    .map_err(|error| control_error(format!("build routing summary failed: {error:#}")))
}

fn candidate_key(
    service_name: &str,
    template: &crate::routing_ir::RoutePlanTemplate,
    provider_id: &str,
    endpoint_id: &str,
) -> Result<ProviderEndpointKey, ProxyControlError> {
    let provider_id = provider_id.trim();
    let endpoint_id = endpoint_id.trim();
    if provider_id.is_empty() || endpoint_id.is_empty() {
        return Err(invalid_request_error(
            "routing control requires a provider and endpoint",
        ));
    }
    let candidate = template
        .candidates
        .iter()
        .find(|candidate| {
            candidate.provider_id == provider_id && candidate.endpoint_id == endpoint_id
        })
        .with_context(|| {
            format!("routing target '{provider_id}.{endpoint_id}' is not a compiled candidate")
        })
        .map_err(|error| invalid_request_error(error.to_string()))?;
    let key = template.candidate_provider_endpoint_key(candidate);
    debug_assert_eq!(key.service_name, service_name);
    Ok(key)
}

fn control_error(message: impl Into<String>) -> ProxyControlError {
    ProxyControlError::new(StatusCode::CONFLICT, message)
}

fn invalid_request_error(message: impl Into<String>) -> ProxyControlError {
    ProxyControlError::new(StatusCode::BAD_REQUEST, message)
}

impl From<RoutingOperatorControlUpdate> for OperatorRoutingMutationStatus {
    fn from(value: RoutingOperatorControlUpdate) -> Self {
        match value {
            RoutingOperatorControlUpdate::Applied => Self::Applied,
            RoutingOperatorControlUpdate::Unchanged => Self::Unchanged,
            RoutingOperatorControlUpdate::Conflict => Self::Conflict,
        }
    }
}

impl From<ProviderManualEligibilityUpdate> for OperatorRoutingMutationStatus {
    fn from(value: ProviderManualEligibilityUpdate) -> Self {
        match value {
            ProviderManualEligibilityUpdate::Applied => Self::Applied,
            ProviderManualEligibilityUpdate::Unchanged => Self::Unchanged,
            ProviderManualEligibilityUpdate::Conflict => Self::Conflict,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::{
        HelperConfig, ProviderConcurrencyLimits, ProviderConfig, ProviderEndpointConfig,
        RouteGraphConfig, ServiceRouteConfig,
    };

    fn endpoint(base_url: &str) -> ProviderEndpointConfig {
        ProviderEndpointConfig {
            base_url: base_url.to_string(),
            continuity_domain: None,
            enabled: true,
            priority: 0,
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            limits: ProviderConcurrencyLimits::default(),
        }
    }

    fn test_config() -> HelperConfig {
        HelperConfig {
            codex: ServiceRouteConfig {
                providers: BTreeMap::from([
                    (
                        "input".to_string(),
                        ProviderConfig {
                            endpoints: BTreeMap::from([(
                                "fast".to_string(),
                                endpoint("https://input.example/v1"),
                            )]),
                            ..ProviderConfig::default()
                        },
                    ),
                    (
                        "ciii".to_string(),
                        ProviderConfig {
                            base_url: Some("https://ciii.example/v1".to_string()),
                            ..ProviderConfig::default()
                        },
                    ),
                ]),
                routing: Some(RouteGraphConfig::round_robin(vec![
                    "input".to_string(),
                    "ciii".to_string(),
                ])),
                ..ServiceRouteConfig::default()
            },
            ..HelperConfig::default()
        }
    }

    async fn proxy_and_routing() -> (ProxyService, OperatorRoutingSummary) {
        let proxy = ProxyService::new(reqwest::Client::new(), Arc::new(test_config()), "codex");
        let routing = current_routing_summary(&proxy)
            .await
            .expect("routing summary");
        (proxy, routing)
    }

    fn request(
        routing: &OperatorRoutingSummary,
        command: OperatorRoutingCommand,
    ) -> OperatorRoutingMutationRequest {
        OperatorRoutingMutationRequest {
            expected_route_graph_key: routing.route_graph_key.clone(),
            expected_control_revision: routing.control_revision,
            expected_policy_revision: routing.provider_policy_revision,
            command,
        }
    }

    #[tokio::test]
    async fn new_session_preference_is_runtime_only_and_idempotent() {
        let (proxy, routing) = proxy_and_routing().await;
        let original_config = proxy.config.capture().await.config();
        let command = OperatorRoutingCommand::SetNewSessionPreference {
            provider_id: "input".to_string(),
            endpoint_id: "fast".to_string(),
        };

        let applied = mutate_operator_routing(&proxy, request(&routing, command.clone()))
            .await
            .expect("set preference");
        assert_eq!(applied.status, OperatorRoutingMutationStatus::Applied);
        assert_eq!(
            applied
                .routing
                .new_session_preference
                .as_ref()
                .map(|target| (target.provider_id.as_str(), target.endpoint_id.as_str())),
            Some(("input", "fast"))
        );
        assert!(Arc::ptr_eq(
            &original_config,
            &proxy.config.capture().await.config()
        ));

        let repeated = mutate_operator_routing(&proxy, request(&applied.routing, command))
            .await
            .expect("repeat preference");
        assert_eq!(repeated.status, OperatorRoutingMutationStatus::Unchanged);
    }

    #[tokio::test]
    async fn stale_route_graph_key_conflicts_without_mutation() {
        let (proxy, routing) = proxy_and_routing().await;
        let mut stale = request(&routing, OperatorRoutingCommand::ClearNewSessionPreference);
        stale.expected_route_graph_key = "sha256:stale".to_string();

        let response = mutate_operator_routing(&proxy, stale)
            .await
            .expect("conflict response");

        assert_eq!(response.status, OperatorRoutingMutationStatus::Conflict);
        assert!(response.routing.new_session_preference.is_none());
    }

    #[tokio::test]
    async fn endpoint_mode_uses_provider_policy_revision_cas() {
        let (proxy, routing) = proxy_and_routing().await;
        let command = OperatorRoutingCommand::SetEndpointMode {
            provider_id: "ciii".to_string(),
            endpoint_id: "default".to_string(),
            mode: OperatorEndpointMode::Draining,
        };

        let applied = mutate_operator_routing(&proxy, request(&routing, command.clone()))
            .await
            .expect("drain endpoint");
        assert_eq!(applied.status, OperatorRoutingMutationStatus::Applied);
        assert!(applied.routing.provider_policy_revision > routing.provider_policy_revision);

        let stale = mutate_operator_routing(&proxy, request(&routing, command))
            .await
            .expect("stale policy conflict");
        assert_eq!(stale.status, OperatorRoutingMutationStatus::Conflict);
    }

    #[tokio::test]
    async fn new_session_preference_rejects_stale_provider_policy() {
        let (proxy, routing) = proxy_and_routing().await;
        let drained = mutate_operator_routing(
            &proxy,
            request(
                &routing,
                OperatorRoutingCommand::SetEndpointMode {
                    provider_id: "input".to_string(),
                    endpoint_id: "fast".to_string(),
                    mode: OperatorEndpointMode::Draining,
                },
            ),
        )
        .await
        .expect("drain endpoint");
        assert_eq!(drained.status, OperatorRoutingMutationStatus::Applied);

        let stale = mutate_operator_routing(
            &proxy,
            request(
                &routing,
                OperatorRoutingCommand::SetNewSessionPreference {
                    provider_id: "input".to_string(),
                    endpoint_id: "fast".to_string(),
                },
            ),
        )
        .await
        .expect("stale preference response");

        assert_eq!(stale.status, OperatorRoutingMutationStatus::Conflict);
        assert!(stale.routing.new_session_preference.is_none());
    }

    #[tokio::test]
    async fn invalid_candidate_is_a_bad_request_without_mutating_control_state() {
        let (proxy, routing) = proxy_and_routing().await;
        let command = OperatorRoutingCommand::SetNewSessionPreference {
            provider_id: "missing".to_string(),
            endpoint_id: "default".to_string(),
        };

        let error = mutate_operator_routing(&proxy, request(&routing, command))
            .await
            .expect_err("invalid candidate must fail");

        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        let current = current_routing_summary(&proxy)
            .await
            .expect("routing summary after rejection");
        assert_eq!(current.control_revision, routing.control_revision);
        assert_eq!(
            current.provider_policy_revision,
            routing.provider_policy_revision
        );
        assert!(current.new_session_preference.is_none());
    }
}
