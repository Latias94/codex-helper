use super::*;

pub(super) fn render_copy_session_id_action(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    row: &SessionRow,
) {
    let can_copy = row.session_id.is_some();
    if ui
        .add_enabled(
            can_copy,
            egui::Button::new(pick(ctx.lang, "复制 session_id", "Copy session_id")),
        )
        .clicked()
        && let Some(sid) = row.session_id.as_deref()
    {
        ui.ctx().copy_text(sid.to_string());
        *ctx.last_info = Some(pick(ctx.lang, "已复制", "Copied").to_string());
    }
}

pub(super) fn render_open_cwd_action(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    row: &SessionRow,
    host_local_session_features: bool,
) {
    let can_open_cwd = row.cwd.is_some() && host_local_session_features;
    let mut open_cwd = ui.add_enabled(
        can_open_cwd,
        egui::Button::new(pick(ctx.lang, "打开 cwd", "Open cwd")),
    );
    if row.cwd.is_none() {
        open_cwd = open_cwd.on_disabled_hover_text(pick(
            ctx.lang,
            "当前会话没有可用 cwd。",
            "The current session has no cwd.",
        ));
    } else if !host_local_session_features {
        open_cwd = open_cwd.on_disabled_hover_text(pick(
            ctx.lang,
            "当前附着的是远端代理；这个 cwd 来自 host-local 观测，不一定存在于这台设备上。",
            "A remote proxy is attached; this cwd came from host-local observation and may not exist on this device.",
        ));
    }

    if open_cwd.clicked()
        && let Some(cwd) = row.cwd.as_deref()
    {
        let path = std::path::PathBuf::from(cwd);
        if let Err(error) = open_in_file_manager(&path, false) {
            *ctx.last_error = Some(format!("open cwd failed: {error}"));
        }
    }
}
