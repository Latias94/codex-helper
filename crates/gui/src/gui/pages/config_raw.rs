use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "原始编辑", "Raw editor"));

    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "从磁盘重载", "Reload from disk"))
            .clicked()
        {
            match std::fs::read_to_string(ctx.proxy_config_path) {
                Ok(t) => {
                    *ctx.proxy_config_text = t.clone();
                    match parse_proxy_config_document(&t) {
                        Ok(doc) => {
                            ctx.view.config.working = Some(doc);
                            ctx.view.config.load_error = None;
                            *ctx.last_info = Some(pick(ctx.lang, "已重载", "Reloaded").to_string());
                            *ctx.last_error = None;
                        }
                        Err(e) => {
                            ctx.view.config.working = None;
                            ctx.view.config.load_error = Some(format!("parse failed: {e}"));
                            *ctx.last_error = Some(format!("parse failed: {e}"));
                        }
                    }
                }
                Err(e) => {
                    *ctx.last_error = Some(format!("read config failed: {e}"));
                }
            }
        }

        if ui.button(pick(ctx.lang, "校验", "Validate")).clicked() {
            match parse_proxy_config_document(ctx.proxy_config_text) {
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
            match parse_proxy_config_document(ctx.proxy_config_text) {
                Ok(cfg) => match save_proxy_config_document(ctx.rt, &cfg) {
                    Ok(()) => {
                        let new_path = crate::config::config_file_path();
                        match std::fs::read_to_string(&new_path) {
                            Ok(t) => {
                                *ctx.proxy_config_text = t.clone();
                                match parse_proxy_config_document(&t) {
                                    Ok(doc) => {
                                        ctx.view.config.working = Some(doc);
                                        ctx.view.config.load_error = None;
                                        *ctx.last_info =
                                            Some(pick(ctx.lang, "已保存", "Saved").to_string());
                                        *ctx.last_error = None;
                                    }
                                    Err(e) => {
                                        ctx.view.config.working = None;
                                        ctx.view.config.load_error =
                                            Some(format!("parse failed: {e}"));
                                        *ctx.last_info =
                                            Some(pick(ctx.lang, "已保存", "Saved").to_string());
                                        *ctx.last_error =
                                            Some(format!("re-read parse failed: {e}"));
                                    }
                                }
                            }
                            Err(e) => {
                                *ctx.last_info = Some(pick(ctx.lang, "已保存", "Saved").to_string());
                                *ctx.last_error = Some(format!("re-read failed: {e}"));
                            }
                        }

                        if matches!(
                            ctx.proxy.kind(),
                            ProxyModeKind::Running | ProxyModeKind::Attached
                        ) && let Err(e) = ctx.proxy.reload_runtime_config(ctx.rt)
                        {
                            *ctx.last_error = Some(format!("reload runtime failed: {e}"));
                        }
                    }
                    Err(e) => {
                        *ctx.last_error = Some(format!("save failed: {e}"));
                    }
                },
                Err(e) => {
                    *ctx.last_error = Some(format!("parse failed: {e}"));
                }
            }
        }
    });

    ui.separator();
    let editor = egui::TextEdit::multiline(ctx.proxy_config_text)
        .font(egui::TextStyle::Monospace)
        .code_editor()
        .desired_rows(28)
        .lock_focus(true);
    ui.add(editor);
}
