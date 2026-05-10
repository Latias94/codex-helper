use super::proxy_settings_document::start_proxy_settings_save;
use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "原始设置编辑", "Raw settings editor"));

    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "从磁盘重载", "Reload from disk"))
            .clicked()
        {
            match std::fs::read_to_string(ctx.proxy_settings_path) {
                Ok(t) => {
                    *ctx.proxy_settings_text = t.clone();
                    match parse_proxy_settings_document(&t) {
                        Ok(doc) => {
                            ctx.view.proxy_settings.working = Some(doc);
                            ctx.view.proxy_settings.load_error = None;
                            *ctx.last_info = Some(pick(ctx.lang, "已重载", "Reloaded").to_string());
                            *ctx.last_error = None;
                        }
                        Err(e) => {
                            ctx.view.proxy_settings.working = None;
                            ctx.view.proxy_settings.load_error = Some(format!("parse failed: {e}"));
                            *ctx.last_error = Some(format!("parse failed: {e}"));
                        }
                    }
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("read settings failed: {e}"));
                }
            }
        }

        if ui.button(pick(ctx.lang, "校验", "Validate")).clicked() {
            match parse_proxy_settings_document(ctx.proxy_settings_text) {
                Ok(_) => {
                    *ctx.last_info = Some(pick(ctx.lang, "校验通过", "Valid").to_string());
                    *ctx.last_error = None;
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("parse failed: {e}"));
                }
            }
        }

        if ui
            .button(pick(ctx.lang, "保存并应用", "Save & apply"))
            .clicked()
        {
            match parse_proxy_settings_document(ctx.proxy_settings_text) {
                Ok(cfg) => start_proxy_settings_save(
                    ctx,
                    cfg,
                    pick(ctx.lang, "已保存", "Saved").to_string(),
                    true,
                ),
                Err(e) => {
                    *ctx.last_error = Some(format!("parse failed: {e}"));
                }
            }
        }
    });
    if ctx.view.proxy_settings.save_load.is_some() {
        ui.add_space(4.0);
        ui.label(pick(ctx.lang, "正在保存设置...", "Saving settings..."));
    }

    ui.separator();
    let editor = egui::TextEdit::multiline(ctx.proxy_settings_text)
        .font(egui::TextStyle::Monospace)
        .code_editor()
        .desired_rows(28)
        .lock_focus(true);
    ui.add(editor);
}
