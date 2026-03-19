use super::*;

pub(super) fn render_setup_config_step(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let cfg_path = ctx.proxy_settings_path.to_path_buf();
    let cfg_exists = cfg_path.exists() && !ctx.proxy_settings_text.trim().is_empty();

    ui.group(|ui| {
        ui.heading(pick(
            ctx.lang,
            "1) 生成/导入配置",
            "1) Create/import proxy settings",
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "设置文件", "Settings file"),
            cfg_path.display()
        ));

        if cfg_exists {
            ui.colored_label(
                egui::Color32::from_rgb(60, 160, 90),
                pick(ctx.lang, "已就绪", "Ready"),
            );
            if ui
                .button(pick(ctx.lang, "打开设置文件", "Open settings file"))
                .clicked()
                && let Err(e) = open_in_file_manager(&cfg_path, true)
            {
                *ctx.last_error = Some(format!("open settings failed: {e}"));
            }
            if ui
                .button(pick(ctx.lang, "前往代理设置页", "Go to Proxy Settings"))
                .clicked()
            {
                ctx.view.requested_page = Some(Page::ProxySettings);
            }
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(200, 120, 40),
                pick(
                    ctx.lang,
                    "未检测到有效配置（建议先创建）",
                    "Settings file not found (create one first)",
                ),
            );
            ui.checkbox(
                &mut ctx.view.setup.import_codex_on_init,
                pick(
                    ctx.lang,
                    "自动从 ~/.codex/config.toml + auth.json 导入 Codex upstream",
                    "Auto-import Codex upstreams from ~/.codex/config.toml + auth.json",
                ),
            );

            if ui
                .button(pick(
                    ctx.lang,
                    "创建设置文件（config.toml）",
                    "Create settings file (config.toml)",
                ))
                .clicked()
            {
                match ctx.rt.block_on(crate::config::init_config_toml(
                    false,
                    ctx.view.setup.import_codex_on_init,
                )) {
                    Ok(path) => {
                        *ctx.last_info = Some(format!(
                            "{}: {}",
                            pick(ctx.lang, "已写入设置", "Wrote settings"),
                            path.display()
                        ));
                        *ctx.proxy_settings_text =
                            std::fs::read_to_string(ctx.proxy_settings_path).unwrap_or_default();
                    }
                    Err(e) => *ctx.last_error = Some(format!("init settings failed: {e}")),
                }
            }
        }
    });
}
