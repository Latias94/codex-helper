use super::proxy_discovery::{
    ProxyDiscoveryActions, ProxyDiscoveryListOptions, render_proxy_discovery_list,
};
use super::*;

pub(super) fn render_attach_proxy_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    actions: &mut ProxyDiscoveryActions,
) {
    let kind = ctx.proxy.kind();

    ui.add_space(6.0);
    ui.collapsing(
        pick(
            ctx.lang,
            "附着到已运行的代理",
            "Attach to an existing proxy",
        ),
        |ui| {
            if !matches!(kind, ProxyModeKind::Stopped) {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 120, 120),
                    pick(
                        ctx.lang,
                        "提示：请先停止/取消附着，再切换到其他代理。",
                        "Tip: stop/detach first before switching to another proxy.",
                    ),
                );
            }

            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "端口", "Port"));
                let mut attach_port = ctx
                    .gui_cfg
                    .attach
                    .last_port
                    .unwrap_or(ctx.gui_cfg.proxy.default_port);
                ui.add(egui::DragValue::new(&mut attach_port).range(1..=65535));
                if Some(attach_port) != ctx.gui_cfg.attach.last_port {
                    ctx.gui_cfg.attach.last_port = Some(attach_port);
                    if let Err(error) = ctx.gui_cfg.save() {
                        *ctx.last_error = Some(format!("save gui config failed: {error}"));
                    }
                }

                if ui
                    .add_enabled(
                        matches!(kind, ProxyModeKind::Stopped),
                        egui::Button::new(pick(ctx.lang, "附着", "Attach")),
                    )
                    .clicked()
                {
                    ctx.proxy.request_attach(attach_port);
                    ctx.gui_cfg.attach.last_port = Some(attach_port);
                    if let Err(error) = ctx.gui_cfg.save() {
                        *ctx.last_error = Some(format!("save gui config failed: {error}"));
                    } else {
                        *ctx.last_info = Some(pick(ctx.lang, "正在附着…", "Attaching...").into());
                    }
                }
            });

            render_proxy_discovery_list(
                ui,
                ctx,
                ProxyDiscoveryListOptions {
                    scroll_id: "overview_discovered_proxies_scroll",
                    grid_id: "discovered_proxies_grid",
                    max_height: 180.0,
                    empty_text: pick(ctx.lang, "（未发现可用代理）", "(no proxies found)"),
                    attach_enabled: matches!(kind, ProxyModeKind::Stopped),
                    show_port_hover_details: true,
                },
                actions,
            );
        },
    );
}
