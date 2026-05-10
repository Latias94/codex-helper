use std::collections::HashMap;
use std::sync::mpsc::TryRecvError;

use super::history_state::{
    HistoryAllByDateIndexLoad, HistoryAllByDateSessionsLoad, HistoryViewState,
};
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

fn cancel_day_index_load(state: &mut HistoryViewState) {
    if let Some(load) = state.day_index_load.take() {
        load.join.abort();
    }
}

fn cancel_day_sessions_load(state: &mut HistoryViewState) {
    if let Some(load) = state.day_sessions_load.take() {
        load.join.abort();
    }
}

fn poll_day_index_load(ctx: &mut PageCtx<'_>) {
    let Some(load) = ctx.view.history.day_index_load.as_mut() else {
        return;
    };
    match load.rx.try_recv() {
        Ok((seq, res)) => {
            if seq != load.seq {
                ctx.view.history.day_index_load = None;
                return;
            }

            let reset_selection = load.reset_selection;
            ctx.view.history.day_index_load = None;
            match res {
                Ok(dates) => {
                    ctx.view.history.all_dates = dates;
                    ctx.view.history.last_error = None;
                    if reset_selection {
                        ctx.view.history.loaded_day_for = None;
                        ctx.view.history.all_day_sessions.clear();
                        ctx.view.history.selected_id = None;
                        super::history::cancel_transcript_load(&mut ctx.view.history);
                        ctx.view.history.transcript_raw_messages.clear();
                        ctx.view.history.transcript_messages.clear();
                        ctx.view.history.transcript_error = None;
                        ctx.view.history.loaded_for = None;
                    }
                    *ctx.last_info = Some(pick(ctx.lang, "已刷新", "Refreshed").to_string());
                }
                Err(error) => {
                    ctx.view.history.last_error = Some(error.to_string());
                }
            }
        }
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            ctx.view.history.day_index_load = None;
        }
    }
}

fn poll_day_sessions_load(ctx: &mut PageCtx<'_>) {
    let Some(load) = ctx.view.history.day_sessions_load.as_mut() else {
        return;
    };
    match load.rx.try_recv() {
        Ok((seq, res)) => {
            if seq != load.seq {
                ctx.view.history.day_sessions_load = None;
                return;
            }

            let date = load.date.clone();
            ctx.view.history.day_sessions_load = None;
            match res {
                Ok(mut list) => {
                    if ctx.view.history.all_selected_date.as_deref() != Some(date.as_str()) {
                        return;
                    }
                    list.sort_by_key(|session| std::cmp::Reverse(session.mtime_ms));
                    ctx.view.history.all_day_sessions = list;
                    let infer_git_root = ctx.view.history.infer_git_root;
                    let items = ctx.view.history.all_day_sessions.as_slice();
                    refresh_branch_cache_for_day_items(
                        &mut ctx.view.history.branch_by_workdir,
                        infer_git_root,
                        items,
                    );
                    ctx.view.history.loaded_day_for = Some(date);
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
        Err(TryRecvError::Empty) => {}
        Err(TryRecvError::Disconnected) => {
            ctx.view.history.day_sessions_load = None;
        }
    }
}

pub(super) fn poll_all_by_date_loaders(ctx: &mut PageCtx<'_>) {
    poll_day_index_load(ctx);
    poll_day_sessions_load(ctx);
}

fn start_day_index_load(ctx: &mut PageCtx<'_>, reset_selection: bool) {
    cancel_day_index_load(&mut ctx.view.history);
    ctx.view.history.day_index_load_seq = ctx.view.history.day_index_load_seq.saturating_add(1);
    let seq = ctx.view.history.day_index_load_seq;
    let limit = ctx.view.history.all_days_limit;
    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let result = crate::sessions::list_codex_session_day_dirs(limit).await;
        let _ = tx.send((seq, result));
    });

    if reset_selection {
        cancel_day_sessions_load(&mut ctx.view.history);
    }

    ctx.view.history.day_index_load = Some(HistoryAllByDateIndexLoad {
        seq,
        reset_selection,
        rx,
        join,
    });
}

fn start_day_sessions_load(ctx: &mut PageCtx<'_>, date: String, day_dir: std::path::PathBuf) {
    cancel_day_sessions_load(&mut ctx.view.history);
    ctx.view.history.day_sessions_load_seq =
        ctx.view.history.day_sessions_load_seq.saturating_add(1);
    let seq = ctx.view.history.day_sessions_load_seq;
    let limit = ctx.view.history.all_day_limit;
    let (tx, rx) = std::sync::mpsc::channel();
    let join = ctx.rt.spawn(async move {
        let result = crate::sessions::list_codex_sessions_in_day_dir(&day_dir, limit).await;
        let _ = tx.send((seq, result));
    });

    ctx.view.history.day_sessions_load = Some(HistoryAllByDateSessionsLoad {
        seq,
        date,
        rx,
        join,
    });
}

pub(super) fn refresh_day_index(ctx: &mut PageCtx<'_>) {
    ctx.view.history.last_error = None;
    start_day_index_load(ctx, true);
}

pub(super) fn load_more_day_index(ctx: &mut PageCtx<'_>) {
    ctx.view.history.all_days_limit = ctx.view.history.all_days_limit.saturating_add(120);
    ctx.view.history.last_error = None;
    start_day_index_load(ctx, false);
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
        let day_dir = ctx
            .view
            .history
            .all_dates
            .iter()
            .find(|item| item.date == date)
            .map(|item| item.path.clone());
        if let Some(day_dir) = day_dir {
            if ctx
                .view
                .history
                .day_sessions_load
                .as_ref()
                .is_some_and(|load| load.date == date)
            {
                return;
            }
            start_day_sessions_load(ctx, date, day_dir);
        }
    }
}
