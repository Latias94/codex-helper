use super::session_rows_sources::session_sort_key;
use super::*;

pub(super) fn build_session_rows_from_cards(cards: &[SessionIdentityCard]) -> Vec<SessionRow> {
    let mut rows = cards
        .iter()
        .map(|card| SessionRow {
            session_id: card.session_id.clone(),
            observation_scope: card.observation_scope,
            host_local_transcript_path: card.host_local_transcript_path.clone(),
            last_client_name: card.last_client_name.clone(),
            last_client_addr: card.last_client_addr.clone(),
            cwd: card.cwd.clone(),
            active_count: card.active_count,
            active_started_at_ms_min: card.active_started_at_ms_min,
            last_status: card.last_status,
            last_duration_ms: card.last_duration_ms,
            last_ended_at_ms: card.last_ended_at_ms,
            last_model: card.last_model.clone(),
            last_reasoning_effort: card.last_reasoning_effort.clone(),
            last_service_tier: card.last_service_tier.clone(),
            last_provider_id: card.last_provider_id.clone(),
            last_station: card.last_station_name.clone(),
            last_upstream_base_url: card.last_upstream_base_url.clone(),
            last_usage: card.last_usage.clone(),
            total_usage: card.total_usage.clone(),
            turns_total: card.turns_total,
            turns_with_usage: card.turns_with_usage,
            binding_profile_name: card.binding_profile_name.clone(),
            binding_continuity_mode: card.binding_continuity_mode,
            last_route_decision: card.last_route_decision.clone(),
            effective_model: card.effective_model.clone(),
            effective_reasoning_effort: card.effective_reasoning_effort.clone(),
            effective_service_tier: card.effective_service_tier.clone(),
            effective_station_value: card.effective_station.clone(),
            effective_upstream_base_url: card.effective_upstream_base_url.clone(),
            override_model: card.override_model.clone(),
            override_effort: card.override_effort.clone(),
            override_station: card.override_station_name.clone(),
            override_service_tier: card.override_service_tier.clone(),
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| std::cmp::Reverse(session_sort_key(row)));
    rows
}
