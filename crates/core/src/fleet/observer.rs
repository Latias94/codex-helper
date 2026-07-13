use crate::dashboard_core::{
    OperatorReadData, OperatorReadModel, OperatorReadStatus, OperatorSessionSummary,
};

use super::model::{
    FleetConfidence, FleetEvidence, FleetEvidenceSource, FleetGraphStatus, FleetNodeHealth,
    FleetNodeKind, FleetNodeSnapshot, FleetProcessSummary, FleetSnapshot, FleetTopology,
    FleetUsageSummary, FleetWorkUnit, FleetWorkUnitKind, FleetWorkUnitState, now_ms,
};
use super::process_scan::scan_codex_processes;

pub fn build_local_fleet_snapshot_from_operator_read_model(
    model: &OperatorReadModel,
    node_id: impl Into<String>,
    label: impl Into<String>,
) -> FleetSnapshot {
    let mut snapshot = build_fleet_snapshot_from_operator_read_model(
        model,
        node_id,
        label,
        FleetNodeKind::Local,
        None,
    );
    if let Some(node) = snapshot.nodes.first_mut() {
        node.processes = scan_codex_processes().summary();
    }
    snapshot
}

pub fn build_fleet_snapshot_from_operator_read_model(
    model: &OperatorReadModel,
    node_id: impl Into<String>,
    label: impl Into<String>,
    kind: FleetNodeKind,
    active_endpoint: Option<String>,
) -> FleetSnapshot {
    let node_id = node_id.into();
    let label = label.into();
    let now = now_ms();
    let validation_error = model.validate().err();
    let (health, last_error) = if let Some(error) = validation_error {
        (
            FleetNodeHealth::ParseFailed,
            Some(format!("invalid operator read-model: {error}")),
        )
    } else {
        match model.status {
            OperatorReadStatus::Ready => (FleetNodeHealth::Fresh, None),
            OperatorReadStatus::Stale => (
                FleetNodeHealth::Stale,
                Some("operator read-model refresh failed".to_string()),
            ),
            OperatorReadStatus::Disconnected => (
                FleetNodeHealth::Unreachable,
                Some("operator read-model is disconnected".to_string()),
            ),
            OperatorReadStatus::AuthRequired => (
                FleetNodeHealth::AuthFailed,
                Some("operator read-model authentication is required".to_string()),
            ),
        }
    };

    let retained_data = matches!(
        model.status,
        OperatorReadStatus::Ready | OperatorReadStatus::Stale
    ) && validation_error.is_none();
    let refreshed_at_ms = if retained_data {
        model.captured_at_ms
    } else {
        now
    };
    let node = if retained_data {
        build_operator_fleet_node(OperatorFleetNodeParts {
            data: model.data.as_ref().expect("validated operator data"),
            node_id,
            label,
            kind,
            health,
            refreshed_at_ms,
            active_endpoint,
            last_error,
            stale_since_ms: (health == FleetNodeHealth::Stale).then_some(now),
            snapshot_age_ms: Some(now.saturating_sub(refreshed_at_ms)),
        })
    } else {
        FleetNodeSnapshot {
            node_id,
            label,
            kind,
            health,
            refreshed_at_ms,
            stale_since_ms: None,
            snapshot_age_ms: None,
            active_endpoint: None,
            last_error,
            processes: FleetProcessSummary::default(),
            topology: FleetTopology::default(),
            work_units: Vec::new(),
        }
    };

    FleetSnapshot {
        api_version: 1,
        service_name: model.service_name.clone(),
        refreshed_at_ms,
        nodes: vec![node],
    }
}

struct OperatorFleetNodeParts<'a> {
    data: &'a OperatorReadData,
    node_id: String,
    label: String,
    kind: FleetNodeKind,
    health: FleetNodeHealth,
    refreshed_at_ms: u64,
    active_endpoint: Option<String>,
    last_error: Option<String>,
    stale_since_ms: Option<u64>,
    snapshot_age_ms: Option<u64>,
}

fn build_operator_fleet_node(parts: OperatorFleetNodeParts<'_>) -> FleetNodeSnapshot {
    let OperatorFleetNodeParts {
        data,
        node_id,
        label,
        kind,
        health,
        refreshed_at_ms,
        active_endpoint,
        last_error,
        stale_since_ms,
        snapshot_age_ms,
    } = parts;
    let mut work_units = data
        .summary
        .sessions
        .iter()
        .map(|session| work_unit_from_operator_session(&node_id, session))
        .collect::<Vec<_>>();
    work_units.sort_by_key(|unit| std::cmp::Reverse(unit.last_activity_ms.unwrap_or(0)));

    FleetNodeSnapshot {
        node_id,
        label,
        kind,
        health,
        refreshed_at_ms,
        stale_since_ms,
        snapshot_age_ms,
        active_endpoint,
        last_error,
        processes: FleetProcessSummary::default(),
        topology: FleetTopology {
            status: FleetGraphStatus::Unavailable,
            edges: Vec::new(),
            note: Some("subagent graph source unavailable; operator session rows preserved".into()),
        },
        work_units,
    }
}

