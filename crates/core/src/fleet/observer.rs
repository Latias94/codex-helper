use std::collections::HashMap;

use crate::dashboard_core::build_dashboard_snapshot;
use crate::dashboard_core::snapshot::DashboardSnapshot;
use crate::state::{ActiveRequest, FinishedRequest, ProxyState, SessionIdentityCard};

use super::model::{
    FleetConfidence, FleetEvidence, FleetEvidenceSource, FleetGraphStatus, FleetNodeHealth,
    FleetNodeKind, FleetNodeSnapshot, FleetProcessSummary, FleetSnapshot, FleetTopology,
    FleetUsageSummary, FleetWorkUnit, FleetWorkUnitKind, FleetWorkUnitState, now_ms,
};
use super::process_scan::scan_codex_processes;

pub async fn build_local_fleet_snapshot(
    state: &ProxyState,
    service_name: &str,
    node_id: impl Into<String>,
    label: impl Into<String>,
) -> FleetSnapshot {
    let refreshed_at_ms = now_ms();
    let dashboard = build_dashboard_snapshot(state, service_name, 2_000, 7).await;
    let process_scan = scan_codex_processes();
    let node = build_local_fleet_node_from_parts(LocalFleetNodeParts {
        node_id: node_id.into(),
        label: label.into(),
        refreshed_at_ms,
        processes: process_scan.summary(),
        session_cards: &dashboard.session_cards,
        active: &dashboard.active,
        recent: &dashboard.recent,
    });

    FleetSnapshot {
        api_version: 1,
        service_name: service_name.to_string(),
        refreshed_at_ms,
        nodes: vec![node],
    }
}

pub fn build_local_fleet_snapshot_from_parts(
    service_name: &str,
    node_id: impl Into<String>,
    label: impl Into<String>,
    refreshed_at_ms: u64,
    session_cards: &[SessionIdentityCard],
    active: &[ActiveRequest],
    recent: &[FinishedRequest],
) -> FleetSnapshot {
    let node = build_local_fleet_node_from_parts(LocalFleetNodeParts {
        node_id: node_id.into(),
        label: label.into(),
        refreshed_at_ms,
        processes: FleetProcessSummary::default(),
        session_cards,
        active,
        recent,
    });

    FleetSnapshot {
        api_version: 1,
        service_name: service_name.to_string(),
        refreshed_at_ms,
        nodes: vec![node],
    }
}

pub fn build_local_fleet_snapshot_from_dashboard(
    service_name: &str,
    node_id: impl Into<String>,
    label: impl Into<String>,
    dashboard: &DashboardSnapshot,
) -> FleetSnapshot {
    let node = build_local_fleet_node_from_parts(LocalFleetNodeParts {
        node_id: node_id.into(),
        label: label.into(),
        refreshed_at_ms: dashboard.refreshed_at_ms,
        processes: scan_codex_processes().summary(),
        session_cards: &dashboard.session_cards,
        active: &dashboard.active,
        recent: &dashboard.recent,
    });

    FleetSnapshot {
        api_version: 1,
        service_name: service_name.to_string(),
        refreshed_at_ms: dashboard.refreshed_at_ms,
        nodes: vec![node],
    }
}

struct LocalFleetNodeParts<'a> {
    node_id: String,
    label: String,
    refreshed_at_ms: u64,
    processes: FleetProcessSummary,
    session_cards: &'a [SessionIdentityCard],
    active: &'a [ActiveRequest],
    recent: &'a [FinishedRequest],
}

