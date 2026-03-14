use super::history_all_by_date::refresh_branch_cache_for_day_items;
use super::*;

pub(super) fn render_all_by_date_controls(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.label(pick(ctx.lang, "最近天数", "Recent days"));
    ui.add(
        egui::DragValue::new(&mut ctx.view.history.all_days_limit)
            .range(1..=10_000)
            .speed(1),
    );
    ui.label(pick(ctx.lang, "当日上限", "Day limit"));
    ui.add(
        egui::DragValue::new(&mut ctx.view.history.all_day_limit)
            .range(1..=10_000)
            .speed(1),
    );
    ui.label(pick(ctx.lang, "工作目录", "Workdir"));
    let mut mode = ctx.gui_cfg.history.workdir_mode.trim().to_ascii_lowercase();
    if mode != "cwd" && mode != "git_root" {
        mode = "cwd".to_string();
    }
    let mut selected_mode = mode.clone();
    egui::ComboBox::from_id_salt("history_workdir_mode_all_by_date")
        .selected_text(match selected_mode.as_str() {
            "git_root" => pick(ctx.lang, "git 根目录", "git root"),
            _ => pick(ctx.lang, "会话 cwd", "session cwd"),
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut selected_mode,
                "cwd".to_string(),
                pick(ctx.lang, "会话 cwd", "session cwd"),
            );
            ui.selectable_value(
                &mut selected_mode,
                "git_root".to_string(),
                pick(ctx.lang, "git 根目录", "git root"),
            );
        });
    if selected_mode != mode {
        ctx.gui_cfg.history.workdir_mode = selected_mode.clone();
        ctx.view.history.infer_git_root = selected_mode == "git_root";
        ctx.view.history.branch_by_workdir.clear();
        let infer_git_root = ctx.view.history.infer_git_root;
        let items = ctx.view.history.all_day_sessions.as_slice();
        refresh_branch_cache_for_day_items(
            &mut ctx.view.history.branch_by_workdir,
            infer_git_root,
            items,
        );
        if let Err(error) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {error}"));
        }
    }
}
