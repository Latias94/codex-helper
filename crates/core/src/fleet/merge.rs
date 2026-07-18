use super::model::{FleetNodeSnapshot, FleetSnapshot, now_ms};

pub fn merge_fleet_nodes(
    service_name: impl Into<String>,
    nodes: Vec<FleetNodeSnapshot>,
) -> FleetSnapshot {
    FleetSnapshot {
        api_version: 1,
        service_name: service_name.into(),
        refreshed_at_ms: now_ms(),
        nodes,
    }
}

pub fn merge_fleet_snapshots(
    service_name: impl Into<String>,
    snapshots: Vec<FleetSnapshot>,
) -> FleetSnapshot {
    let nodes = snapshots
        .into_iter()
        .flat_map(|snapshot| snapshot.nodes)
        .collect::<Vec<_>>();
    merge_fleet_nodes(service_name, nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::CredentialAggregateReadiness;
    use crate::fleet::model::{
        FleetGraphStatus, FleetNodeHealth, FleetNodeKind, FleetProcessSummary, FleetTopology,
    };

    #[test]
    fn merge_keeps_node_boundaries() {
        let node = |node_id: &str| FleetNodeSnapshot {
            node_id: node_id.to_string(),
            label: node_id.to_string(),
            kind: FleetNodeKind::Remote,
            health: FleetNodeHealth::Fresh,
            credential_readiness: None,
            refreshed_at_ms: 1,
            stale_since_ms: None,
            snapshot_age_ms: None,
            active_endpoint: None,
            last_error: None,
            processes: FleetProcessSummary::default(),
            topology: FleetTopology {
                status: FleetGraphStatus::Unavailable,
                edges: Vec::new(),
                note: None,
            },
            work_units: Vec::new(),
        };
        let merged = merge_fleet_nodes("codex", vec![node("a"), node("b")]);
        assert_eq!(
            merged
                .nodes
                .iter()
                .map(|node| node.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn merge_preserves_node_owned_credential_readiness() {
        let node = |node_id: &str, credential_readiness| FleetNodeSnapshot {
            node_id: node_id.to_string(),
            label: node_id.to_string(),
            kind: FleetNodeKind::Remote,
            health: FleetNodeHealth::Fresh,
            credential_readiness: Some(credential_readiness),
            refreshed_at_ms: 1,
            stale_since_ms: None,
            snapshot_age_ms: None,
            active_endpoint: None,
            last_error: None,
            processes: FleetProcessSummary::default(),
            topology: FleetTopology {
                status: FleetGraphStatus::Unavailable,
                edges: Vec::new(),
                note: None,
            },
            work_units: Vec::new(),
        };
        let merged = merge_fleet_nodes(
            "codex",
            vec![
                node("local", CredentialAggregateReadiness::Ready),
                node("server", CredentialAggregateReadiness::Blocked),
            ],
        );

        assert_eq!(
            merged
                .nodes
                .iter()
                .map(|node| (node.node_id.as_str(), node.credential_readiness))
                .collect::<Vec<_>>(),
            vec![
                ("local", Some(CredentialAggregateReadiness::Ready)),
                ("server", Some(CredentialAggregateReadiness::Blocked)),
            ]
        );
    }
}
