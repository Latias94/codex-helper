use super::session_rows_sources::{
    empty_observed_session_row, observed_session_row_from_active, observed_session_row_from_recent,
    observed_session_row_from_stats, session_sort_key, update_session_row_route_decision,
};
use super::*;

fn merge_active_request_row(entry: &mut SessionRow, req: ActiveRequest) {
    entry.active_count = entry.active_count.saturating_add(1);
    entry.active_started_at_ms_min = Some(
        entry
            .active_started_at_ms_min
            .unwrap_or(req.started_at_ms)
            .min(req.started_at_ms),
    );
    if entry.cwd.is_none() {
        entry.cwd = req.cwd;
    }
    if entry.last_client_name.is_none() {
        entry.last_client_name = req.client_name;
    }
    if entry.last_client_addr.is_none() {
        entry.last_client_addr = req.client_addr;
    }
    if let Some(effort) = req.reasoning_effort {
        entry.last_reasoning_effort = Some(effort);
    }
    if let Some(service_tier) = req.service_tier {
        entry.last_service_tier = Some(service_tier);
    }
    if entry.last_model.is_none() {
        entry.last_model = req.model;
    }
    if entry.last_provider_id.is_none() {
        entry.last_provider_id = req.provider_id;
    }
    if entry.last_station.is_none() {
        entry.last_station = req.station_name;
    }
    if entry.last_upstream_base_url.is_none() {
        entry.last_upstream_base_url = req.upstream_base_url;
    }
    update_session_row_route_decision(&mut entry.last_route_decision, req.route_decision.as_ref());
}

fn merge_recent_request_row(entry: &mut SessionRow, request: &FinishedRequest) {
    let should_update = entry
        .last_ended_at_ms
        .is_none_or(|prev| request.ended_at_ms >= prev);
    if should_update {
        entry.last_status = Some(request.status_code);
        entry.last_duration_ms = Some(request.duration_ms);
        entry.last_ended_at_ms = Some(request.ended_at_ms);
        entry.last_client_name = request
            .client_name
            .clone()
            .or(entry.last_client_name.clone());
        entry.last_client_addr = request
            .client_addr
            .clone()
            .or(entry.last_client_addr.clone());
        entry.last_model = request.model.clone().or(entry.last_model.clone());
        entry.last_reasoning_effort = request
            .reasoning_effort
            .clone()
            .or(entry.last_reasoning_effort.clone());
        entry.last_service_tier = request
            .service_tier
            .clone()
            .or(entry.last_service_tier.clone());
        entry.last_provider_id = request
            .provider_id
            .clone()
            .or(entry.last_provider_id.clone());
        entry.last_station = request.station_name.clone().or(entry.last_station.clone());
        entry.last_upstream_base_url = request
            .upstream_base_url
            .clone()
            .or(entry.last_upstream_base_url.clone());
        entry.last_usage = request.usage.clone().or(entry.last_usage.clone());
    }
    if entry.cwd.is_none() {
        entry.cwd = request.cwd.clone();
    }
    update_session_row_route_decision(
        &mut entry.last_route_decision,
        request.route_decision.as_ref(),
    );
}

