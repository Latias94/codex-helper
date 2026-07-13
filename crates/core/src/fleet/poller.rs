use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::control_plane_client::{ControlPlaneClient, ControlPlaneEndpoint, ControlPlaneError};

use super::model::{FleetNodeHealth, FleetNodeSnapshot, FleetSnapshot, now_ms};
use super::observer::build_fleet_snapshot_from_operator_read_model;
use super::registry::FleetNodeConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FleetPollResult {
    pub node_id: String,
    pub label: String,
    pub health: FleetNodeHealth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<FleetNodeSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn poll_fleet_node(
    service_name: &str,
    node_id: &str,
    node: &FleetNodeConfig,
) -> FleetPollResult {
    let label = node.display_label(node_id);
    let mut last_failure: Option<(FleetNodeHealth, String)> = None;

    for endpoint_url in node.endpoints() {
        let endpoint =
            match ControlPlaneEndpoint::new(endpoint_url, node.admin_token_env.as_deref()) {
                Ok(endpoint) => endpoint,
                Err(err) => {
                    record_fallback_failure(
                        &mut last_failure,
                        FleetNodeHealth::Unsupported,
                        err.to_string(),
                    );
                    continue;
                }
            };
        let client = match ControlPlaneClient::new(endpoint) {
            Ok(client) => client,
            Err(err) => {
                record_fallback_failure(
                    &mut last_failure,
                    FleetNodeHealth::Unreachable,
                    err.to_string(),
                );
                continue;
            }
        };

        match client.operator_read_model().await {
            Ok(model) if model.service_name == service_name => {
                let node_snapshot = build_fleet_snapshot_from_operator_read_model(
                    &model,
                    node_id,
                    &label,
                    super::model::FleetNodeKind::Remote,
                    Some(client.endpoint().admin_base_url().to_string()),
                )
                .nodes
                .into_iter()
                .next()
                .expect("operator fleet projection always contains one node");
                return FleetPollResult {
                    node_id: node_id.to_string(),
                    label,
                    health: FleetNodeHealth::Fresh,
                    snapshot: Some(node_snapshot),
                    error: None,
                };
            }
            Ok(model) => {
                record_fallback_failure(
                    &mut last_failure,
                    FleetNodeHealth::ParseFailed,
                    format!(
                        "operator read-model service mismatch: expected {service_name}, got {}",
                        model.service_name
                    ),
                );
                continue;
            }
            Err(err) => {
                record_fallback_failure(
                    &mut last_failure,
                    health_from_control_plane_error(&err),
                    err.to_string(),
                );
                continue;
            }
        }
    }

    let (health, error) = last_failure.unwrap_or_else(|| {
        (
            FleetNodeHealth::Unreachable,
            "no configured fleet endpoint could be reached".to_string(),
        )
    });
    FleetPollResult {
        node_id: node_id.to_string(),
        label: label.clone(),
        health,
        snapshot: Some(empty_remote_node(
            node_id,
            &label,
            health,
            Some(error.clone()),
        )),
        error: Some(error),
    }
}

pub fn stale_node_from_previous(
    previous: &FleetNodeSnapshot,
    health: FleetNodeHealth,
    error: impl Into<String>,
) -> FleetNodeSnapshot {
    let now = now_ms();
    let mut snapshot = previous.clone();
    snapshot.health = health;
    snapshot.snapshot_age_ms = Some(now.saturating_sub(previous.refreshed_at_ms));
    snapshot.stale_since_ms = snapshot.stale_since_ms.or(Some(now));
    snapshot.last_error = Some(error.into());
    snapshot
}

pub fn node_snapshot_from_poll_result(
    node_id: &str,
    result: FleetPollResult,
    previous: Option<&FleetNodeSnapshot>,
) -> FleetNodeSnapshot {
    let error = result
        .error
        .clone()
        .unwrap_or_else(|| "fleet node refresh failed".to_string());
    if result.health.is_current() {
        return result.snapshot.unwrap_or_else(|| {
            empty_remote_node(node_id, result.label.as_str(), result.health, Some(error))
        });
    }

    let can_retain_previous = matches!(
        result.health,
        FleetNodeHealth::Stale
            | FleetNodeHealth::RateLimited
            | FleetNodeHealth::Unsupported
            | FleetNodeHealth::Unreachable
    );
    previous
        .filter(|_| can_retain_previous)
        .map(|previous| stale_node_from_previous(previous, FleetNodeHealth::Stale, error.clone()))
        .or(result.snapshot)
        .unwrap_or_else(|| {
            empty_remote_node(node_id, result.label.as_str(), result.health, Some(error))
        })
}

pub fn health_from_control_plane_error(err: &ControlPlaneError) -> FleetNodeHealth {
    match err {
        ControlPlaneError::UntrustedRequestPath { .. } => FleetNodeHealth::Unreachable,
        ControlPlaneError::HttpStatus { status, .. } if *status == 401 || *status == 403 => {
            FleetNodeHealth::AuthFailed
        }
        ControlPlaneError::HttpStatus { status, .. } if *status == 429 => {
            FleetNodeHealth::RateLimited
        }
        ControlPlaneError::HttpStatus { status, .. } if *status == 404 || *status == 501 => {
            FleetNodeHealth::Unsupported
        }
        ControlPlaneError::HttpStatus { .. } => FleetNodeHealth::Unreachable,
        ControlPlaneError::Decode { .. } => FleetNodeHealth::ParseFailed,
        ControlPlaneError::InvalidPayload { .. } => FleetNodeHealth::ParseFailed,
        ControlPlaneError::Transport { .. } => FleetNodeHealth::Unreachable,
    }
}

fn empty_remote_node(
    node_id: &str,
    label: &str,
    health: FleetNodeHealth,
    error: Option<String>,
) -> FleetNodeSnapshot {
    FleetNodeSnapshot {
        node_id: node_id.to_string(),
        label: label.to_string(),
        kind: super::model::FleetNodeKind::Remote,
        health,
        refreshed_at_ms: now_ms(),
        stale_since_ms: None,
        snapshot_age_ms: None,
        active_endpoint: None,
        last_error: error,
        processes: super::model::FleetProcessSummary::default(),
        topology: super::model::FleetTopology::default(),
        work_units: Vec::new(),
    }
}

fn record_fallback_failure(
    last_failure: &mut Option<(FleetNodeHealth, String)>,
    health: FleetNodeHealth,
    error: String,
) {
    let should_replace = match last_failure {
        Some((current_health, _)) => health_priority(health) >= health_priority(*current_health),
        None => true,
    };
    if should_replace {
        *last_failure = Some((health, error));
    }
}

fn health_priority(health: FleetNodeHealth) -> u8 {
    match health {
        FleetNodeHealth::Fresh => 6,
        FleetNodeHealth::AuthFailed => 5,
        FleetNodeHealth::ParseFailed => 4,
        FleetNodeHealth::RateLimited => 3,
        FleetNodeHealth::Unsupported => 2,
        FleetNodeHealth::Stale => 1,
        FleetNodeHealth::Unreachable => 0,
    }
}

pub async fn poll_fleet_registry(
    service_name: &str,
    registry: &super::registry::FleetRegistryConfig,
) -> FleetSnapshot {
    poll_fleet_registry_with_previous(service_name, registry, None).await
}

pub async fn poll_fleet_registry_with_previous(
    service_name: &str,
    registry: &super::registry::FleetRegistryConfig,
    previous: Option<&FleetSnapshot>,
) -> FleetSnapshot {
    let mut nodes = Vec::new();
    for (node_id, node) in registry.enabled_nodes() {
        let result = poll_fleet_node(service_name, node_id, node).await;
        let previous_node = previous.and_then(|snapshot| {
            snapshot
                .nodes
                .iter()
                .find(|snapshot| snapshot.node_id.as_str() == node_id.as_str())
        });
        nodes.push(node_snapshot_from_poll_result(
            node_id,
            result,
            previous_node,
        ));
    }
    super::merge::merge_fleet_nodes(service_name, nodes)
}

pub fn default_poll_interval() -> Duration {
    Duration::from_millis(1_500)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard_core::{
        ApiV1OperatorSummary, OperatorReadData, OperatorReadModel, OperatorRevisionBundle,
    };
    use crate::fleet::{FleetNodeKind, FleetProcessSummary, FleetTopology};
    use axum::{Json, http::StatusCode, routing::get};
    use std::collections::BTreeMap;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::TcpListener;

    async fn spawn_axum_server(app: axum::Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        (addr, handle)
    }

    fn ready_operator_model() -> OperatorReadModel {
        OperatorReadModel::ready(
            "codex",
            42,
            OperatorRevisionBundle {
                runtime_revision: 7,
                runtime_digest: "runtime-7".to_string(),
                route_digest: "route-7".to_string(),
                catalog_revision: "catalog-7".to_string(),
                pricing_revision: "pricing-7".to_string(),
                operator_pricing_revision: "operator-pricing-7".to_string(),
                policy_revision: 8,
                ledger_revision: "ledger-9".to_string(),
            },
            OperatorReadData {
                summary: ApiV1OperatorSummary {
                    api_version: 1,
                    service_name: "codex".to_string(),
                    runtime: Default::default(),
                    counts: Default::default(),
                    retry: Default::default(),
                    sessions: Vec::new(),
                    profiles: Vec::new(),
                    providers: Vec::new(),
                },
                active_requests: Vec::new(),
                recent_requests: Vec::new(),
                usage_summaries: Vec::new(),
                usage_day: Default::default(),
                usage_rollup: Default::default(),
                stats_5m: Default::default(),
                stats_1h: Default::default(),
                pricing_catalog: Default::default(),
                provider_balances: Vec::new(),
            },
        )
    }

    #[test]
    fn classifies_http_errors_for_fleet_degradation() {
        let err = |status| ControlPlaneError::HttpStatus {
            status,
            body_excerpt: String::new(),
        };

        assert_eq!(
            health_from_control_plane_error(&err(403)),
            FleetNodeHealth::AuthFailed
        );
        assert_eq!(
            health_from_control_plane_error(&err(429)),
            FleetNodeHealth::RateLimited
        );
        assert_eq!(
            health_from_control_plane_error(&err(404)),
            FleetNodeHealth::Unsupported
        );
    }

    #[test]
    fn invalid_payload_failure_outranks_reconnectable_endpoint_failures() {
        let mut failure = None;
        record_fallback_failure(
            &mut failure,
            FleetNodeHealth::ParseFailed,
            "invalid payload".to_string(),
        );
        record_fallback_failure(
            &mut failure,
            FleetNodeHealth::RateLimited,
            "rate limited".to_string(),
        );
        record_fallback_failure(
            &mut failure,
            FleetNodeHealth::Unsupported,
            "unsupported".to_string(),
        );

        assert_eq!(
            failure,
            Some((FleetNodeHealth::ParseFailed, "invalid payload".to_string()))
        );
    }

    #[tokio::test]
    async fn poll_fleet_node_falls_back_to_second_endpoint_after_http_failure() {
        let first_hits = Arc::new(AtomicUsize::new(0));
        let second_hits = Arc::new(AtomicUsize::new(0));

        let first_count = first_hits.clone();
        let first_app = axum::Router::new().route(
            "/__codex_helper/api/v1/operator/read-model",
            get(move || {
                let first_count = first_count.clone();
                async move {
                    first_count.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::FORBIDDEN, "forbidden")
                }
            }),
        );
        let (first_addr, first_handle) = spawn_axum_server(first_app).await;

        let second_count = second_hits.clone();
        let second_model = ready_operator_model();
        let second_app = axum::Router::new().route(
            "/__codex_helper/api/v1/operator/read-model",
            get(move || {
                let second_count = second_count.clone();
                let second_model = second_model.clone();
                async move {
                    second_count.fetch_add(1, Ordering::SeqCst);
                    Json(second_model)
                }
            }),
        );
        let (second_addr, second_handle) = spawn_axum_server(second_app).await;

        let first_url = format!("http://{}", first_addr);
        let second_url = format!("http://{}", second_addr);
        let node = FleetNodeConfig {
            label: Some("remote-node".to_string()),
            admin_url: Some(first_url.clone()),
            admin_urls: vec![second_url.clone()],
            admin_token_env: None,
            enabled: true,
        };

        let result = poll_fleet_node("codex", "node-a", &node).await;
        assert_eq!(result.health, FleetNodeHealth::Fresh);
        assert!(result.error.is_none());

        let snapshot = result.snapshot.expect("snapshot");
        assert_eq!(snapshot.node_id, "node-a");
        assert_eq!(snapshot.label, "remote-node");
        assert_eq!(snapshot.health, FleetNodeHealth::Fresh);
        assert_eq!(
            snapshot.active_endpoint.as_deref(),
            Some(second_url.as_str())
        );
        assert_eq!(first_hits.load(Ordering::SeqCst), 1);
        assert_eq!(second_hits.load(Ordering::SeqCst), 1);

        first_handle.abort();
        second_handle.abort();
    }

    #[tokio::test]
    async fn poll_fleet_registry_uses_previous_snapshot_when_node_refresh_fails() {
        let hits = Arc::new(AtomicUsize::new(0));
        let hit_count = hits.clone();
        let app = axum::Router::new().route(
            "/__codex_helper/api/v1/operator/read-model",
            get(move || {
                let hit_count = hit_count.clone();
                async move {
                    hit_count.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::FORBIDDEN, "forbidden")
                }
            }),
        );
        let (addr, handle) = spawn_axum_server(app).await;

        let previous = FleetSnapshot {
            api_version: 1,
            service_name: "codex".to_string(),
            refreshed_at_ms: 200,
            nodes: vec![FleetNodeSnapshot {
                node_id: "node-a".to_string(),
                label: "remote-node".to_string(),
                kind: FleetNodeKind::Remote,
                health: FleetNodeHealth::Fresh,
                refreshed_at_ms: 100,
                stale_since_ms: None,
                snapshot_age_ms: None,
                active_endpoint: Some("http://previous.example".to_string()),
                last_error: None,
                processes: FleetProcessSummary {
                    scan_available: true,
                    codex_like_processes: 2,
                    error: None,
                },
                topology: FleetTopology::default(),
                work_units: vec![super::super::model::FleetWorkUnit {
                    node_id: "node-a".to_string(),
                    id: "session:keep".to_string(),
                    parent_id: None,
                    kind: super::super::model::FleetWorkUnitKind::Root,
                    state: super::super::model::FleetWorkUnitState::Running,
                    evidence: super::super::model::FleetEvidence::high(
                        super::super::model::FleetEvidenceSource::RuntimeStatus,
                    ),
                    session_id: Some("keep".to_string()),
                    local_thread_id: Some("keep".to_string()),
                    task_name: Some("preserved".to_string()),
                    cwd: None,
                    model: None,
                    provider_id: None,
                    last_status: None,
                    active_started_at_ms: None,
                    last_activity_ms: Some(100),
                    last_error: None,
                    usage: Default::default(),
                }],
            }],
        };
        let registry = super::super::registry::FleetRegistryConfig {
            nodes: BTreeMap::from([(
                "node-a".to_string(),
                FleetNodeConfig {
                    label: Some("remote-node".to_string()),
                    admin_url: Some(format!("http://{}", addr)),
                    admin_urls: Vec::new(),
                    admin_token_env: None,
                    enabled: true,
                },
            )]),
        };

        let snapshot = poll_fleet_registry_with_previous("codex", &registry, Some(&previous)).await;

        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(snapshot.nodes.len(), 1);
        let node = &snapshot.nodes[0];
        assert_eq!(node.node_id, "node-a");
        assert_eq!(node.health, FleetNodeHealth::AuthFailed);
        assert!(node.work_units.is_empty());
        assert!(node.active_endpoint.is_none());
        assert!(node.snapshot_age_ms.is_none());
        assert!(node.stale_since_ms.is_none());
        assert!(
            node.last_error
                .as_deref()
                .is_some_and(|err| err.contains("403"))
        );

        handle.abort();
    }

    #[test]
    fn reconnectable_failure_retains_previous_facts_as_stale() {
        let previous = FleetNodeSnapshot {
            node_id: "node-a".to_string(),
            label: "remote-node".to_string(),
            kind: FleetNodeKind::Remote,
            health: FleetNodeHealth::Fresh,
            refreshed_at_ms: 100,
            stale_since_ms: None,
            snapshot_age_ms: None,
            active_endpoint: Some("http://previous.example".to_string()),
            last_error: None,
            processes: FleetProcessSummary::default(),
            topology: FleetTopology::default(),
            work_units: Vec::new(),
        };
        let result = FleetPollResult {
            node_id: "node-a".to_string(),
            label: "remote-node".to_string(),
            health: FleetNodeHealth::Unreachable,
            snapshot: Some(empty_remote_node(
                "node-a",
                "remote-node",
                FleetNodeHealth::Unreachable,
                Some("transport failed".to_string()),
            )),
            error: Some("transport failed".to_string()),
        };

        let node = node_snapshot_from_poll_result("node-a", result, Some(&previous));

        assert_eq!(node.health, FleetNodeHealth::Stale);
        assert_eq!(
            node.active_endpoint.as_deref(),
            Some("http://previous.example")
        );
        assert_eq!(node.last_error.as_deref(), Some("transport failed"));
        assert!(node.stale_since_ms.is_some());
    }

    #[test]
    fn parse_failure_drops_previous_runtime_facts() {
        let previous = FleetNodeSnapshot {
            node_id: "node-a".to_string(),
            label: "remote-node".to_string(),
            kind: FleetNodeKind::Remote,
            health: FleetNodeHealth::Fresh,
            refreshed_at_ms: 100,
            stale_since_ms: None,
            snapshot_age_ms: None,
            active_endpoint: Some("http://previous.example".to_string()),
            last_error: None,
            processes: FleetProcessSummary {
                scan_available: true,
                codex_like_processes: 2,
                error: None,
            },
            topology: FleetTopology::default(),
            work_units: Vec::new(),
        };
        let result = FleetPollResult {
            node_id: "node-a".to_string(),
            label: "remote-node".to_string(),
            health: FleetNodeHealth::ParseFailed,
            snapshot: Some(empty_remote_node(
                "node-a",
                "remote-node",
                FleetNodeHealth::ParseFailed,
                Some("invalid payload".to_string()),
            )),
            error: Some("invalid payload".to_string()),
        };

        let node = node_snapshot_from_poll_result("node-a", result, Some(&previous));

        assert_eq!(node.health, FleetNodeHealth::ParseFailed);
        assert!(node.active_endpoint.is_none());
        assert_eq!(node.processes, FleetProcessSummary::default());
        assert!(node.stale_since_ms.is_none());
    }
}
