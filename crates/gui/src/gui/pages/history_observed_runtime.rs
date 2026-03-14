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
        return super::build_session_rows_from_cards(std::slice::from_ref(card))
            .into_iter()
            .next();
    }

    super::build_session_rows(
        snapshot.active.clone(),
        &snapshot.recent,
        &snapshot.session_model_overrides,
        &snapshot.session_effort_overrides,
        &snapshot.session_station_overrides,
        &snapshot.session_service_tier_overrides,
        snapshot.global_station_override.as_deref(),
        &snapshot.session_stats,
    )
    .into_iter()
    .find(|row| row.session_id.as_deref() == Some(session_id))
}

pub(super) fn refresh_history_sessions_with_fallback(
    ctx: &mut PageCtx<'_>,
    scope: HistoryScope,
    observed_fallback_supported: bool,
) -> anyhow::Result<(Vec<SessionSummary>, HistoryDataSource)> {
    let recent_since_minutes = ctx.view.history.recent_since_minutes;
    let recent_limit = ctx.view.history.recent_limit;
    let local_result = ctx.rt.block_on(async move {
        match scope {
            HistoryScope::CurrentProject => {
                crate::sessions::find_codex_sessions_for_current_dir(200).await
            }
            HistoryScope::GlobalRecent => {
                let since = std::time::Duration::from_secs(
                    (recent_since_minutes as u64).saturating_mul(60),
                );
                crate::sessions::find_recent_codex_session_summaries(since, recent_limit).await
            }
            HistoryScope::AllByDate => Ok(Vec::new()),
        }
    });

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
