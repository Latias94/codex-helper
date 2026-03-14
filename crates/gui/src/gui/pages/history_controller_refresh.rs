use super::history::refresh_branch_cache_for_sessions;
use super::history::{HistoryDataSource, HistoryScope};
use super::history_external::merge_external_focus_session;
use super::history_observed::refresh_history_sessions_with_fallback;
use super::history_transcript_runtime::cancel_transcript_load;
use super::*;

pub(in crate::gui::pages) fn clear_history_transcript_state(state: &mut HistoryViewState) {
    cancel_transcript_load(state);
    state.transcript_raw_messages.clear();
    state.transcript_messages.clear();
    state.transcript_error = None;
    state.loaded_for = None;
    state.transcript_plain_key = None;
    state.transcript_plain_text.clear();
}

pub(super) fn history_refresh_needed(state: &HistoryViewState, remote_attached: bool) -> bool {
    if remote_attached
        && state.scope != HistoryScope::AllByDate
        && state.loaded_at_ms.is_none()
        && state.sessions_all.is_empty()
    {
        return true;
    }
    state.external_focus.as_ref().is_some_and(|focus| {
        !state
            .sessions_all
            .iter()
            .any(|summary| summary.id == focus.summary.id)
    })
}

pub(super) fn refresh_history_sessions(
    ctx: &mut PageCtx<'_>,
    shared_observed_history_available: bool,
) {
    let scope = ctx.view.history.scope;
    match refresh_history_sessions_with_fallback(ctx, scope, shared_observed_history_available) {
        Ok((mut list, data_source)) => {
            if let Some(focus) = ctx.view.history.external_focus.as_ref() {
                merge_external_focus_session(&mut list, focus);
            }
            ctx.view.history.sessions_all = list;
            ctx.view.history.data_source = data_source;
            let infer_git_root = ctx.view.history.infer_git_root;
            let sessions = ctx.view.history.sessions_all.as_slice();
            refresh_branch_cache_for_sessions(
                &mut ctx.view.history.branch_by_workdir,
                infer_git_root,
                sessions,
            );
            ctx.view.history.search_transcript_applied = None;
            ctx.view.history.loaded_at_ms = Some(now_ms());
            ctx.view.history.last_error = None;
            super::history_controller_filter::apply_metadata_filter_to_history_state(
                &mut ctx.view.history,
            );

            if ctx.view.history.sessions.is_empty() {
                ctx.view.history.selected_idx = 0;
                ctx.view.history.selected_id = None;
                clear_history_transcript_state(&mut ctx.view.history);
            } else if ctx.view.history.selected_id.as_deref().is_none_or(|id| {
                !ctx.view
                    .history
                    .sessions
                    .iter()
                    .any(|session| session.id == id)
            }) {
                ctx.view.history.selected_idx = 0;
                ctx.view.history.selected_id = Some(ctx.view.history.sessions[0].id.clone());
                clear_history_transcript_state(&mut ctx.view.history);
            }
            *ctx.last_info = Some(if data_source == HistoryDataSource::ObservedFallback {
                pick(ctx.lang, "已刷新（共享观测）", "Refreshed (observed)").to_string()
            } else {
                pick(ctx.lang, "已刷新", "Refreshed").to_string()
            });
        }
        Err(error) => {
            ctx.view.history.last_error = Some(error.to_string());
        }
    }
}

pub(super) fn stabilize_history_selection(state: &mut HistoryViewState) {
    let selected_idx = state
        .selected_id
        .as_deref()
        .and_then(|id| state.sessions.iter().position(|session| session.id == id))
        .unwrap_or(
            state
                .selected_idx
                .min(state.sessions.len().saturating_sub(1)),
        );
    state.selected_idx = selected_idx;
    state.selected_id = Some(state.sessions[selected_idx].id.clone());
}
