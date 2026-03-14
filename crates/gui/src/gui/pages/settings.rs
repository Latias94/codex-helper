use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "设置", "Settings"));

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "语言", "Language"));
        let mut lang = ctx.gui_cfg.language_enum();
        egui::ComboBox::from_id_salt("gui_lang")
            .selected_text(match lang {
                Language::Zh => "中文",
                Language::En => "English",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut lang, Language::Zh, "中文");
                ui.selectable_value(&mut lang, Language::En, "English");
            });
        if lang != ctx.gui_cfg.language_enum() {
            ctx.gui_cfg.set_language_enum(lang);
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            } else {
                *ctx.last_info = Some(pick(lang, "已保存语言设置", "Language saved").to_string());
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "刷新间隔(ms)", "Refresh (ms)"));
        let mut refresh_ms = ctx.gui_cfg.ui.refresh_ms;
        ui.add(egui::DragValue::new(&mut refresh_ms).range(100..=5000));
        if refresh_ms != ctx.gui_cfg.ui.refresh_ms {
            ctx.gui_cfg.ui.refresh_ms = refresh_ms;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.separator();

    ui.horizontal(|ui| {
        let mut enabled = ctx.gui_cfg.proxy.auto_attach_or_start;
        ui.checkbox(
            &mut enabled,
            pick(
                ctx.lang,
                "启动时自动附着/启动代理",
                "Auto attach-or-start on launch",
            ),
        );
        if enabled != ctx.gui_cfg.proxy.auto_attach_or_start {
            ctx.gui_cfg.proxy.auto_attach_or_start = enabled;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        let mut enabled = ctx.gui_cfg.proxy.discovery_scan_fallback;
        ui.checkbox(
            &mut enabled,
            pick(
                ctx.lang,
                "探测失败后扫 3210-3220",
                "Scan 3210-3220 on failure",
            ),
        );
        if enabled != ctx.gui_cfg.proxy.discovery_scan_fallback {
            ctx.gui_cfg.proxy.discovery_scan_fallback = enabled;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "端口占用时", "On port in use"));
        let mut action = PortInUseAction::parse(&ctx.gui_cfg.attach.on_port_in_use);
        egui::ComboBox::from_id_salt("attach_port_in_use_action")
            .selected_text(match action {
                PortInUseAction::Ask => pick(ctx.lang, "每次询问", "Ask"),
                PortInUseAction::Attach => pick(ctx.lang, "默认附着", "Attach"),
                PortInUseAction::StartNewPort => pick(ctx.lang, "自动换端口", "Start new port"),
                PortInUseAction::Exit => pick(ctx.lang, "退出", "Exit"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut action,
                    PortInUseAction::Ask,
                    pick(ctx.lang, "每次询问", "Ask"),
                );
                ui.selectable_value(
                    &mut action,
                    PortInUseAction::Attach,
                    pick(ctx.lang, "默认附着", "Attach"),
                );
                ui.selectable_value(
                    &mut action,
                    PortInUseAction::StartNewPort,
                    pick(ctx.lang, "自动换端口", "Start new port"),
                );
                ui.selectable_value(
                    &mut action,
                    PortInUseAction::Exit,
                    pick(ctx.lang, "退出", "Exit"),
                );
            });
        if action.as_str() != ctx.gui_cfg.attach.on_port_in_use {
            ctx.gui_cfg.attach.on_port_in_use = action.as_str().to_string();
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        let mut remember = ctx.gui_cfg.attach.remember_choice;
        ui.checkbox(
            &mut remember,
            pick(
                ctx.lang,
                "记住选择（不再弹窗）",
                "Remember choice (no prompt)",
            ),
        );
        if remember != ctx.gui_cfg.attach.remember_choice {
            ctx.gui_cfg.attach.remember_choice = remember;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "关闭窗口行为", "Close behavior"));

        let mut behavior = ctx.gui_cfg.window.close_behavior.clone();
        egui::ComboBox::from_id_salt("window_close_behavior")
            .selected_text(behavior.as_str())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut behavior,
                    "minimize_to_tray".to_string(),
                    "minimize_to_tray",
                );
                ui.selectable_value(&mut behavior, "exit".to_string(), "exit");
            });
        if behavior != ctx.gui_cfg.window.close_behavior {
            ctx.gui_cfg.window.close_behavior = behavior;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "启动时行为", "Startup behavior"));

        let mut behavior = ctx.gui_cfg.window.startup_behavior.clone();
        let selected_label = match behavior.as_str() {
            "show" => pick(ctx.lang, "显示窗口", "Show window"),
            "minimized" => pick(ctx.lang, "最小化到任务栏", "Minimize"),
            _ => pick(ctx.lang, "最小化到托盘", "Minimize to tray"),
        };

        egui::ComboBox::from_id_salt("window_startup_behavior")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut behavior,
                    "show".to_string(),
                    pick(ctx.lang, "显示窗口", "Show window"),
                );
                ui.selectable_value(
                    &mut behavior,
                    "minimized".to_string(),
                    pick(ctx.lang, "最小化到任务栏", "Minimize"),
                );
                ui.selectable_value(
                    &mut behavior,
                    "minimize_to_tray".to_string(),
                    pick(ctx.lang, "最小化到托盘", "Minimize to tray"),
                );
            });

        if behavior != ctx.gui_cfg.window.startup_behavior {
            ctx.gui_cfg.window.startup_behavior = behavior;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            } else {
                *ctx.last_info = Some(
                    pick(ctx.lang, "已保存（下次启动生效）", "Saved (next launch)").to_string(),
                );
            }
        }
    });

    ui.horizontal(|ui| {
        let mut enabled = ctx.gui_cfg.tray.enabled;
        ui.checkbox(&mut enabled, pick(ctx.lang, "启用托盘", "Enable tray"));
        if enabled != ctx.gui_cfg.tray.enabled {
            ctx.gui_cfg.tray.enabled = enabled;
            if let Err(e) = ctx.gui_cfg.save() {
                *ctx.last_error = Some(format!("save gui config failed: {e}"));
            } else {
                *ctx.last_info =
                    Some(pick(ctx.lang, "已保存托盘设置", "Tray setting saved").to_string());
            }
        }
        ui.label(pick(
            ctx.lang,
            "(托盘菜单：Show/Hide、Start/Stop、Quit)",
            "(Tray menu: Show/Hide, Start/Stop, Quit)",
        ));
    });

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "开机启动", "Autostart"));

        let reg_enabled = autostart::is_enabled().unwrap_or(false);
        let mut desired = ctx.gui_cfg.autostart.enabled;
        ui.checkbox(&mut desired, pick(ctx.lang, "启用", "Enabled"));

        if desired != ctx.gui_cfg.autostart.enabled {
            match autostart::set_enabled(desired) {
                Ok(()) => {
                    ctx.gui_cfg.autostart.enabled = desired;
                    if let Err(e) = ctx.gui_cfg.save() {
                        *ctx.last_error = Some(format!("save gui config failed: {e}"));
                    } else {
                        *ctx.last_info =
                            Some(pick(ctx.lang, "已更新开机启动", "Autostart updated").to_string());
                    }
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("set autostart failed: {e}"));
                }
            }
        }

        if ctx.gui_cfg.autostart.enabled != reg_enabled {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(ctx.lang, "（未应用到系统）", "(not applied)"),
            );
        }

        ui.label(pick(ctx.lang, "（Windows）", "(Windows)"));
    });
}
