use super::components::console_layout::{ConsoleTone, console_section};
use super::sessions_quick_actions_general::{
    render_copy_session_id_action, render_open_cwd_action,
};
use super::sessions_quick_actions_navigation::{
    render_open_history_action, render_open_requests_action, render_open_transcript_action,
};
use super::*;

pub(super) fn render_session_quick_actions(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    row: &SessionRow,
    host_local_session_features: bool,
) {
    console_section(
        ui,
        pick(ctx.lang, "快捷操作", "Quick actions"),
        ConsoleTone::Neutral,
        |ui| {
            ui.horizontal_wrapped(|ui| {
                render_copy_session_id_action(ui, ctx, row);
                render_open_cwd_action(ui, ctx, row, host_local_session_features);
                render_open_requests_action(ui, ctx, row);
                render_open_history_action(ui, ctx, row, host_local_session_features);
                render_open_transcript_action(ui, ctx, row, host_local_session_features);
            });
        },
    );
}