fn build_local_fleet_node_from_parts(parts: LocalFleetNodeParts<'_>) -> FleetNodeSnapshot {
    let LocalFleetNodeParts {
        node_id,
        label,
        refreshed_at_ms,
        processes,
        session_cards,
        active,
        recent,
    } = parts;

    let active_by_session = active
        .iter()
        .filter_map(|request| request.session_id.as_deref().map(|sid| (sid, request)))
        .fold(
            HashMap::<&str, Vec<&ActiveRequest>>::new(),
            |mut acc, (sid, request)| {
                acc.entry(sid).or_default().push(request);
                acc
            },
        );
    let recent_by_session = recent
        .iter()
        .filter_map(|request| request.session_id.as_deref().map(|sid| (sid, request)))
        .fold(
            HashMap::<&str, Vec<&FinishedRequest>>::new(),
            |mut acc, (sid, request)| {
                acc.entry(sid).or_default().push(request);
                acc
            },
        );

    let mut work_units = session_cards
        .iter()
        .enumerate()
        .map(|(idx, card)| {
            work_unit_from_session_card(
                &node_id,
                idx,
                card,
                card.session_id
                    .as_deref()
                    .and_then(|sid| active_by_session.get(sid).map(Vec::as_slice)),
                card.session_id
                    .as_deref()
                    .and_then(|sid| recent_by_session.get(sid).map(Vec::as_slice)),
            )
        })
        .collect::<Vec<_>>();

    work_units.sort_by_key(|unit| std::cmp::Reverse(unit.last_activity_ms.unwrap_or(0)));

    FleetNodeSnapshot {
        node_id,
        label,
        kind: FleetNodeKind::Local,
        health: FleetNodeHealth::Fresh,
        refreshed_at_ms,
        stale_since_ms: None,
        snapshot_age_ms: Some(0),
        active_endpoint: None,
        last_error: None,
        processes,
        topology: FleetTopology {
            status: FleetGraphStatus::Unavailable,
            edges: Vec::new(),
            note: Some(
                "subagent graph source unavailable; node-local session rows preserved".into(),
            ),
        },
        work_units,
    }
}

fn work_unit_from_session_card(
    node_id: &str,
    idx: usize,
    card: &SessionIdentityCard,
    active: Option<&[&ActiveRequest]>,
    recent: Option<&[&FinishedRequest]>,
) -> FleetWorkUnit {
    let id = card
        .session_id
        .as_ref()
        .map(|sid| format!("session:{sid}"))
        .unwrap_or_else(|| format!("unknown-session:{idx}"));
    let active_started_at_ms = active
        .and_then(|requests| requests.iter().map(|r| r.started_at_ms).min())
        .or(card.active_started_at_ms_min);
    let last_recent = recent.and_then(|requests| {
        requests
            .iter()
            .max_by_key(|request| request.ended_at_ms)
            .copied()
    });
    let last_activity_ms = active_started_at_ms
        .max(card.last_ended_at_ms)
        .or(card.last_ended_at_ms)
        .or(active_started_at_ms);
    let state = infer_work_unit_state(card, last_recent);
    let evidence = infer_work_unit_evidence(card, active, recent);
    let last_error = last_recent
        .filter(|request| request.status_code >= 400)
        .map(|request| format!("last status {}", request.status_code));

    FleetWorkUnit {
        node_id: node_id.to_string(),
        id,
        parent_id: None,
        kind: FleetWorkUnitKind::Root,
        state,
        evidence,
        session_id: card.session_id.clone(),
        local_thread_id: card.session_id.clone(),
        task_name: None,
        cwd: card.cwd.clone(),
        model: card
            .effective_model
            .as_ref()
            .map(|value| value.value.clone())
            .or_else(|| card.last_model.clone()),
        station_name: card
            .effective_station
            .as_ref()
            .map(|value| value.value.clone())
            .or_else(|| card.last_station_name.clone()),
        provider_id: card.last_provider_id.clone(),
        last_status: card.last_status,
        active_started_at_ms,
        last_activity_ms,
        last_error,
        usage: FleetUsageSummary {
            last_usage: card.last_usage.clone(),
            total_usage: card.total_usage.clone(),
            turns_total: card.turns_total,
            turns_with_usage: card.turns_with_usage,
            last_output_tokens_per_second: card.last_output_tokens_per_second,
            avg_output_tokens_per_second: card.avg_output_tokens_per_second,
        },
    }
}

