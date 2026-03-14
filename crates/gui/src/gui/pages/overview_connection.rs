use super::proxy_discovery::{
    ProxyDiscoveryActions, ProxyDiscoveryApplyOptions, apply_proxy_discovery_actions,
};
use super::*;

pub(super) fn render_connection_panel(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
) -> ProxyDiscoveryActions {
    let mut actions = ProxyDiscoveryActions::default();

    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "连接与路由", "Connection & routing"));
        super::overview_connection_status::render_connection_status_section(ui, ctx, &mut actions);
        super::overview_connection_attach::render_attach_proxy_section(ui, ctx, &mut actions);
        stations::render_profile_management_entrypoint(ui, ctx);
        super::overview_station_summary::render_overview_station_summary(ui, ctx);
    });

    actions
}

pub(super) fn apply_connection_actions(ctx: &mut PageCtx<'_>, actions: ProxyDiscoveryActions) {
    apply_proxy_discovery_actions(
        ctx,
        actions,
        ProxyDiscoveryApplyOptions {
            scan_done_none: pick(ctx.lang, "扫描完成：未发现代理", "Scan done: none found"),
            scan_done_found: pick(ctx.lang, "扫描完成：已列出可用代理", "Scan done"),
            attach_success: pick(ctx.lang, "正在附着…", "Attaching..."),
            sync_desired_port: false,
            sync_default_port: false,
        },
    );
}
