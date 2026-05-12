use super::*;

pub(super) fn sync_session_order(state: &mut SessionsViewState, rows: &[SessionRow]) {
    let mut current_set: HashSet<Option<String>> = HashSet::new();
    let mut active_set: HashSet<Option<String>> = HashSet::new();
    for row in rows {
        current_set.insert(row.session_id.clone());
        if row.active_count > 0 {
            active_set.insert(row.session_id.clone());
        }
    }

    if state.ordered_session_ids.is_empty() {
        state.ordered_session_ids = rows.iter().map(|row| row.session_id.clone()).collect();
        state.last_active_set = active_set;
        return;
    }

    state
        .ordered_session_ids
        .retain(|session_id| current_set.contains(session_id));

    let mut known: HashSet<Option<String>> = state.ordered_session_ids.iter().cloned().collect();
    let mut missing_active: Vec<Option<String>> = Vec::new();
    let mut missing_inactive: Vec<Option<String>> = Vec::new();
    for row in rows {
        if known.contains(&row.session_id) {
            continue;
        }
        known.insert(row.session_id.clone());
        if active_set.contains(&row.session_id) {
            missing_active.push(row.session_id.clone());
        } else {
            missing_inactive.push(row.session_id.clone());
        }
    }

    if state.lock_order {
        state.ordered_session_ids.extend(missing_active);
        state.ordered_session_ids.extend(missing_inactive);
        state.last_active_set = active_set;
        return;
    }

    let mut active_ids: Vec<Option<String>> = Vec::new();
    let mut inactive_ids: Vec<Option<String>> = Vec::new();
    for session_id in state.ordered_session_ids.drain(..) {
        if active_set.contains(&session_id) {
            active_ids.push(session_id);
        } else {
            inactive_ids.push(session_id);
        }
    }
    state.ordered_session_ids.extend(active_ids);
    state.ordered_session_ids.extend(inactive_ids);

    let insert_at = state
        .ordered_session_ids
        .iter()
        .take_while(|session_id| active_set.contains(*session_id))
        .count();
    let active_missing_len = missing_active.len();
    state
        .ordered_session_ids
        .splice(insert_at..insert_at, missing_active);
    let insert_at2 = insert_at + active_missing_len;
    state
        .ordered_session_ids
        .splice(insert_at2..insert_at2, missing_inactive);

    state.last_active_set = active_set;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionRow {
    pub(super) session_id: Option<String>,
    pub(super) observation_scope: SessionObservationScope,
    pub(super) host_local_transcript_path: Option<String>,
    pub(super) last_client_name: Option<String>,
    pub(super) last_client_addr: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) active_count: u64,
    pub(super) active_started_at_ms_min: Option<u64>,
    pub(super) last_status: Option<u16>,
    pub(super) last_duration_ms: Option<u64>,
    pub(super) last_ended_at_ms: Option<u64>,
    pub(super) last_model: Option<String>,
    pub(super) last_reasoning_effort: Option<String>,
    pub(super) last_service_tier: Option<String>,
    pub(super) last_provider_id: Option<String>,
    pub(super) last_station: Option<String>,
    pub(super) last_upstream_base_url: Option<String>,
    pub(super) last_usage: Option<UsageMetrics>,
    pub(super) total_usage: Option<UsageMetrics>,
    pub(super) turns_total: Option<u64>,
    pub(super) turns_with_usage: Option<u64>,
    pub(super) binding_profile_name: Option<String>,
    pub(super) binding_continuity_mode: Option<crate::state::SessionContinuityMode>,
    pub(super) last_route_decision: Option<RouteDecisionProvenance>,
    pub(super) route_affinity: Option<SessionRouteAffinity>,
    pub(super) effective_model: Option<ResolvedRouteValue>,
    pub(super) effective_reasoning_effort: Option<ResolvedRouteValue>,
    pub(super) effective_service_tier: Option<ResolvedRouteValue>,
    pub(super) effective_station_value: Option<ResolvedRouteValue>,
    pub(super) effective_upstream_base_url: Option<ResolvedRouteValue>,
    pub(super) override_model: Option<String>,
    pub(super) override_effort: Option<String>,
    pub(super) override_station: Option<String>,
    pub(super) override_service_tier: Option<String>,
}

impl SessionRow {
    pub(super) fn last_station_name(&self) -> Option<&str> {
        self.last_station.as_deref()
    }

    pub(super) fn effective_station(&self) -> Option<&ResolvedRouteValue> {
        self.effective_station_value.as_ref()
    }

    pub(super) fn effective_station_name(&self) -> Option<&str> {
        self.effective_station()
            .map(|resolved| resolved.value.as_str())
    }

    pub(super) fn effective_station_source(&self) -> Option<RouteValueSource> {
        self.effective_station().map(|resolved| resolved.source)
    }

    pub(super) fn override_station_name(&self) -> Option<&str> {
        self.override_station.as_deref()
    }
}
