use super::proxy_discovery::ProxyDiscoveryActions;
use super::*;

fn render_snapshot_summary(ui: &mut egui::Ui, ctx: &PageCtx<'_>, kind: ProxyModeKind) {
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

    if let Some(snapshot) = ctx.proxy.snapshot() {
        if let Some(base) = snapshot.base_url.as_deref() {
            ui.label(format!("{}: {base}", pick(ctx.lang, "地址", "Base URL")));
        }
        if let Some(service) = snapshot.service_name.as_deref() {
            ui.label(format!("{}: {service}", pick(ctx.lang, "服务", "Service")));
        }
        if let Some(port) = snapshot.port {
            ui.label(format!("{}: {port}", pick(ctx.lang, "端口", "Port")));
        }
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "API", "API"),
            if snapshot.supports_v1 { "v1" } else { "legacy" }
        ));
    }
}

fn render_service_and_port_controls(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>, can_edit: bool) {
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "服务", "Service"));
        ui.add_enabled_ui(can_edit, |ui| {
            let mut svc = ctx.proxy.desired_service();
            egui::ComboBox::from_id_salt("proxy_service")
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
                if let Err(error) = ctx.gui_cfg.save() {
                    *ctx.last_error = Some(format!("save gui config failed: {error}"));
                }
            }
        });

        ui.add_space(12.0);
        ui.label(pick(ctx.lang, "端口", "Port"));
        ui.add_enabled_ui(can_edit, |ui| {
            let mut port = ctx.proxy.desired_port();
            ui.add(egui::DragValue::new(&mut port).range(1..=65535));
            if port != ctx.proxy.desired_port() {
                ctx.proxy.set_desired_port(port);
                ctx.gui_cfg.proxy.default_port = port;
                if let Err(error) = ctx.gui_cfg.save() {
                    *ctx.last_error = Some(format!("save gui config failed: {error}"));
                }
            }
        });

        if !can_edit {
            ui.colored_label(
                egui::Color32::from_rgb(120, 120, 120),
                pick(ctx.lang, "（停止后可修改）", "(stop to edit)"),
            );
        }
    });
}

fn render_runtime_action_buttons(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    kind: ProxyModeKind,
    actions: &mut ProxyDiscoveryActions,
) {
    ui.horizontal(|ui| {
        match kind {
            ProxyModeKind::Stopped => {
                if ui
                    .button(pick(ctx.lang, "启动代理", "Start proxy"))
                    .clicked()
                {
                    let action = PortInUseAction::parse(&ctx.gui_cfg.attach.on_port_in_use);
                    ctx.proxy.request_start_or_prompt(
                        ctx.rt,
                        action,
                        ctx.gui_cfg.attach.remember_choice,
                    );

                    if let Some(error) = ctx.proxy.last_start_error() {
                        *ctx.last_error = Some(error.to_string());
                    }
                }
            }
            ProxyModeKind::Running => {
                if ui
                    .button(pick(ctx.lang, "停止代理", "Stop proxy"))
                    .clicked()
                {
                    if let Err(error) = ctx.proxy.stop(ctx.rt) {
                        *ctx.last_error = Some(format!("stop failed: {error}"));
                    } else {
                        *ctx.last_info = Some(pick(ctx.lang, "已停止", "Stopped").to_string());
                    }
                }
            }
            ProxyModeKind::Attached => {
                if ui.button(pick(ctx.lang, "取消附着", "Detach")).clicked() {
                    ctx.proxy.clear_port_in_use_modal();
                    ctx.proxy.detach();
                    *ctx.last_info = Some(pick(ctx.lang, "已取消附着", "Detached").to_string());
                }
            }
            ProxyModeKind::Starting => {
                ui.spinner();
            }
        }

        if matches!(kind, ProxyModeKind::Running | ProxyModeKind::Attached)
            && ui.button(pick(ctx.lang, "刷新", "Refresh")).clicked()
        {
            ctx.proxy
                .refresh_current_if_due(ctx.rt, std::time::Duration::from_secs(0));
        }

        if matches!(kind, ProxyModeKind::Running | ProxyModeKind::Attached)
            && ui
                .button(pick(ctx.lang, "重载代理运行态", "Reload proxy runtime"))
                .clicked()
        {
            if let Err(error) = ctx.proxy.reload_runtime_config(ctx.rt) {
                *ctx.last_error = Some(format!("reload runtime failed: {error}"));
            } else {
                *ctx.last_info = Some(pick(ctx.lang, "已重载", "Reloaded").to_string());
            }
        }

        if ui
            .button(pick(ctx.lang, "扫描 3210-3220", "Scan 3210-3220"))
            .clicked()
        {
            actions.scan_local_proxies = true;
        }
        if let Some(last_scan) = ctx.proxy.last_discovery_scan() {
            ui.label(format!(
                "{}: {}s",
                pick(ctx.lang, "上次扫描", "Last scan"),
                last_scan.elapsed().as_secs()
            ));
        }
    });
}

pub(super) fn render_connection_status_section(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    actions: &mut ProxyDiscoveryActions,
) {
    let kind = ctx.proxy.kind();
    render_snapshot_summary(ui, ctx, kind);

    let can_edit = matches!(kind, ProxyModeKind::Stopped);
    render_service_and_port_controls(ui, ctx, can_edit);

    ui.add_space(6.0);
    render_runtime_action_buttons(ui, ctx, kind, actions);
}
