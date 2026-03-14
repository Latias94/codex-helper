use super::history::RECENT_WINDOWS;
use super::*;

pub(super) fn render_global_recent_controls(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    refresh_requested: &mut bool,
) {
    let mut window_changed = false;
    ui.label(pick(ctx.lang, "窗口", "Window"));
    for (mins, label) in RECENT_WINDOWS.iter().copied() {
        let selected = ctx.view.history.recent_since_minutes == mins;
        if ui.selectable_label(selected, label).clicked() {
            ctx.view.history.recent_since_minutes = mins;
            window_changed = true;
        }
    }

    ui.label(pick(ctx.lang, "最近(分钟)", "Since (minutes)"));
    let before = ctx.view.history.recent_since_minutes;
    ui.add(
        egui::DragValue::new(&mut ctx.view.history.recent_since_minutes)
            .range(5..=10_080)
            .speed(5),
    )
    .on_hover_text(pick(
        ctx.lang,
        "建议优先用“窗口”快速切换；这里用于精确自定义。",
        "Prefer Window presets; use this for fine-grained customization.",
    ));
    if ctx.view.history.recent_since_minutes != before {
        window_changed = true;
    }
    let approx_h = (ctx.view.history.recent_since_minutes as f32) / 60.0;
    ui.label(format!("≈{approx_h:.1}h"));
    ui.label(pick(ctx.lang, "条数", "Limit"));
    ui.add(
        egui::DragValue::new(&mut ctx.view.history.recent_limit)
            .range(1..=500)
            .speed(1),
    );
    ui.label(pick(ctx.lang, "工作目录", "Workdir"));
    let mut mode = ctx.gui_cfg.history.workdir_mode.trim().to_ascii_lowercase();
    if mode != "cwd" && mode != "git_root" {
        mode = "cwd".to_string();
    }
    let mut selected_mode = mode.clone();
    egui::ComboBox::from_id_salt("history_workdir_mode")
        .selected_text(match selected_mode.as_str() {
            "git_root" => pick(ctx.lang, "git 根目录", "git root"),
            _ => pick(ctx.lang, "会话 cwd", "session cwd"),
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut selected_mode,
                "cwd".to_string(),
                pick(ctx.lang, "会话 cwd", "session cwd"),
            )
            .on_hover_text(pick(
                ctx.lang,
                "使用会话记录中的 cwd 作为恢复/复制的工作目录（推荐）",
                "Use the session's cwd as workdir (recommended).",
            ));
            ui.selectable_value(
                &mut selected_mode,
                "git_root".to_string(),
                pick(ctx.lang, "git 根目录", "git root"),
            )
            .on_hover_text(pick(
                ctx.lang,
                "在 cwd 上向上查找 .git 作为项目根目录（用于复制/打开）",
                "Find .git upward from cwd as project root (for copy/open).",
            ));
        });
    if selected_mode != mode {
        ctx.gui_cfg.history.workdir_mode = selected_mode.clone();
        ctx.view.history.infer_git_root = selected_mode == "git_root";
        let infer_git_root = ctx.view.history.infer_git_root;
        let sessions = ctx.view.history.sessions_all.as_slice();
        super::history::refresh_branch_cache_for_sessions(
            &mut ctx.view.history.branch_by_workdir,
            infer_git_root,
            sessions,
        );
        if let Err(error) = ctx.gui_cfg.save() {
            *ctx.last_error = Some(format!("save gui config failed: {error}"));
        }
        window_changed = true;
    }

    ui.checkbox(
        &mut ctx.view.history.group_by_workdir,
        pick(ctx.lang, "按项目分组", "Group by project"),
    )
    .on_hover_text(pick(
        ctx.lang,
        "按工作目录分组并折叠，适合“第二天继续昨天的一堆会话”的批量恢复。",
        "Group by workdir with collapsible headers; great for batch resume next day.",
    ));

    if window_changed {
        *refresh_requested = true;
        *ctx.last_info = Some(
            pick(
                ctx.lang,
                "窗口已更新（将影响下次刷新）",
                "Window updated (affects next refresh)",
            )
            .to_string(),
        );
    }
}