fn work_unit_from_operator_session(
    node_id: &str,
    session: &OperatorSessionSummary,
) -> FleetWorkUnit {
    let last_activity_ms = session
        .active_started_at_ms_min
        .max(session.last_ended_at_ms)
        .or(session.last_ended_at_ms)
        .or(session.active_started_at_ms_min);
    let state = if session.active_count > 0 {
        FleetWorkUnitState::Running
    } else if session.last_status.is_some_and(|status| status >= 400) {
        FleetWorkUnitState::Errored
    } else if session.last_ended_at_ms.is_some() || session.turns_total.unwrap_or(0) > 0 {
        FleetWorkUnitState::Completed
    } else {
        FleetWorkUnitState::Unknown
    };
    let evidence = if session.active_count > 0 || session.active_started_at_ms_min.is_some() {
        FleetEvidence::high(FleetEvidenceSource::RuntimeStatus)
    } else if session.last_ended_at_ms.is_some() {
        FleetEvidence::with_detail(
            FleetEvidenceSource::RuntimeStatus,
            FleetConfidence::Medium,
            "derived from operator request/session history",
        )
    } else {
        FleetEvidence::default()
    };

    FleetWorkUnit {
        node_id: node_id.to_string(),
        id: session.session_key.clone(),
        parent_id: None,
        kind: FleetWorkUnitKind::Root,
        state,
        evidence,
        session_id: Some(session.session_key.clone()),
        local_thread_id: Some(session.session_key.clone()),
        task_name: None,
        cwd: None,
        model: session
            .effective_model
            .as_ref()
            .map(|value| value.value.clone())
            .or_else(|| session.last_model.clone()),
        provider_id: session.last_provider_id.clone(),
        last_status: session.last_status,
        active_started_at_ms: session.active_started_at_ms_min,
        last_activity_ms,
        last_error: session
            .last_status
            .filter(|status| *status >= 400)
            .map(|status| format!("last status {status}")),
        usage: FleetUsageSummary {
            last_usage: session.last_usage.clone(),
            total_usage: session.total_usage.clone(),
            turns_total: session.turns_total,
            turns_with_usage: session.turns_with_usage,
            last_output_tokens_per_second: session.last_output_tokens_per_second,
            avg_output_tokens_per_second: session.avg_output_tokens_per_second,
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::dashboard_core::{
        ApiV1OperatorSummary, OperatorReadData, OperatorReadModel, OperatorRevisionBundle,
        OperatorSessionSummary,
    };
    use crate::state::SessionIdentityCard;
    use crate::usage::UsageMetrics;

    use super::*;

    fn card(session_id: &str) -> SessionIdentityCard {
        SessionIdentityCard {
            session_id: Some(session_id.to_string()),
            last_model: Some("gpt-5".to_string()),
            total_usage: Some(UsageMetrics {
                total_tokens: 12,
                output_tokens: 5,
                ..UsageMetrics::default()
            }),
            avg_output_tokens_per_second: Some(42.0),
            turns_total: Some(2),
            ..SessionIdentityCard::default()
        }
    }

    fn ready_operator_model() -> OperatorReadModel {
        let mut session = card("sid-operator");
        session.active_count = 1;
        session.active_started_at_ms_min = Some(100);
        OperatorReadModel::ready(
            "codex",
            200,
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
                    sessions: vec![OperatorSessionSummary::from_session_card(&session, 0)],
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
    fn operator_read_model_projection_enforces_four_state_fact_retention() {
        let ready = ready_operator_model();
        let ready_snapshot = build_fleet_snapshot_from_operator_read_model(
            &ready,
            "remote-a",
            "Remote A",
            FleetNodeKind::Remote,
            Some("https://admin.example".to_string()),
        );
        let ready_node = &ready_snapshot.nodes[0];
        assert_eq!(ready_node.health, FleetNodeHealth::Fresh);
        assert_eq!(ready_node.work_units.len(), 1);
        assert_eq!(
            ready_node.active_endpoint.as_deref(),
            Some("https://admin.example")
        );

        let stale = OperatorReadModel::stale_from(&ready);
        let stale_snapshot = build_fleet_snapshot_from_operator_read_model(
            &stale,
            "remote-a",
            "Remote A",
            FleetNodeKind::Remote,
            None,
        );
        assert_eq!(stale_snapshot.nodes[0].health, FleetNodeHealth::Stale);
        assert_eq!(stale_snapshot.nodes[0].work_units.len(), 1);

        for (model, expected_health) in [
            (
                OperatorReadModel::auth_required("codex"),
                FleetNodeHealth::AuthFailed,
            ),
            (
                OperatorReadModel::disconnected("codex"),
                FleetNodeHealth::Unreachable,
            ),
        ] {
            let snapshot = build_fleet_snapshot_from_operator_read_model(
                &model,
                "remote-a",
                "Remote A",
                FleetNodeKind::Remote,
                None,
            );
            assert_eq!(snapshot.nodes[0].health, expected_health);
            assert!(snapshot.nodes[0].work_units.is_empty());
        }
    }
}
