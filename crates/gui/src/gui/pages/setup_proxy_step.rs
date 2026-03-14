use super::proxy_discovery::{
    ProxyDiscoveryActions, ProxyDiscoveryListOptions, render_proxy_discovery_list,
};
use super::*;

pub(super) fn render_setup_proxy_step(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
) -> ProxyDiscoveryActions {
    let mut discovery_actions = ProxyDiscoveryActions::default();

    ui.group(|ui| {
        ui.heading(pick(ctx.lang, "2) 启动本地代理", "2) Start local proxy"));

        let kind = ctx.proxy.kind();
        let status_text = match kind {
            ProxyModeKind::Running => pick(ctx.lang, "运行中", "Running"),
            ProxyModeKind::Attached => pick(ctx.lang, "已附着", "Attached"),
            ProxyModeKind::Starting => pick(ctx.lang, "启动中", "Starting"),
            ProxyModeKind::Stopped => pick(ctx.lang, "未运行", "Stopped"),
        };
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "状态", "Status"),
            status_text
        ));

        ui.horizontal(|ui| {
            ui.label(pick(ctx.lang, "服务", "Service"));
            let mut svc = ctx.proxy.desired_service();
            egui::ComboBox::from_id_salt("setup_service")
                .selected_text(match svc {
                    crate::config::ServiceKind::Codex => "codex",
                    crate::config::ServiceKind::Claude => "claude",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut svc, crate::config::ServiceKind::Codex, "codex");
                    ui.selectable_value(&mut svc, crate::config::ServiceKind::Claude, "claude");
                });
            if svc != ctx.proxy.desired_service() {
                ctx.proxy.set_desired_service(svc);
                ctx.gui_cfg.set_service_kind(svc);
                if let Err(e) = ctx.gui_cfg.save() {
                    *ctx.last_error = Some(format!("save gui config failed: {e}"));
                }
            }

            ui.add_space(12.0);
            ui.label(pick(ctx.lang, "端口", "Port"));
            let mut port = ctx.proxy.desired_port();
            ui.add(egui::DragValue::new(&mut port).range(1..=65535));
            if port != ctx.proxy.desired_port() {
                ctx.proxy.set_desired_port(port);
                ctx.gui_cfg.proxy.default_port = port;
                if let Err(e) = ctx.gui_cfg.save() {
                    *ctx.last_error = Some(format!("save gui config failed: {e}"));
                }
            }
        });

        ui.horizontal(|ui| {
            let can_start = matches!(ctx.proxy.kind(), ProxyModeKind::Stopped);
            if ui
                .add_enabled(
                    can_start,
                    egui::Button::new(pick(ctx.lang, "启动代理", "Start proxy")),
                )
                .clicked()
            {
                let action = PortInUseAction::parse(&ctx.gui_cfg.attach.on_port_in_use);
                ctx.proxy.request_start_or_prompt(
                    ctx.rt,
                    action,
                    ctx.gui_cfg.attach.remember_choice,
                );
            }

            let can_stop = matches!(
                ctx.proxy.kind(),
                ProxyModeKind::Running | ProxyModeKind::Attached
            );
            if ui
                .add_enabled(
                    can_stop,
                    egui::Button::new(pick(ctx.lang, "停止代理", "Stop proxy")),
                )
                .clicked()
            {
                if let Err(e) = ctx.proxy.stop(ctx.rt) {
                    *ctx.last_error = Some(format!("stop failed: {e}"));
                } else {
                    *ctx.last_info =
                        Some(pick(ctx.lang, "已停止代理", "Proxy stopped").to_string());
                }
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.label(pick(
            ctx.lang,
            "已运行代理？（例如：你已在 TUI 中启动）",
            "Already running? (e.g. started from TUI)",
        ));
        ui.horizontal(|ui| {
            if ui
                .button(pick(ctx.lang, "扫描 3210-3220", "Scan 3210-3220"))
                .clicked()
            {
                discovery_actions.scan_local_proxies = true;
            }
            if let Some(t) = ctx.proxy.last_discovery_scan() {
                ui.label(format!(
                    "{}: {}s",
                    pick(ctx.lang, "上次扫描", "Last scan"),
                    t.elapsed().as_secs()
                ));
            }
        });

        render_proxy_discovery_list(
            ui,
            ctx,
            ProxyDiscoveryListOptions {
                scroll_id: "setup_discovered_proxies_scroll",
                grid_id: "setup_discovered_proxies_grid",
                max_height: 160.0,
                empty_text: pick(ctx.lang, "（未发现可用代理）", "(no proxies found)"),
                attach_enabled: true,
                show_port_hover_details: false,
            },
            &mut discovery_actions,
        );
    });

    discovery_actions
}
