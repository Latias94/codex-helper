use eframe::egui;

use super::super::i18n::pick;
use super::components::{history_controls, history_sessions, history_transcript};
use super::history::ResolvedHistoryLayout;
use super::history_external::{
    render_history_selection_context, render_open_in_requests_button,
    render_open_in_sessions_button,
};
use super::history_observed::{
    history_session_supports_local_actions, render_observed_session_placeholder,
};
use super::history_transcript_runtime::{
    ensure_transcript_loading, select_session_and_reset_transcript,
};
use super::{
    PageCtx, build_wt_items_from_session_summaries, history_workdir_from_cwd, open_wt_items,
};
use crate::sessions::SessionSummarySource;

pub(super) fn render_history_content(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    layout: ResolvedHistoryLayout,
) {
    ui.add_space(6.0);
    match layout {
        ResolvedHistoryLayout::Horizontal => render_history_horizontal(ui, ctx),
        ResolvedHistoryLayout::Vertical => render_history_vertical(ui, ctx),
    }
}

fn render_history_horizontal(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.columns(2, |cols| {
        let pending_select = history_sessions::render_sessions_panel_horizontal(&mut cols[0], ctx);
        if let Some((idx, id)) = pending_select {
            select_session_and_reset_transcript(ctx, idx, id);
        }
        ensure_selected_history_transcript_loading(ctx);

        cols[1].heading(pick(ctx.lang, "对话记录", "Transcript"));
        cols[1].add_space(4.0);

        let selected_idx = selected_history_idx(ctx);
        let selected = ctx.view.history.sessions[selected_idx].clone();
        let selected_id = selected.id.clone();
        let selected_source = selected.source;
        let workdir = history_workdir_from_cwd(
            selected.cwd.as_deref().unwrap_or("-"),
            ctx.view.history.infer_git_root,
        );
        let mut open_selected_clicked = false;

        cols[1].group(|ui| {
            open_selected_clicked =
                history_controls::render_resume_group(ui, ctx, "history_wt_batch_mode");

            ui.horizontal(|ui| {
                history_controls::render_selected_session_actions(
                    ui,
                    ctx,
                    selected_id.as_str(),
                    workdir.as_str(),
                    selected.path.as_path(),
                    selected_source,
                );
                render_open_in_sessions_button(ui, ctx, selected_id.as_str());
                render_open_in_requests_button(ui, ctx, selected_id.as_str());
            });

            render_history_selection_context(ui, ctx.lang, &ctx.view.history, &selected);
            ui.label(format!("id: {}", selected_id));
            ui.label(format!("dir: {}", workdir));

            if let Some(first) = selected.first_user_message.as_deref() {
                let mut text = first.to_string();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(3)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false),
                );
            }
        });

        if open_selected_clicked {
            open_selected_history_items(ctx);
        }

        if matches!(selected_source, SessionSummarySource::LocalFile) {
            history_transcript::render_transcript_toolbar(
                &mut cols[1],
                ctx,
                "history_transcript_view",
            );
            history_transcript::render_transcript_body(
                &mut cols[1],
                ctx.lang,
                &mut ctx.view.history,
                480.0,
            );
        } else {
            render_observed_session_placeholder(&mut cols[1], ctx, &selected);
        }
    });
}

fn render_history_vertical(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let max_h = ui.available_height();
    let desired_h = ctx
        .view
        .history
        .sessions_panel_height
        .clamp(160.0, max_h * 0.55);

    let mut pending_select: Option<(usize, String)> = None;

    let resp = egui::TopBottomPanel::top("history_vertical_sessions_panel")
        .resizable(true)
        .default_height(desired_h)
        .min_height(160.0)
        .max_height(max_h * 0.8)
        .show_inside(ui, |ui| {
            pending_select = history_sessions::render_sessions_panel_vertical(ui, ctx);
        });

    ctx.view.history.sessions_panel_height = resp.response.rect.height();
    let pointer_down = ui.ctx().input(|i| i.pointer.any_down());
    if !pointer_down
        && (ctx.gui_cfg.history.sessions_panel_height - ctx.view.history.sessions_panel_height)
            .abs()
            > 2.0
    {
        ctx.gui_cfg.history.sessions_panel_height = ctx.view.history.sessions_panel_height;
        if let Err(error) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {error}"));
        }
    }

    if let Some((idx, id)) = pending_select.take() {
        select_session_and_reset_transcript(ctx, idx, id);
    }
    ensure_selected_history_transcript_loading(ctx);

    ui.add_space(6.0);
    ui.heading(pick(ctx.lang, "对话记录", "Transcript"));
    ui.add_space(4.0);

    let selected_idx = selected_history_idx(ctx);
    let selected = ctx.view.history.sessions[selected_idx].clone();
    let selected_id = selected.id.clone();
    let selected_source = selected.source;
    let workdir = history_workdir_from_cwd(
        selected.cwd.as_deref().unwrap_or("-"),
        ctx.view.history.infer_git_root,
    );
    let mut open_selected_clicked = false;

    ui.horizontal(|ui| {
        history_controls::render_selected_session_actions(
            ui,
            ctx,
            selected_id.as_str(),
            workdir.as_str(),
            selected.path.as_path(),
            selected_source,
        );
        render_open_in_sessions_button(ui, ctx, selected_id.as_str());
        render_open_in_requests_button(ui, ctx, selected_id.as_str());
        open_selected_clicked = history_controls::render_open_selected_in_wt_button(ui, ctx);
    });

    if open_selected_clicked {
        open_selected_history_items(ctx);
    }

    render_history_selection_context(ui, ctx.lang, &ctx.view.history, &selected);
    ui.label(format!("id: {}", selected_id));
    ui.label(format!("dir: {}", workdir));

    if matches!(selected_source, SessionSummarySource::LocalFile) {
        history_transcript::render_transcript_toolbar(ui, ctx, "history_transcript_view");
        let transcript_max_h = ui.available_height();
        history_transcript::render_transcript_body(
            ui,
            ctx.lang,
            &mut ctx.view.history,
            transcript_max_h,
        );
    } else {
        render_observed_session_placeholder(ui, ctx, &selected);
    }
}

fn selected_history_idx(ctx: &PageCtx<'_>) -> usize {
    ctx.view
        .history
        .selected_idx
        .min(ctx.view.history.sessions.len().saturating_sub(1))
}

fn ensure_selected_history_transcript_loading(ctx: &mut PageCtx<'_>) {
    let selected_idx = selected_history_idx(ctx);
    if ctx.view.history.auto_load_transcript
        && ctx
            .view
            .history
            .sessions
            .get(selected_idx)
            .is_some_and(history_session_supports_local_actions)
        && let Some(id) = ctx.view.history.selected_id.clone()
    {
        let path = ctx.view.history.sessions[selected_idx].path.clone();
        let tail = if ctx.view.history.transcript_full {
            None
        } else {
            Some(ctx.view.history.transcript_tail)
        };
        ensure_transcript_loading(ctx, path, (id, tail));
    }
}

fn open_selected_history_items(ctx: &mut PageCtx<'_>) {
    let selected_ids = ctx.view.history.batch_selected_ids.clone();
    let infer_git_root = ctx.view.history.infer_git_root;
    let resume_cmd = ctx.view.history.resume_cmd.clone();
    let items = build_wt_items_from_session_summaries(
        ctx.view
            .history
            .sessions
            .iter()
            .filter(|summary| selected_ids.contains(&summary.id)),
        infer_git_root,
        resume_cmd.as_str(),
    );
    open_wt_items(ctx, items);
}
