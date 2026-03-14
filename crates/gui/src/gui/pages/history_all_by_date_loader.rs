use std::collections::HashMap;

use super::*;
use crate::sessions::SessionIndexItem;

pub(super) fn refresh_branch_cache_for_day_items(
    branch_by_workdir: &mut HashMap<String, Option<String>>,
    infer_git_root: bool,
    items: &[SessionIndexItem],
) {
    for session in items {
        let Some(cwd) = session.cwd.as_deref() else {
            continue;
        };
        let workdir = history_workdir_from_cwd(cwd, infer_git_root);
        if workdir == "-" || workdir.trim().is_empty() {
            continue;
        }
        if branch_by_workdir.contains_key(workdir.as_str()) {
            continue;
        }
        let branch = super::history::read_git_branch_shallow(workdir.as_str());
        branch_by_workdir.insert(workdir, branch);
    }
}

pub(super) fn refresh_day_index(ctx: &mut PageCtx<'_>) {
    let limit = ctx.view.history.all_days_limit;
    match ctx
        .rt
        .block_on(crate::sessions::list_codex_session_day_dirs(limit))
    {
        Ok(dates) => {
            ctx.view.history.all_dates = dates;
            ctx.view.history.last_error = None;
            ctx.view.history.loaded_day_for = None;
            ctx.view.history.all_day_sessions.clear();
            ctx.view.history.selected_id = None;
            super::history::cancel_transcript_load(&mut ctx.view.history);
            ctx.view.history.transcript_raw_messages.clear();
            ctx.view.history.transcript_messages.clear();
            ctx.view.history.transcript_error = None;
            *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
        }
        Err(error) => {
            ctx.view.history.last_error = Some(error.to_string());
        }
    }
}

pub(super) fn load_more_day_index(ctx: &mut PageCtx<'_>) {
    ctx.view.history.all_days_limit = ctx.view.history.all_days_limit.saturating_add(120);
    let limit = ctx.view.history.all_days_limit;
    match ctx
        .rt
        .block_on(crate::sessions::list_codex_session_day_dirs(limit))
    {
        Ok(dates) => {
            ctx.view.history.all_dates = dates;
            ctx.view.history.last_error = None;
            *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
        }
        Err(error) => {
            ctx.view.history.last_error = Some(error.to_string());
        }
    }
}

pub(super) fn ensure_selected_date_loaded(ctx: &mut PageCtx<'_>) {
    if ctx
        .view
        .history
        .all_selected_date
        .as_deref()
        .is_none_or(|date| {
            !ctx.view
                .history
                .all_dates
                .iter()
                .any(|item| item.date == date)
        })
    {
        ctx.view.history.all_selected_date = Some(ctx.view.history.all_dates[0].date.clone());
        ctx.view.history.loaded_day_for = None;
    }

    if let Some(date) = ctx.view.history.all_selected_date.clone()
        && ctx.view.history.loaded_day_for.as_deref() != Some(date.as_str())
    {
        let limit = ctx.view.history.all_day_limit;
        let day_dir = ctx
            .view
            .history
            .all_dates
            .iter()
            .find(|item| item.date == date)
            .map(|item| item.path.clone());
        if let Some(day_dir) = day_dir {
            match ctx
                .rt
                .block_on(crate::sessions::list_codex_sessions_in_day_dir(
                    &day_dir, limit,
                )) {
                Ok(mut list) => {
                    list.sort_by_key(|session| std::cmp::Reverse(session.mtime_ms));
                    ctx.view.history.all_day_sessions = list;
                    let infer_git_root = ctx.view.history.infer_git_root;
                    let items = ctx.view.history.all_day_sessions.as_slice();
                    refresh_branch_cache_for_day_items(
                        &mut ctx.view.history.branch_by_workdir,
                        infer_git_root,
                        items,
                    );
                    ctx.view.history.loaded_day_for = Some(date.clone());
                    ctx.view.history.selected_id = None;
                    ctx.view.history.transcript_messages.clear();
                    ctx.view.history.transcript_error = None;
                    ctx.view.history.loaded_for = None;
                }
                Err(error) => {
                    ctx.view.history.last_error = Some(error.to_string());
                }
            }
        }
    }
}
