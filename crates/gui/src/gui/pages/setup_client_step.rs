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
            render_switch_enabled_state(
                ui,
                ctx.lang,
                status.enabled,
                status.has_backup,
                "提示：当前已指向本地代理但未找到备份文件；请手动检查 Claude settings。",
                "Tip: enabled but no backup was found; inspect Claude settings manually.",
            );

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
            render_switch_enabled_state(
                ui,
                ctx.lang,
                status.enabled,
                status.has_switch_state,
                "提示：当前已指向本地代理但未找到 switch state；无法自动判断原 provider。",
                "Tip: enabled but no switch state was found; the original provider cannot be inferred automatically.",
            );

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
                        status.has_switch_state,
                        egui::Button::new(pick(ctx.lang, "关闭代理", "Disable proxy")),
                    )
                    .clicked()
                {
                    match crate::codex_integration::switch_off() {
                        Ok(()) => {
                            *ctx.last_info = Some(
                                pick(
                                    ctx.lang,
                                    "已关闭 Codex 本地代理",
                                    "Disabled Codex local proxy",
                                )
                                .to_string(),
                            );
                        }
                        Err(e) => *ctx.last_error = Some(format!("switch off failed: {e}")),
                    }
                }
            });

            if !status.has_switch_state {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 120, 40),
                    pick(
                        ctx.lang,
                        "提示：未检测到 switch state；如当前已指向本地代理，请手动检查 ~/.codex/config.toml。",
                        "Tip: no switch state detected; if Codex points to the local proxy, inspect ~/.codex/config.toml manually.",
                    ),
                );
            }
        }
        Err(e) => *ctx.last_error = Some(format!("read codex switch status failed: {e}")),
    }
}

fn render_switch_enabled_state(
    ui: &mut egui::Ui,
    lang: Language,
    enabled: bool,
    has_state: bool,
    missing_state_zh: &str,
    missing_state_en: &str,
) {
    if enabled {
        ui.colored_label(
            egui::Color32::from_rgb(60, 160, 90),
            pick(lang, "已启用（本地代理）", "Enabled (local proxy)"),
        );
        if !has_state {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(lang, missing_state_zh, missing_state_en),
            );
        }
    } else {
        ui.colored_label(
            egui::Color32::from_rgb(200, 120, 40),
            pick(lang, "未启用", "Not enabled"),
        );
    }
}
