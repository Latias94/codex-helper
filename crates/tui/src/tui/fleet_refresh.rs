use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;

use crate::config::HelperConfig;
use crate::dashboard_core::{OperatorLocalSessionMetadata, OperatorReadModel};
use codex_helper_core::fleet::merge::merge_fleet_snapshots;
use codex_helper_core::fleet::poller::{node_snapshot_from_poll_result, poll_fleet_node};
use codex_helper_core::fleet::{
    FleetNodeKind, FleetSnapshot, build_fleet_snapshot_from_operator_read_model,
    build_local_fleet_snapshot_from_operator_read_model,
    enrich_local_fleet_snapshot_session_metadata, now_ms,
};

use super::state::{RuntimeConnectionKind, UiState};

#[derive(Debug, Clone)]
pub(super) enum FleetRefreshSource {
    Integrated {
        model: Box<OperatorReadModel>,
        local_sessions: HashMap<String, OperatorLocalSessionMetadata>,
        cfg: Arc<HelperConfig>,
    },
    Attached {
        model: Box<OperatorReadModel>,
        local_sessions: HashMap<String, OperatorLocalSessionMetadata>,
        admin_base_url: String,
        connection_kind: RuntimeConnectionKind,
    },
}

#[derive(Debug)]
pub(super) struct FleetRefreshResult {
    generation: u64,
    result: Result<FleetSnapshot, String>,
}

pub(super) type FleetRefreshSender = mpsc::UnboundedSender<FleetRefreshResult>;

pub(super) fn local_session_metadata_for_fleet(
    ui: &UiState,
) -> HashMap<String, OperatorLocalSessionMetadata> {
    if ui.runtime_connection.is_remote_observer() {
        HashMap::new()
    } else {
        ui.host_local_sessions.clone()
    }
}

pub(super) fn start_fleet_refresh(
    ui: &mut UiState,
    source: FleetRefreshSource,
    tx: FleetRefreshSender,
) {
    ui.fleet_refresh_generation = ui.fleet_refresh_generation.wrapping_add(1);
    let generation = ui.fleet_refresh_generation;
    let previous = ui.fleet_snapshot.clone();
    ui.fleet_loading = true;
    ui.fleet_last_error = None;
    ui.fleet_last_refresh_at = Some(Instant::now());
    ui.toast = Some((fleet_refreshing_message(ui.language), Instant::now()));

    tokio::spawn(async move {
        let result = load_fleet_snapshot(source, previous)
            .await
            .map_err(|err| err.to_string());
        let _ = tx.send(FleetRefreshResult { generation, result });
    });
}

pub(super) fn apply_fleet_refresh_result(ui: &mut UiState, result: FleetRefreshResult) -> bool {
    if result.generation != ui.fleet_refresh_generation {
        return false;
    }

    ui.fleet_loading = false;
    ui.fleet_last_loaded_at_ms = Some(now_ms());
    match result.result {
        Ok(snapshot) => {
            let node_count = snapshot.nodes.len();
            let active_count = snapshot.active_work_units();
            ui.fleet_snapshot = Some(snapshot);
            ui.fleet_last_error = None;
            ui.sync_fleet_selection();
            ui.toast = Some((
                fleet_loaded_message(ui.language, node_count, active_count),
                Instant::now(),
            ));
        }
        Err(err) => {
            ui.fleet_last_error = Some(err.clone());
            ui.sync_fleet_selection();
            ui.toast = Some((fleet_load_failed_message(ui.language, &err), Instant::now()));
        }
    }
    true
}

async fn load_fleet_snapshot(
    source: FleetRefreshSource,
    previous: Option<FleetSnapshot>,
) -> anyhow::Result<FleetSnapshot> {
    match source {
        FleetRefreshSource::Integrated {
            model,
            local_sessions,
            cfg,
        } => load_integrated_fleet_snapshot(model, local_sessions, cfg, previous).await,
        FleetRefreshSource::Attached {
            model,
            local_sessions,
            admin_base_url,
            connection_kind,
        } => {
            let (node_id, label, kind) = match connection_kind {
                RuntimeConnectionKind::LocalAttached => ("local", "local", FleetNodeKind::Local),
                RuntimeConnectionKind::RemoteObserver => {
                    ("remote", admin_base_url.as_str(), FleetNodeKind::Remote)
                }
                RuntimeConnectionKind::Integrated => ("local", "local", FleetNodeKind::Local),
            };
            let mut snapshot = build_fleet_snapshot_from_operator_read_model(
                &model,
                node_id,
                label,
                kind,
                Some(admin_base_url.clone()),
            );
            enrich_local_fleet_snapshot_session_metadata(&mut snapshot, &local_sessions);
            Ok(snapshot)
        }
    }
}

