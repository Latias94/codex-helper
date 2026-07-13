use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;

use crate::config::HelperConfig;
use crate::dashboard_core::OperatorReadModel;
use codex_helper_core::fleet::merge::merge_fleet_snapshots;
use codex_helper_core::fleet::poller::{node_snapshot_from_poll_result, poll_fleet_node};
use codex_helper_core::fleet::{
    FleetNodeKind, FleetSnapshot, build_fleet_snapshot_from_operator_read_model,
    build_local_fleet_snapshot_from_operator_read_model, now_ms,
};

use super::state::UiState;

#[derive(Debug, Clone)]
pub(super) enum FleetRefreshSource {
    Integrated {
        model: Box<OperatorReadModel>,
        cfg: Arc<HelperConfig>,
    },
    Attached {
        model: Box<OperatorReadModel>,
        admin_base_url: String,
    },
}

#[derive(Debug)]
pub(super) struct FleetRefreshResult {
    generation: u64,
    result: Result<FleetSnapshot, String>,
}

pub(super) type FleetRefreshSender = mpsc::UnboundedSender<FleetRefreshResult>;

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
        FleetRefreshSource::Integrated { model, cfg } => {
            load_integrated_fleet_snapshot(model, cfg, previous).await
        }
        FleetRefreshSource::Attached {
            model,
            admin_base_url,
        } => Ok(build_fleet_snapshot_from_operator_read_model(
            &model,
            "local",
            "local",
            FleetNodeKind::Local,
            Some(admin_base_url),
        )),
    }
}

async fn load_integrated_fleet_snapshot(
    model: Box<OperatorReadModel>,
    cfg: Arc<HelperConfig>,
    previous: Option<FleetSnapshot>,
) -> anyhow::Result<FleetSnapshot> {
    let service_name = model.service_name.clone();
    let local = build_local_fleet_snapshot_from_operator_read_model(&model, "local", "local");
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

    #[test]
    fn non_current_poll_result_keeps_previous_snapshot_stale() {
        let previous = FleetNodeSnapshot {
            node_id: "node-a".to_string(),
            label: "remote".to_string(),
            kind: FleetNodeKind::Remote,
            health: FleetNodeHealth::Fresh,
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
    async fn attached_fleet_refresh_projects_the_current_operator_bundle() {
        let snapshot = load_fleet_snapshot(
            FleetRefreshSource::Attached {
                model: Box::new(OperatorReadModel::auth_required("codex")),
                admin_base_url: "https://admin.example".to_string(),
            },
            None,
        )
        .await
        .expect("attached fleet snapshot");

        assert_eq!(snapshot.service_name, "codex");
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.nodes[0].health, FleetNodeHealth::AuthFailed);
        assert!(snapshot.nodes[0].work_units.is_empty());
        assert!(snapshot.nodes[0].active_endpoint.is_none());
    }
}
