use super::*;

pub(super) fn render_proxy_mode_summary(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    match ctx.proxy.kind() {
        ProxyModeKind::Stopped => {
            ui.add_space(8.0);
            ui.label(pick(
                ctx.lang,
                "提示：可在上方“连接与路由”面板启动或附着到代理。",
                "Tip: use the panel above to start or attach to a proxy.",
            ));
        }
        ProxyModeKind::Starting => {
            ui.label(pick(ctx.lang, "正在启动…", "Starting..."));
        }
        ProxyModeKind::Running => {
            super::overview_runtime_status_running::render_running_proxy_summary(ui, ctx);
        }
        ProxyModeKind::Attached => {
            super::overview_runtime_status_attached::render_attached_proxy_summary(ui, ctx);
        }
    }
}
