use eframe::egui;

use super::components::{history_controls, history_transcript};
use super::history_external::{render_open_in_requests_button, render_open_in_sessions_button};
use super::{PageCtx, history_workdir_from_cwd, open_wt_items};
use crate::sessions::SessionSummarySource;

pub(super) fn render_all_by_date_transcript_panel(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    selected_id: String,
    selected_cwd: String,
    selected_path: std::path::PathBuf,
    selected_first: Option<String>,
    body_height: f32,
    transcript_view_id: &'static str,
) {
    let workdir = history_workdir_from_cwd(selected_cwd.as_str(), ctx.view.history.infer_git_root);
    let mut open_selected_clicked = false;

    ui.group(|ui| {
        open_selected_clicked =
            history_controls::render_resume_group(ui, ctx, "history_wt_batch_mode_all_by_date");

        ui.horizontal(|ui| {
            history_controls::render_selected_session_actions(
                ui,
                ctx,
                selected_id.as_str(),
                workdir.as_str(),
                selected_path.as_path(),
                SessionSummarySource::LocalFile,
            );
            render_open_in_sessions_button(ui, ctx, selected_id.as_str());
            render_open_in_requests_button(ui, ctx, selected_id.as_str());
        });

        ui.label(format!("id: {}", selected_id));
        ui.label(format!("dir: {}", workdir));

        if let Some(first) = selected_first.as_deref() {
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
        let selected_ids = ctx.view.history.batch_selected_ids.clone();
        let infer_git_root = ctx.view.history.infer_git_root;
        let resume_cmd = ctx.view.history.resume_cmd.clone();
        let items = history_controls::build_wt_items_from_day_sessions(
            ctx.view
                .history
                .all_day_sessions
                .iter()
                .filter(|session| selected_ids.contains(&session.id)),
            infer_git_root,
            resume_cmd.as_str(),
        );
        open_wt_items(ctx, items);
    }

    if ctx.view.history.auto_load_transcript {
        let tail = if ctx.view.history.transcript_full {
            None
        } else {
            Some(ctx.view.history.transcript_tail)
        };
        let key = (selected_id.clone(), tail);
        super::history::ensure_transcript_loading(ctx, selected_path.clone(), key);
    }

    ui.add_space(6.0);
    history_transcript::render_transcript_toolbar(ui, ctx, transcript_view_id);
    history_transcript::render_transcript_body(ui, ctx.lang, &mut ctx.view.history, body_height);
}