async fn load_integrated_fleet_snapshot(
    model: Box<OperatorReadModel>,
    local_sessions: HashMap<String, OperatorLocalSessionMetadata>,
    cfg: Arc<HelperConfig>,
    previous: Option<FleetSnapshot>,
) -> anyhow::Result<FleetSnapshot> {
    let service_name = model.service_name.clone();
    let mut local = build_local_fleet_snapshot_from_operator_read_model(&model, "local", "local");
    enrich_local_fleet_snapshot_session_metadata(&mut local, &local_sessions);
    let mut snapshots = vec![local];
    let mut remote_nodes = Vec::new();
    let previous_nodes = previous
        .as_ref()
        .map(|snapshot| {
            snapshot
                .nodes
                .iter()
                .map(|node| (node.node_id.as_str(), node))
                .collect::<std::collections::HashMap<_, _>>()
        })
        .unwrap_or_default();

    for (node_id, node) in cfg.fleet.enabled_nodes() {
        if node_id == "local" {
            continue;
        }
        let result = poll_fleet_node(&service_name, node_id, node).await;
        let node_snapshot = node_snapshot_from_poll_result(
            node_id,
            result,
            previous_nodes.get(node_id.as_str()).copied(),
        );
        remote_nodes.push(node_snapshot);
    }

    if !remote_nodes.is_empty() {
        snapshots.push(FleetSnapshot {
            api_version: 1,
            service_name: service_name.to_string(),
            refreshed_at_ms: now_ms(),
            nodes: remote_nodes,
        });
    }

    Ok(merge_fleet_snapshots(&service_name, snapshots))
}

fn fleet_refreshing_message(lang: super::Language) -> String {
    match lang {
        super::Language::Zh => "fleet: 刷新中...".to_string(),
        super::Language::En => "fleet: refreshing...".to_string(),
    }
}

fn fleet_loaded_message(lang: super::Language, nodes: usize, active: usize) -> String {
    match lang {
        super::Language::Zh => format!("fleet: 已加载 {nodes} 个节点，活跃 {active}"),
        super::Language::En => format!("fleet: loaded {nodes} nodes, {active} active"),
    }
}

