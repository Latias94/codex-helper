use super::sessions_controller_types::SessionRenderData;
use super::view_state::SessionsViewState;
use super::*;

pub(super) fn build_session_rows_for_snapshot(
    snapshot: &GuiRuntimeSnapshot,
) -> (bool, Vec<SessionRow>) {
    let has_session_cards = !snapshot.session_cards.is_empty();
    let mut rows = if has_session_cards {
        build_session_rows_from_cards(&snapshot.session_cards)
    } else {
        build_session_rows(
            snapshot.active.clone(),
            &snapshot.recent,
            &snapshot.session_model_overrides,
            &snapshot.session_effort_overrides,
            &snapshot.session_station_overrides,
            &snapshot.session_route_target_overrides,
            &snapshot.session_service_tier_overrides,
            snapshot.global_station_override.as_deref(),
            &snapshot.session_stats,
        )
    };
    for row in &mut rows {
        if let Some(session_id) = row.session_id.as_deref()
            && let Some(route_target) = snapshot.session_route_target_overrides.get(session_id)
        {
            row.override_route_target = Some(route_target.clone());
        }
    }
    (has_session_cards, rows)
}

pub(super) fn sync_default_profile_selection(
    state: &mut SessionsViewState,
    default_profile: Option<&str>,
    profiles: &[ControlProfileOption],
) {
    if state
        .default_profile_selection
        .as_ref()
        .is_none_or(|name| !profiles.iter().any(|profile| profile.name == *name))
    {
        state.default_profile_selection = default_profile
            .map(ToOwned::to_owned)
            .or_else(|| profiles.first().map(|profile| profile.name.clone()));
    }
}

pub(super) fn build_session_render_data(
    state: &mut SessionsViewState,
    rows: Vec<SessionRow>,
) -> SessionRenderData {
    let mut row_index_by_id = HashMap::new();
    for (idx, row) in rows.iter().enumerate() {
        row_index_by_id.insert(row.session_id.clone(), idx);
    }

    sync_session_order(state, &rows);

    let query = state.search.trim().to_lowercase();
    let filtered_indices = state
        .ordered_session_ids
        .iter()
        .filter_map(|id| row_index_by_id.get(id).copied())
        .filter(|idx| {
            let row = &rows[*idx];
            if state.active_only && row.active_count == 0 {
                return false;
            }
            if state.errors_only && row.last_status.is_some_and(|status| status < 400) {
                return false;
            }
            if state.overrides_only
                && row.override_model.is_none()
                && row.override_effort.is_none()
                && row.override_station_name().is_none()
                && row.override_route_target().is_none()
                && row.override_service_tier.is_none()
            {
                return false;
            }
            session_row_matches_query(row, &query)
        })
        .take(400)
        .collect::<Vec<_>>();

    let selected_idx_in_filtered = state
        .selected_session_id
        .as_deref()
        .and_then(|sid| {
            filtered_indices.iter().position(|idx| {
                rows.get(*idx).and_then(|row| row.session_id.as_deref()) == Some(sid)
            })
        })
        .unwrap_or(
            state
                .selected_idx
                .min(filtered_indices.len().saturating_sub(1)),
        );

    state.selected_idx = selected_idx_in_filtered;
    state.selected_session_id = filtered_indices
        .get(state.selected_idx)
        .and_then(|idx| rows.get(*idx))
        .and_then(|row| row.session_id.clone());

    SessionRenderData {
        rows,
        filtered_indices,
        selected_idx_in_filtered,
    }
}

pub(super) fn sync_session_editor_from_selection(
    state: &mut SessionsViewState,
    selected: Option<&SessionRow>,
    profiles: &[ControlProfileOption],
    default_profile: Option<&str>,
    use_route_target_overrides: bool,
) {
    let selected_sid = selected.and_then(|row| row.session_id.clone());
    if state.editor.sid == selected_sid {
        return;
    }

    state.editor.sid = selected_sid;
    state.editor.profile_selection = selected
        .and_then(|row| row.binding_profile_name.clone())
        .filter(|name| profiles.iter().any(|profile| profile.name == *name))
        .or_else(|| default_profile.map(ToOwned::to_owned))
        .or_else(|| profiles.first().map(|profile| profile.name.clone()));
    state.editor.model_override = selected
        .and_then(|row| row.override_model.clone())
        .unwrap_or_default();
    state.editor.config_override = selected.and_then(|row| {
        if use_route_target_overrides {
            row.override_route_target().map(str::to_owned)
        } else {
            row.override_station_name().map(str::to_owned)
        }
    });
    state.editor.effort_override = selected.and_then(|row| row.override_effort.clone());
    state.editor.custom_effort = selected
        .and_then(|row| row.override_effort.clone())
        .unwrap_or_default();
    state.editor.service_tier_override = selected.and_then(|row| row.override_service_tier.clone());
    state.editor.custom_service_tier = selected
        .and_then(|row| row.override_service_tier.clone())
        .unwrap_or_default();
}
