use super::*;

pub(super) fn render_port_in_use_modal(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    if !ctx.proxy.show_port_in_use_modal() {
        return;
    }

    let mut open = true;
    egui::Window::new(pick(ctx.lang, "端口已被占用", "Port is in use"))
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ui.ctx(), |ui| {
            let port = ctx.proxy.desired_port();
            ui.label(format!(
                "{}: 127.0.0.1:{}",
                pick(ctx.lang, "监听端口冲突", "Bind conflict"),
                port
            ));
            ui.add_space(8.0);

            let mut remember = ctx.proxy.port_in_use_modal_remember();
            ui.checkbox(
                &mut remember,
                pick(
                    ctx.lang,
                    "记住我的选择（下次不再弹窗）",
                    "Remember my choice",
                ),
            );
            ctx.proxy.set_port_in_use_modal_remember(remember);

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .button(pick(ctx.lang, "附着到现有代理", "Attach"))
                    .clicked()
                {
                    if remember {
                        ctx.gui_cfg.attach.remember_choice = true;
                        ctx.gui_cfg.attach.on_port_in_use =
                            PortInUseAction::Attach.as_str().to_string();
                        let _ = ctx.gui_cfg.save();
                    }
                    ctx.proxy.confirm_port_in_use_attach();
                }
            });

            ui.horizontal(|ui| {
                ui.label(pick(ctx.lang, "换端口启动", "Start on another port"));
                let mut p = ctx
                    .proxy
                    .port_in_use_modal_suggested_port()
                    .unwrap_or(port.saturating_add(1));
                ui.add(egui::DragValue::new(&mut p).range(1..=65535));
                ctx.proxy.set_port_in_use_modal_new_port(p);
                if ui.button(pick(ctx.lang, "启动", "Start")).clicked() {
                    if remember {
                        ctx.gui_cfg.attach.remember_choice = true;
                        ctx.gui_cfg.attach.on_port_in_use =
                            PortInUseAction::StartNewPort.as_str().to_string();
                        let _ = ctx.gui_cfg.save();
                    }
                    ctx.proxy.confirm_port_in_use_new_port(ctx.rt);
                }
            });

            ui.horizontal(|ui| {
                if ui.button(pick(ctx.lang, "退出", "Exit")).clicked() {
                    if remember {
                        ctx.gui_cfg.attach.remember_choice = true;
                        ctx.gui_cfg.attach.on_port_in_use =
                            PortInUseAction::Exit.as_str().to_string();
                        let _ = ctx.gui_cfg.save();
                    }
                    ctx.proxy.confirm_port_in_use_exit();
                }
            });
        });

    if !open {
        ctx.proxy.clear_port_in_use_modal();
    }
}