fn infer_work_unit_state(
    card: &SessionIdentityCard,
    last_recent: Option<&FinishedRequest>,
) -> FleetWorkUnitState {
    if card.active_count > 0 {
        return FleetWorkUnitState::Running;
    }
    if let Some(status) = card.last_status
        && status >= 400
    {
        return FleetWorkUnitState::Errored;
    }
    if let Some(request) = last_recent
        && request.status_code >= 400
    {
        return FleetWorkUnitState::Errored;
    }
    if card.last_ended_at_ms.is_some() || card.turns_total.unwrap_or(0) > 0 {
        return FleetWorkUnitState::Completed;
    }
    FleetWorkUnitState::Unknown
}

fn infer_work_unit_evidence(
    card: &SessionIdentityCard,
    active: Option<&[&ActiveRequest]>,
    recent: Option<&[&FinishedRequest]>,
) -> FleetEvidence {
    if card.active_count > 0 || card.active_started_at_ms_min.is_some() {
        return FleetEvidence::high(FleetEvidenceSource::RuntimeStatus);
    }
    if active.is_some_and(|requests| !requests.is_empty()) {
        return FleetEvidence::high(FleetEvidenceSource::RuntimeStatus);
    }
    if recent.is_some_and(|requests| !requests.is_empty()) || card.last_ended_at_ms.is_some() {
        return FleetEvidence::with_detail(
            FleetEvidenceSource::RuntimeStatus,
            FleetConfidence::Medium,
            "derived from request/session runtime history",
        );
    }
    if card.host_local_transcript_path.is_some() {
        return FleetEvidence::with_detail(
            FleetEvidenceSource::SessionLog,
            FleetConfidence::Medium,
            "derived from local Codex transcript path",
        );
    }
    FleetEvidence::default()
}

#[cfg(test)]
mod tests {
    use crate::state::{FinishedRequest, RequestObservability, SessionIdentityCard};
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

    fn finished(id: u64, session_id: &str, status_code: u16) -> FinishedRequest {
        FinishedRequest {
            id,
            trace_id: None,
            session_id: Some(session_id.to_string()),
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            station_name: None,
            provider_id: None,
            upstream_base_url: None,
            route_decision: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            observability: RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code,
            duration_ms: 10,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: id,
        }
    }

    #[test]
    fn local_snapshot_preserves_source_confidence_and_usage() {
        let mut c = card("sid-1");
        c.active_count = 1;
        c.active_started_at_ms_min = Some(100);

        let snapshot =
            build_local_fleet_snapshot_from_parts("codex", "local", "Local", 200, &[c], &[], &[]);

        let node = snapshot.nodes.first().expect("node");
        let unit = node.work_units.first().expect("unit");
        assert_eq!(unit.node_id, "local");
        assert_eq!(unit.state, FleetWorkUnitState::Running);
        assert_eq!(unit.evidence.source, FleetEvidenceSource::RuntimeStatus);
        assert_eq!(unit.evidence.confidence, FleetConfidence::High);
        assert_eq!(unit.usage.avg_output_tokens_per_second, Some(42.0));
        assert_eq!(
            unit.usage
                .total_usage
                .as_ref()
                .map(|usage| usage.total_tokens),
            Some(12)
        );
        assert_eq!(node.topology.status, FleetGraphStatus::Unavailable);
    }

    #[test]
    fn local_snapshot_marks_recent_error_without_active_request() {
        let mut c = card("sid-err");
        c.active_count = 0;
        c.last_status = Some(500);
        c.last_ended_at_ms = Some(300);
        let recent = vec![finished(300, "sid-err", 500)];

        let snapshot = build_local_fleet_snapshot_from_parts(
            "codex",
            "local",
            "Local",
            400,
            &[c],
            &[],
            &recent,
        );

        let unit = &snapshot.nodes[0].work_units[0];
        assert_eq!(unit.state, FleetWorkUnitState::Errored);
        assert_eq!(unit.last_error.as_deref(), Some("last status 500"));
        assert_eq!(unit.evidence.confidence, FleetConfidence::Medium);
    }
}
