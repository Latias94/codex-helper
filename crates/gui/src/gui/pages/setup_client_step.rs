use super::*;

pub(super) fn render_setup_client_step(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.group(|ui| {
        ui.heading(pick(
            ctx.lang,
            "3) 让客户端走本地代理",
            "3) Point client to local proxy",
        ));

        let svc = ctx.proxy.desired_service();
        let port = ctx
            .proxy
            .snapshot()
            .and_then(|s| s.port)
            .unwrap_or(ctx.proxy.desired_port());

        match svc {
            crate::config::ServiceKind::Claude => render_claude_switch_step(ui, ctx, port),
            crate::config::ServiceKind::Codex => render_codex_switch_step(ui, ctx, port),
        }
    });
}

fn render_claude_switch_step(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>, port: u16) {
    match crate::codex_integration::claude_switch_status() {
        Ok(status) => {
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "Claude settings", "Claude settings"),
                status.settings_path.display()
            ));
            ui.label(format!(
                "{}: {}",
                pick(
                    ctx.lang,
                    "当前 ANTHROPIC_BASE_URL",
                    "Current ANTHROPIC_BASE_URL"
                ),
                status.base_url.as_deref().unwrap_or("-")
            ));
            render_switch_enabled_state(ui, ctx.lang, status.enabled, status.has_backup);

            ui.horizontal(|ui| {
                let enable_label = match ctx.lang {
                    Language::Zh => format!("启用（端口 {port}）"),
                    Language::En => format!("Enable (port {port})"),
                };
                if ui
                    .add_enabled(!status.enabled, egui::Button::new(enable_label))
                    .clicked()
                {
                    match crate::codex_integration::claude_switch_on(port) {
                        Ok(()) => {
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已更新 Claude settings 指向本地代理",
                                    "Updated Claude settings to local proxy",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => *ctx.last_error = Some(format!("switch on failed: {e}")),
                    }
                }

                if ui
                    .add_enabled(
                        status.has_backup,
                        egui::Button::new(pick(
                            ctx.lang,
                            "恢复（从备份）",
                            "Restore (from backup)",
                        )),
                    )
                    .clicked()
                {
                    match crate::codex_integration::claude_switch_off() {
                        Ok(()) => {
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已从备份恢复 Claude settings",
                                    "Restored Claude settings from backup",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => *ctx.last_error = Some(format!("switch off failed: {e}")),
                    }
                }
            });
        }
        Err(e) => *ctx.last_error = Some(format!("read claude switch status failed: {e}")),
    }
}

fn render_codex_switch_step(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>, port: u16) {
    match crate::codex_integration::codex_switch_status() {
        Ok(status) => {
            ui.label(pick(
                ctx.lang,
                "Codex 将通过 ~/.codex/config.toml 的 model_provider 指向本地代理。",
                "Codex will route through ~/.codex/config.toml (model_provider).",
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "当前 model_provider", "Current model_provider"),
                status.model_provider.as_deref().unwrap_or("-")
            ));
            ui.label(format!(
                "{}: {}",
                pick(ctx.lang, "当前 base_url", "Current base_url"),
                status.base_url.as_deref().unwrap_or("-")
            ));
            render_switch_enabled_state(ui, ctx.lang, status.enabled, status.has_backup);

            ui.horizontal(|ui| {
                let enable_label = match ctx.lang {
                    Language::Zh => format!("启用（端口 {port}）"),
                    Language::En => format!("Enable (port {port})"),
                };
                if ui
                    .add_enabled(!status.enabled, egui::Button::new(enable_label))
                    .clicked()
                {
                    match crate::codex_integration::switch_on(port) {
                        Ok(()) => {
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已更新 ~/.codex/config.toml 指向本地代理",
                                    "Updated ~/.codex/config.toml to local proxy",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => *ctx.last_error = Some(format!("switch on failed: {e}")),
                    }
                }

                if ui
                    .add_enabled(
                        status.has_backup,
                        egui::Button::new(pick(
                            ctx.lang,
                            "恢复（从备份）",
                            "Restore (from backup)",
                        )),
                    )
                    .clicked()
                {
                    match crate::codex_integration::switch_off() {
                        Ok(()) => {
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已从备份恢复 ~/.codex/config.toml",
                                    "Restored ~/.codex/config.toml from backup",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => *ctx.last_error = Some(format!("switch off failed: {e}")),
                    }
                }
            });

            if !status.has_backup {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 120, 40),
                    pick(
                        ctx.lang,
                        "提示：未检测到备份文件（首次 switch on 时会自动创建备份）。",
                        "Tip: no backup detected (a backup is created on first switch on).",
                    ),
                );
            }
        }
        Err(e) => *ctx.last_error = Some(format!("read codex switch status failed: {e}")),
    }
}

fn render_switch_enabled_state(ui: &mut egui::Ui, lang: Language, enabled: bool, has_backup: bool) {
    if enabled {
        ui.colored_label(
            egui::Color32::from_rgb(60, 160, 90),
            pick(lang, "已启用（本地代理）", "Enabled (local proxy)"),
        );
        if !has_backup {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    lang,
                    "提示：当前已指向本地代理但未找到备份文件；请勿重复 switch on，否则备份可能覆盖原始配置。",
                    "Tip: enabled but no backup found; avoid repeated switch on (backup may not represent the original config).",
                ),
            );
        }
    } else {
        ui.colored_label(
            egui::Color32::from_rgb(200, 120, 40),
            pick(lang, "未启用", "Not enabled"),
        );
    }
}
