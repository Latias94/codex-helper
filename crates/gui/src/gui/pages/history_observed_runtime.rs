use super::history::{HistoryDataSource, HistoryScope};
use super::*;

pub(super) fn history_session_supports_local_actions(summary: &SessionSummary) -> bool {
    matches!(summary.source, SessionSummarySource::LocalFile)
}

pub(super) fn observed_session_row_from_snapshot(
    snapshot: &GuiRuntimeSnapshot,
    session_id: &str,
) -> Option<super::SessionRow> {
    if let Some(card) = snapshot
        .session_cards
        .iter()
        .find(|card| card.session_id.as_deref() == Some(session_id))
    {
        let mut row = super::build_session_rows_from_cards(std::slice::from_ref(card))
            .into_iter()
            .next();
        if let Some(row) = row.as_mut()
            && let Some(route_target) = snapshot.session_route_target_overrides.get(session_id)
        {
            row.override_route_target = Some(route_target.clone());
        }
        return row;
    }

    super::build_session_rows(
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
    .into_iter()
    .find(|row| row.session_id.as_deref() == Some(session_id))
}

pub(super) async fn load_local_history_sessions(
    scope: HistoryScope,
    recent_since_minutes: u32,
    recent_limit: usize,
) -> anyhow::Result<Vec<SessionSummary>> {
    match scope {
        HistoryScope::CurrentProject => {
            crate::sessions::find_codex_sessions_for_current_dir(200).await
        }
        HistoryScope::GlobalRecent => {
            let since =
                std::time::Duration::from_secs((recent_since_minutes as u64).saturating_mul(60));
            crate::sessions::find_recent_codex_session_summaries(since, recent_limit).await
        }
        HistoryScope::AllByDate => Ok(Vec::new()),
    }
}

pub(super) fn resolve_history_sessions_with_fallback(
    ctx: &mut PageCtx<'_>,
    _scope: HistoryScope,
    observed_fallback_supported: bool,
    local_result: anyhow::Result<Vec<SessionSummary>>,
) -> anyhow::Result<(Vec<SessionSummary>, HistoryDataSource)> {
    match local_result {
        Ok(mut list) => {
            if !list.is_empty() || !observed_fallback_supported {
                sort_session_summaries_by_mtime_desc(&mut list);
                return Ok((list, HistoryDataSource::LocalFiles));
            }

            let observed = ctx
                .proxy
                .snapshot()
                .map(|snapshot| {
                    super::history_observed_summary::build_observed_history_summaries(
                        &snapshot, ctx.lang,
                    )
                })
                .unwrap_or_default();
            if !observed.is_empty() {
                Ok((observed, HistoryDataSource::ObservedFallback))
            } else {
                sort_session_summaries_by_mtime_desc(&mut list);
                Ok((list, HistoryDataSource::LocalFiles))
            }
        }
        Err(err) => {
            if !observed_fallback_supported {
                return Err(err);
            }
            let observed = ctx
                .proxy
                .snapshot()
                .map(|snapshot| {
                    super::history_observed_summary::build_observed_history_summaries(
                        &snapshot, ctx.lang,
                    )
                })
                .unwrap_or_default();
            if !observed.is_empty() {
                Ok((observed, HistoryDataSource::ObservedFallback))
            } else {
                Err(err)
            }
        }
    }
}
