use eframe::egui;

use super::components::history_sessions;
use super::history::ResolvedHistoryLayout;
use super::history_all_by_date_loader::{
    ensure_selected_date_loaded, load_more_day_index, poll_all_by_date_loaders, refresh_day_index,
};
use super::history_all_by_date_transcript::render_all_by_date_transcript_panel;
use super::*;

fn selected_day_session_details(
    ctx: &PageCtx<'_>,
) -> Option<(String, String, std::path::PathBuf, Option<String>)> {
    let selected_idx = ctx.view.history.selected_id.as_deref().and_then(|id| {
        ctx.view
            .history
            .all_day_sessions
            .iter()
            .position(|session| session.id == id)
    });
    let selected = selected_idx.and_then(|idx| ctx.view.history.all_day_sessions.get(idx))?;
    Some((
        selected.id.clone(),
        selected.cwd.clone().unwrap_or_else(|| "-".to_string()),
        selected.path.clone(),
        selected.first_user_message.clone(),
    ))
}

fn render_history_all_by_date_vertical(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>, q: &str) {
    let max_h = ui.available_height();
    let desired_h = ctx
        .view
        .history
        .sessions_panel_height
        .clamp(200.0, max_h * 0.55);

    let q = q.trim();
    let mut pending_select: Option<(usize, String)> = None;

    let resp = egui::TopBottomPanel::top("history_all_vertical_nav_panel")
        .resizable(true)
        .default_height(desired_h)
        .min_height(200.0)
        .max_height(max_h * 0.8)
        .show_inside(ui, |ui| {
            ui.columns(2, |cols| {
                let max_h = cols[0].available_height().max(160.0);
                history_sessions::render_all_by_date_dates_panel(
                    &mut cols[0],
                    ctx,
                    max_h,
                    "history_all_by_date_dates_scroll",
                );

                let max_h = cols[1].available_height().max(160.0);
                pending_select = history_sessions::render_all_by_date_sessions_panel(
                    &mut cols[1],
                    ctx,
                    q,
                    max_h,
                    "history_all_by_date_sessions_scroll",
                );
            });
        });

    ctx.view.history.sessions_panel_height = resp.response.rect.height();
    let pointer_down = ui.ctx().input(|input| input.pointer.any_down());
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

    if let Some((idx, id)) = pending_select {
        super::history::select_session_and_reset_transcript(ctx, idx, id);
    }

    ui.add_space(6.0);
    ui.heading(pick(ctx.lang, "对话记录", "Transcript"));
    ui.add_space(4.0);

    let Some((selected_id, selected_cwd, selected_path, selected_first)) =
        selected_day_session_details(ctx)
    else {
        ui.label(pick(
            ctx.lang,
            "选择一个会话以预览对话。",
            "Select a session to preview.",
        ));
        return;
    };

    render_all_by_date_transcript_panel(
        ui,
        ctx,
        selected_id,
        selected_cwd,
        selected_path,
        selected_first,
        ui.available_height(),
        "history_transcript_view_all",
    );
}

pub(super) fn render_history_all_by_date(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    layout: ResolvedHistoryLayout,
) {
    ui.add_space(6.0);
    poll_all_by_date_loaders(ctx);

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add(
            egui::TextEdit::singleline(&mut ctx.view.history.query)
                .desired_width(260.0)
                .hint_text(pick(
                    ctx.lang,
                    "关键词（匹配 cwd 或首条用户消息）",
                    "keyword (cwd or first user message)",
                )),
        );

        if ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked() {
            refresh_day_index(ctx);
        }

        if ui
            .button(pick(ctx.lang, "加载更多天", "Load more days"))
            .clicked()
        {
            load_more_day_index(ctx);
        }

        ui.checkbox(
            &mut ctx.view.history.auto_load_transcript,
            pick(ctx.lang, "自动加载对话", "Auto load transcript"),
        );
    });

    if let Some(error) = ctx.view.history.last_error.as_deref() {
        ui.add_space(4.0);
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), error);
    }
    if ctx.view.history.day_index_load.is_some() {
        ui.add_space(4.0);
        ui.label(pick(
            ctx.lang,
            "正在加载日期索引...",
            "Loading date index...",
        ));
    } else if ctx.view.history.day_sessions_load.is_some() {
        ui.add_space(4.0);
        ui.label(pick(
            ctx.lang,
            "正在加载所选日期的会话...",
            "Loading sessions for the selected date...",
        ));
    }

    if ctx.view.history.all_dates.is_empty() {
        ui.add_space(8.0);
        if ctx.view.history.day_index_load.is_none() {
            ui.label(pick(
                ctx.lang,
                "暂无日期索引。点击“刷新”加载。",
                "No date index loaded. Click Refresh.",
            ));
        }
        return;
    }

    ensure_selected_date_loaded(ctx);

    let q = ctx.view.history.query.trim().to_lowercase();

    ui.add_space(6.0);
    if layout == ResolvedHistoryLayout::Vertical {
        render_history_all_by_date_vertical(ui, ctx, q.as_str());
        return;
    }

    ui.columns(3, |cols| {
        history_sessions::render_all_by_date_dates_panel(
            &mut cols[0],
            ctx,
            520.0,
            "history_all_by_date_dates_scroll",
        );
        let pending_select = history_sessions::render_all_by_date_sessions_panel(
            &mut cols[1],
            ctx,
            q.as_str(),
            520.0,
            "history_all_by_date_sessions_scroll",
        );
        if let Some((idx, id)) = pending_select {
            super::history::select_session_and_reset_transcript(ctx, idx, id);
        }

        cols[2].heading(pick(ctx.lang, "对话记录", "Transcript"));
        cols[2].add_space(4.0);

        let Some((selected_id, selected_cwd, selected_path, selected_first)) =
            selected_day_session_details(ctx)
        else {
            cols[2].label(pick(
                ctx.lang,
                "选择一个会话以预览对话。",
                "Select a session to preview.",
            ));
            return;
        };

        render_all_by_date_transcript_panel(
            &mut cols[2],
            ctx,
            selected_id,
            selected_cwd,
            selected_path,
            selected_first,
            360.0,
            "history_transcript_view_all",
        );
    });
}
