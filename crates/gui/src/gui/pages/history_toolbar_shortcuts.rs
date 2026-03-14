use eframe::egui;

use super::history::HistoryScope;
use super::*;

pub(super) fn handle_history_shortcuts(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    if ctx.view.history.scope != HistoryScope::GlobalRecent {
        return;
    }

    let copy_list = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::Y);
    if ui
        .ctx()
        .input_mut(|input| input.consume_shortcut(&copy_list))
    {
        let mut out = String::new();
        for session in ctx.view.history.sessions.iter() {
            let cwd = session.cwd.as_deref().unwrap_or("-");
            if cwd == "-" {
                continue;
            }
            let root = history_workdir_from_cwd(cwd, ctx.view.history.infer_git_root);
            out.push_str(root.trim());
            out.push(' ');
            out.push_str(session.id.as_str());
            out.push('\n');
        }
        ui.ctx().copy_text(out);
        *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
    }

    let copy_selected = egui::KeyboardShortcut::new(egui::Modifiers::CTRL, egui::Key::Enter);
    if ui
        .ctx()
        .input_mut(|input| input.consume_shortcut(&copy_selected))
    {
        if let Some(session) = ctx.view.history.selected_id.as_deref().and_then(|id| {
            ctx.view
                .history
                .sessions
                .iter()
                .find(|session| session.id == id)
        }) {
            let cwd = session.cwd.as_deref().unwrap_or("-");
            if cwd == "-" {
                *ctx.last_error = Some(pick(ctx.lang, "cwd 不可用", "cwd unavailable").to_string());
            } else {
                let workdir = history_workdir_from_cwd(cwd, ctx.view.history.infer_git_root);
                ui.ctx()
                    .copy_text(format!("{} {}", workdir.trim(), session.id));
                *ctx.last_info = Some(pick(ctx.lang, "已复制到剪贴板", "Copied").to_string());
            }
        } else {
            *ctx.last_error =
                Some(pick(ctx.lang, "未选中任何会话", "No session selected").to_string());
        }
    }
}
