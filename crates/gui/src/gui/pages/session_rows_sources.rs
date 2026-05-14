use super::*;

pub(in crate::gui::pages) fn empty_observed_session_row(session_id: Option<String>) -> SessionRow {
    SessionRow {
        session_id,
        observation_scope: SessionObservationScope::ObservedOnly,
        host_local_transcript_path: None,
        last_client_name: None,
        last_client_addr: None,
        cwd: None,
        active_count: 0,
        active_started_at_ms_min: None,
        last_status: None,
        last_duration_ms: None,
        last_ended_at_ms: None,
        last_model: None,
        last_reasoning_effort: None,
        last_service_tier: None,
        last_provider_id: None,
        last_station: None,
        last_upstream_base_url: None,
        last_usage: None,
        total_usage: None,
        turns_total: None,
        turns_with_usage: None,
        binding_profile_name: None,
        binding_continuity_mode: None,
        last_route_decision: None,
        route_affinity: None,
        effective_model: None,
        effective_reasoning_effort: None,
        effective_service_tier: None,
        effective_station_value: None,
        effective_upstream_base_url: None,
        override_model: None,
        override_effort: None,
        override_station: None,
        override_route_target: None,
        override_service_tier: None,
    }
}

pub(in crate::gui::pages) fn observed_session_row_from_active(req: &ActiveRequest) -> SessionRow {
    let mut row = empty_observed_session_row(req.session_id.clone());
    row.last_client_name = req.client_name.clone();
    row.last_client_addr = req.client_addr.clone();
    row.cwd = req.cwd.clone();
    row.active_started_at_ms_min = Some(req.started_at_ms);
    row.last_model = req.model.clone();
    row.last_reasoning_effort = req.reasoning_effort.clone();
    row.last_service_tier = req.service_tier.clone();
    row.last_provider_id = req.provider_id.clone();
    row.last_station = req.station_name.clone();
    row.last_upstream_base_url = req.upstream_base_url.clone();
    row.last_route_decision = req.route_decision.clone();
    row.route_affinity = None;
    row
}

pub(in crate::gui::pages) fn observed_session_row_from_recent(
    request: &FinishedRequest,
) -> SessionRow {
    let mut row = empty_observed_session_row(request.session_id.clone());
    row.last_client_name = request.client_name.clone();
    row.last_client_addr = request.client_addr.clone();
    row.cwd = request.cwd.clone();
    row.last_model = request.model.clone();
    row.last_reasoning_effort = request.reasoning_effort.clone();
    row.last_service_tier = request.service_tier.clone();
    row.last_provider_id = request.provider_id.clone();
    row.last_station = request.station_name.clone();
    row.last_upstream_base_url = request.upstream_base_url.clone();
    row.last_usage = request.usage.clone();
    row.last_route_decision = request.route_decision.clone();
    row.route_affinity = None;
    row
}

pub(in crate::gui::pages) fn observed_session_row_from_stats(
    session_id: Option<String>,
    stats: &SessionStats,
) -> SessionRow {
    let mut row = empty_observed_session_row(session_id);
    row.last_route_decision = stats.last_route_decision.clone();
    row.route_affinity = None;
    row
}

pub(super) fn session_sort_key(row: &SessionRow) -> u64 {
    row.last_ended_at_ms
        .unwrap_or(0)
        .max(row.active_started_at_ms_min.unwrap_or(0))
}

pub(super) fn update_session_row_route_decision(
    slot: &mut Option<RouteDecisionProvenance>,
    candidate: Option<&RouteDecisionProvenance>,
) {
    let Some(candidate) = candidate.cloned() else {
        return;
    };
    let current_at = slot
        .as_ref()
        .map(|decision| decision.decided_at_ms)
        .unwrap_or(0);
    if current_at <= candidate.decided_at_ms {
        *slot = Some(candidate);
    }
}
