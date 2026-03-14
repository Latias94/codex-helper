use super::history::{HistoryDataSource, HistoryScope};
use super::history_controller_refresh::clear_history_transcript_state;
use super::history_external::ensure_external_focus_visible;
use super::history_transcript_runtime::reset_transcript_view_after_session_switch;
use super::*;

pub(super) fn apply_tail_transcript_search(ctx: &mut PageCtx<'_>) {
    if ctx.view.history.data_source == HistoryDataSource::ObservedFallback {
        *ctx.last_error = Some(
            pick(
                ctx.lang,
                "共享观测模式下没有本地 transcript 文件，不能执行尾部对话搜索。",
                "Observed mode has no local transcript files, so transcript tail search is unavailable.",
            )
            .to_string(),
        );
        return;
    }

    let query = ctx.view.history.query.trim().to_string();
    if query.is_empty() {
        *ctx.last_error = Some(
            pick(
                ctx.lang,
                "请输入关键词后再应用“搜对话(尾部)”",
                "Enter a query before applying transcript search",
            )
            .to_string(),
        );
        return;
    }

    let scope = ctx.view.history.scope;
    let tail = ctx.view.history.search_transcript_tail_n;
    let all = ctx.view.history.sessions_all.clone();
    let needle = query.clone();
    let needle_lc = needle.to_lowercase();

    let meta_match = |summary: &SessionSummary| -> bool {
        match scope {
            HistoryScope::GlobalRecent => {
                summary
                    .cwd
                    .as_deref()
                    .is_some_and(|cwd| cwd.to_lowercase().contains(needle_lc.as_str()))
                    || summary
                        .first_user_message
                        .as_deref()
                        .is_some_and(|msg| msg.to_lowercase().contains(needle_lc.as_str()))
            }
            _ => summary
                .first_user_message
                .as_deref()
                .is_some_and(|msg| msg.to_lowercase().contains(needle_lc.as_str())),
        }
    };

    let future = async move {
        let mut out: Vec<SessionSummary> = Vec::new();
        for summary in all.into_iter() {
            if meta_match(&summary) {
                out.push(summary);
                continue;
            }
            if crate::sessions::codex_session_transcript_tail_contains_query(
                &summary.path,
                &needle,
                tail,
            )
            .await?
            {
                out.push(summary);
            }
        }
        Ok::<Vec<SessionSummary>, anyhow::Error>(out)
    };

    match ctx.rt.block_on(future) {
        Ok(list) => {
            ctx.view.history.sessions = list;
            ensure_external_focus_visible(&mut ctx.view.history);
            ctx.view.history.search_transcript_applied = Some((scope, query, tail));
            ctx.view.history.applied_scope = scope;
            ctx.view.history.applied_query = ctx.view.history.query.clone();
            ctx.view.history.selected_idx = 0;
            ctx.view.history.selected_id = ctx.view.history.sessions.first().map(|s| s.id.clone());
            clear_history_transcript_state(&mut ctx.view.history);
            *ctx.last_info = Some(pick(ctx.lang, "已应用全文过滤", "Applied").to_string());
        }
        Err(error) => {
            ctx.view.history.last_error = Some(error.to_string());
        }
    }
}

pub(super) fn apply_metadata_filter_to_history_state(state: &mut HistoryViewState) {
    let query = state.query.trim().to_lowercase();
    if query.is_empty() {
        state.sessions = state.sessions_all.clone();
    } else {
        let scope = state.scope;
        state.sessions = state
            .sessions_all
            .iter()
            .filter(|summary| match scope {
                HistoryScope::GlobalRecent => {
                    summary
                        .cwd
                        .as_deref()
                        .is_some_and(|cwd| cwd.to_lowercase().contains(query.as_str()))
                        || summary
                            .first_user_message
                            .as_deref()
                            .is_some_and(|msg| msg.to_lowercase().contains(query.as_str()))
                }
                _ => summary
                    .first_user_message
                    .as_deref()
                    .is_some_and(|msg| msg.to_lowercase().contains(query.as_str())),
            })
            .cloned()
            .collect();
    }
    ensure_external_focus_visible(state);
    state.applied_scope = state.scope;
    state.applied_query = state.query.clone();
}

pub(super) fn apply_pending_metadata_filter(ctx: &mut PageCtx<'_>) {
    if (ctx.view.history.applied_scope != ctx.view.history.scope
        || ctx.view.history.applied_query != ctx.view.history.query)
        && !matches!(ctx.view.history.scope, HistoryScope::AllByDate)
    {
        ctx.view.history.search_transcript_applied = None;
        apply_metadata_filter_to_history_state(&mut ctx.view.history);

        let selected_ok = ctx.view.history.selected_id.as_deref().is_some_and(|id| {
            ctx.view
                .history
                .sessions
                .iter()
                .any(|session| session.id == id)
        });
        if !selected_ok {
            ctx.view.history.selected_idx = 0;
            ctx.view.history.selected_id = ctx.view.history.sessions.first().map(|s| s.id.clone());
            reset_transcript_view_after_session_switch(ctx);
        }
    }
}
