use super::view_state::SessionsViewState;
use super::*;

#[derive(Debug, Default)]
pub(super) struct SessionPageActions {
    pub(super) apply_session_profile: Option<(String, String)>,
    pub(super) clear_session_profile_binding: Option<String>,
    pub(super) clear_session_manual_overrides: Option<String>,
}

#[derive(Debug)]
pub(super) struct SessionRenderData {
    pub(super) rows: Vec<SessionRow>,
    pub(super) filtered_indices: Vec<usize>,
    pub(super) selected_idx_in_filtered: usize,
}

impl SessionRenderData {
    pub(super) fn filtered_rows(&self) -> impl Iterator<Item = &SessionRow> {
        self.filtered_indices
            .iter()
            .filter_map(|idx| self.rows.get(*idx))
    }

    pub(super) fn selected_row(&self) -> Option<&SessionRow> {
        self.filtered_indices
            .get(self.selected_idx_in_filtered)
            .and_then(|idx| self.rows.get(*idx))
    }
}

pub(super) fn build_runtime_station_catalog(
    snapshot: &GuiRuntimeSnapshot,
) -> BTreeMap<String, StationOption> {
    snapshot
        .stations
        .iter()
        .cloned()
        .map(|config| (config.name.clone(), config))
        .collect()
}

pub(super) fn resolve_session_preview_catalogs(
    ctx: &PageCtx<'_>,
    session_preview_service_name: &str,
) -> Option<(
    BTreeMap<String, PersistedStationSpec>,
    BTreeMap<String, PersistedStationProviderRef>,
)> {
    ctx.proxy
        .attached()
        .and_then(|att| {
            att.supports_station_spec_api.then(|| {
                (
                    att.persisted_stations.clone(),
                    att.persisted_station_providers.clone(),
                )
            })
        })
        .or_else(|| {
            if matches!(ctx.proxy.kind(), ProxyModeKind::Attached) {
                None
            } else {
                local_profile_preview_catalogs_from_text(
                    ctx.proxy_config_text,
                    session_preview_service_name,
                )
            }
        })
}

pub(super) fn build_session_rows_for_snapshot(
    snapshot: &GuiRuntimeSnapshot,
) -> (bool, Vec<SessionRow>) {
    let has_session_cards = !snapshot.session_cards.is_empty();
    let rows = if has_session_cards {
        build_session_rows_from_cards(&snapshot.session_cards)
    } else {
        build_session_rows(
            snapshot.active.clone(),
            &snapshot.recent,
            &snapshot.session_model_overrides,
            &snapshot.session_effort_overrides,
            &snapshot.session_station_overrides,
            &snapshot.session_service_tier_overrides,
            snapshot.global_station_override.as_deref(),
            &snapshot.session_stats,
        )
    };
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
    state.editor.config_override =
        selected.and_then(|row| row.override_station_name().map(str::to_owned));
    state.editor.effort_override = selected.and_then(|row| row.override_effort.clone());
    state.editor.custom_effort = selected
        .and_then(|row| row.override_effort.clone())
        .unwrap_or_default();
    state.editor.service_tier_override = selected.and_then(|row| row.override_service_tier.clone());
    state.editor.custom_service_tier = selected
        .and_then(|row| row.override_service_tier.clone())
        .unwrap_or_default();
}

pub(super) fn apply_session_page_actions(
    ctx: &mut PageCtx<'_>,
    actions: SessionPageActions,
    default_profile: Option<&str>,
    profiles: &[ControlProfileOption],
) -> bool {
    let mut force_refresh = false;

    if let Some((sid, profile_name)) = actions.apply_session_profile {
        match ctx
            .proxy
            .apply_session_profile(ctx.rt, sid, profile_name.clone())
        {
            Ok(()) => {
                force_refresh = true;
                *ctx.last_info = Some(format!(
                    "{}: {profile_name}",
                    pick(ctx.lang, "已应用 profile", "Profile applied")
                ));
            }
            Err(e) => {
                *ctx.last_error = Some(format!("apply profile failed: {e}"));
            }
        }
    }

    if let Some(sid) = actions.clear_session_manual_overrides {
        match ctx.proxy.clear_session_manual_overrides(ctx.rt, sid) {
            Ok(()) => {
                force_refresh = true;
                ctx.view.sessions.editor.model_override.clear();
                ctx.view.sessions.editor.config_override = None;
                ctx.view.sessions.editor.effort_override = None;
                ctx.view.sessions.editor.custom_effort.clear();
                ctx.view.sessions.editor.service_tier_override = None;
                ctx.view.sessions.editor.custom_service_tier.clear();
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已重置 session manual overrides",
                        "Session manual overrides reset",
                    )
                    .to_string(),
                );
            }
            Err(e) => {
                *ctx.last_error = Some(format!("reset session manual overrides failed: {e}"));
            }
        }
    }

    if let Some(sid) = actions.clear_session_profile_binding {
        match ctx.proxy.clear_session_profile_binding(ctx.rt, sid) {
            Ok(()) => {
                force_refresh = true;
                ctx.view.sessions.editor.profile_selection = default_profile
                    .map(ToOwned::to_owned)
                    .or_else(|| profiles.first().map(|profile| profile.name.clone()));
                *ctx.last_info = Some(
                    pick(
                        ctx.lang,
                        "已清除 profile binding",
                        "Profile binding cleared",
                    )
                    .to_string(),
                );
            }
            Err(e) => {
                *ctx.last_error = Some(format!("clear profile binding failed: {e}"));
            }
        }
    }

    force_refresh
}
