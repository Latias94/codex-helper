use super::overview_connection::{apply_connection_actions, render_connection_panel};
use super::overview_port_modal::render_port_in_use_modal;
use super::overview_runtime_status::render_proxy_mode_summary;
use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "总览", "Overview"));
    ui.separator();

    // Sync defaults from GUI config (so Settings changes take effect without restart).
    // Avoid overriding the UI state while running/attached.
    if matches!(ctx.proxy.kind(), ProxyModeKind::Stopped) {
        ctx.proxy
            .set_defaults(ctx.gui_cfg.proxy.default_port, ctx.gui_cfg.service_kind());
    }

    let actions = render_connection_panel(ui, ctx);
    render_proxy_mode_summary(ui, ctx);
    apply_connection_actions(ctx, actions);
    render_port_in_use_modal(ui, ctx);
}