fn fleet_load_failed_message(lang: super::Language, err: &dyn std::fmt::Display) -> String {
    match lang {
        super::Language::Zh => format!("fleet: 加载失败：{err}"),
        super::Language::En => format!("fleet: load failed: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_helper_core::fleet::poller::FleetPollResult;
    use codex_helper_core::fleet::{
        FleetEvidence, FleetEvidenceSource, FleetNodeHealth, FleetNodeKind, FleetNodeSnapshot,
        FleetProcessSummary, FleetTopology, FleetWorkUnit, FleetWorkUnitKind, FleetWorkUnitState,
    };

    fn empty_remote_node(
        node_id: &str,
        label: &str,
        health: FleetNodeHealth,
        error: Option<String>,
    ) -> FleetNodeSnapshot {
        FleetNodeSnapshot {
            node_id: node_id.to_string(),
            label: label.to_string(),
            kind: FleetNodeKind::Remote,
            health,
            credential_readiness: None,
            refreshed_at_ms: now_ms(),
            stale_since_ms: None,
            snapshot_age_ms: None,
            active_endpoint: None,
            last_error: error,
            processes: FleetProcessSummary::default(),
            topology: FleetTopology::default(),
            work_units: Vec::new(),
        }
    }

    fn local_session_metadata() -> OperatorLocalSessionMetadata {
        OperatorLocalSessionMetadata {
            raw_session_id: "019f-local-session".to_string(),
            cwd: Some("/workspace/codex-helper".to_string()),
            last_client_name: None,
            last_client_addr: None,
            host_local_transcript_path: None,
        }
    }

    #[test]
    fn fleet_local_metadata_is_available_only_to_local_connection_modes() {
        for connection_kind in [
            RuntimeConnectionKind::Integrated,
            RuntimeConnectionKind::LocalAttached,
        ] {
            let ui = UiState {
                runtime_connection: connection_kind,
                host_local_sessions: HashMap::from([(
                    "session:sha256:local".to_string(),
                    local_session_metadata(),
                )]),
                ..UiState::default()
            };
            assert_eq!(local_session_metadata_for_fleet(&ui).len(), 1);
        }

        let remote = UiState {
            runtime_connection: RuntimeConnectionKind::RemoteObserver,
            host_local_sessions: HashMap::from([(
                "session:sha256:must-not-leak".to_string(),
                local_session_metadata(),
            )]),
            ..UiState::default()
        };
        assert!(local_session_metadata_for_fleet(&remote).is_empty());
    }

    #[test]
    fn non_current_poll_result_keeps_previous_snapshot_stale() {
        let previous = FleetNodeSnapshot {
            node_id: "node-a".to_string(),
            label: "remote".to_string(),
            kind: FleetNodeKind::Remote,
            health: FleetNodeHealth::Fresh,
            credential_readiness: None,
            refreshed_at_ms: 100,
            stale_since_ms: None,
            snapshot_age_ms: None,
            active_endpoint: Some("https://old.example.com:4211".to_string()),
            last_error: None,
            processes: FleetProcessSummary {
                scan_available: true,
                codex_like_processes: 3,
                error: None,
            },
            topology: FleetTopology::default(),
            work_units: vec![FleetWorkUnit {
                node_id: "node-a".to_string(),
                id: "root".to_string(),
                parent_id: None,
                kind: FleetWorkUnitKind::Root,
                state: FleetWorkUnitState::Running,
                evidence: FleetEvidence::high(FleetEvidenceSource::RuntimeStatus),
                session_id: None,
                local_thread_id: None,
                task_name: Some("keep me".to_string()),
                cwd: None,
                model: None,
                provider_id: None,
                last_status: None,
                active_started_at_ms: None,
                last_activity_ms: Some(100),
                last_error: None,
                usage: Default::default(),
            }],
        };

        let result = FleetPollResult {
            node_id: "node-a".to_string(),
            label: "remote".to_string(),
            health: FleetNodeHealth::Stale,
            snapshot: Some(empty_remote_node(
                "node-a",
                "remote",
                FleetNodeHealth::Stale,
                Some("timeout".to_string()),
            )),
            error: Some("timeout".to_string()),
        };

        let snapshot = node_snapshot_from_poll_result("node-a", result, Some(&previous));

        assert_eq!(snapshot.health, FleetNodeHealth::Stale);
        assert_eq!(snapshot.last_error.as_deref(), Some("timeout"));
        assert_eq!(snapshot.work_units.len(), 1);
        assert!(snapshot.stale_since_ms.is_some());
    }

    #[tokio::test]
    async fn local_attached_fleet_refresh_projects_the_current_operator_bundle() {
        let snapshot = load_fleet_snapshot(
            FleetRefreshSource::Attached {
                model: Box::new(OperatorReadModel::auth_required("codex")),
                local_sessions: HashMap::new(),
                admin_base_url: "https://admin.example".to_string(),
                connection_kind: RuntimeConnectionKind::LocalAttached,
            },
            None,
        )
        .await
        .expect("attached fleet snapshot");

        assert_eq!(snapshot.service_name, "codex");
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.nodes[0].node_id, "local");
        assert_eq!(snapshot.nodes[0].kind, FleetNodeKind::Local);
        assert_eq!(snapshot.nodes[0].health, FleetNodeHealth::AuthFailed);
        assert!(snapshot.nodes[0].work_units.is_empty());
        assert!(snapshot.nodes[0].active_endpoint.is_none());
    }

    #[tokio::test]
    async fn remote_observer_fleet_refresh_preserves_remote_identity() {
        let snapshot = load_fleet_snapshot(
            FleetRefreshSource::Attached {
                model: Box::new(OperatorReadModel::auth_required("codex")),
                local_sessions: HashMap::new(),
                admin_base_url: "https://admin.example".to_string(),
                connection_kind: RuntimeConnectionKind::RemoteObserver,
            },
            None,
        )
        .await
        .expect("remote fleet snapshot");

        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.nodes[0].node_id, "remote");
        assert_eq!(snapshot.nodes[0].label, "https://admin.example");
        assert_eq!(snapshot.nodes[0].kind, FleetNodeKind::Remote);
    }
}