fn merge_session_stats_row(entry: &mut SessionRow, session_stats: &SessionStats) {
    if entry.turns_total.is_none() {
        entry.turns_total = Some(session_stats.turns_total);
    }
    if entry.last_client_name.is_none() {
        entry.last_client_name = session_stats.last_client_name.clone();
    }
    if entry.last_client_addr.is_none() {
        entry.last_client_addr = session_stats.last_client_addr.clone();
    }
    if entry.last_status.is_none() {
        entry.last_status = session_stats.last_status;
    }
    if entry.last_duration_ms.is_none() {
        entry.last_duration_ms = session_stats.last_duration_ms;
    }
    if entry.last_ended_at_ms.is_none() {
        entry.last_ended_at_ms = session_stats.last_ended_at_ms;
    }
    if entry.last_model.is_none() {
        entry.last_model = session_stats.last_model.clone();
    }
    if entry.last_reasoning_effort.is_none() {
        entry.last_reasoning_effort = session_stats.last_reasoning_effort.clone();
    }
    if entry.last_service_tier.is_none() {
        entry.last_service_tier = session_stats.last_service_tier.clone();
    }
    if entry.last_provider_id.is_none() {
        entry.last_provider_id = session_stats.last_provider_id.clone();
    }
    if entry.last_station.is_none() {
        entry.last_station = session_stats.last_station_name.clone();
    }
    if entry.last_usage.is_none() {
        entry.last_usage = session_stats.last_usage.clone();
    }
    if entry.total_usage.is_none() {
        entry.total_usage = Some(session_stats.total_usage.clone());
    }
    if entry.turns_with_usage.is_none() {
        entry.turns_with_usage = Some(session_stats.turns_with_usage);
    }
    update_session_row_route_decision(
        &mut entry.last_route_decision,
        session_stats.last_route_decision.as_ref(),
    );
}

fn apply_session_overrides(
    map: &mut std::collections::HashMap<Option<String>, SessionRow>,
    model_overrides: &HashMap<String, String>,
    overrides: &HashMap<String, String>,
    station_overrides: &HashMap<String, String>,
    service_tier_overrides: &HashMap<String, String>,
) {
    for (session_id, model) in model_overrides.iter() {
        let entry = map
            .entry(Some(session_id.clone()))
            .or_insert_with(|| empty_observed_session_row(Some(session_id.clone())));
        entry.override_model = Some(model.clone());
    }

    for (session_id, effort) in overrides.iter() {
        let entry = map
            .entry(Some(session_id.clone()))
            .or_insert_with(|| empty_observed_session_row(Some(session_id.clone())));
        entry.override_effort = Some(effort.clone());
    }

    for (session_id, station_name) in station_overrides.iter() {
        let entry = map
            .entry(Some(session_id.clone()))
            .or_insert_with(|| empty_observed_session_row(Some(session_id.clone())));
        entry.override_station = Some(station_name.clone());
    }

    for (session_id, service_tier) in service_tier_overrides.iter() {
        let entry = map
            .entry(Some(session_id.clone()))
            .or_insert_with(|| empty_observed_session_row(Some(session_id.clone())));
        entry.override_service_tier = Some(service_tier.clone());
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_session_rows(
    active: Vec<ActiveRequest>,
    recent: &[FinishedRequest],
    model_overrides: &HashMap<String, String>,
    overrides: &HashMap<String, String>,
    station_overrides: &HashMap<String, String>,
    service_tier_overrides: &HashMap<String, String>,
    global_station_override: Option<&str>,
    stats: &HashMap<String, SessionStats>,
) -> Vec<SessionRow> {
    use std::collections::HashMap as StdHashMap;

    let mut map: StdHashMap<Option<String>, SessionRow> = StdHashMap::new();

    for req in active {
        let entry = map
            .entry(req.session_id.clone())
            .or_insert_with(|| observed_session_row_from_active(&req));
        merge_active_request_row(entry, req);
    }

    for request in recent {
        let entry = map
            .entry(request.session_id.clone())
            .or_insert_with(|| observed_session_row_from_recent(request));
        merge_recent_request_row(entry, request);
    }

    for (session_id, session_stats) in stats.iter() {
        let key = Some(session_id.clone());
        let entry = map
            .entry(key.clone())
            .or_insert_with(|| observed_session_row_from_stats(key, session_stats));
        merge_session_stats_row(entry, session_stats);
    }

    apply_session_overrides(
        &mut map,
        model_overrides,
        overrides,
        station_overrides,
        service_tier_overrides,
    );

    let mut rows = map.into_values().collect::<Vec<_>>();
    for row in &mut rows {
        if row.cwd.is_some() {
            row.observation_scope = SessionObservationScope::HostLocalEnriched;
        }
        apply_effective_route_to_row(row, global_station_override);
    }
    rows.sort_by_key(|row| std::cmp::Reverse(session_sort_key(row)));
    rows
}
