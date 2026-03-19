use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "代理设置", "Proxy Settings"));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "当前设置文件", "Current settings file"),
        ctx.proxy_settings_path.display()
    ));

    ui.separator();

    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "视图", "View"));
        egui::ComboBox::from_id_salt("config_view_mode")
            .selected_text(match ctx.view.proxy_settings.mode {
                ProxySettingsMode::Form => pick(ctx.lang, "表单", "Form"),
                ProxySettingsMode::Raw => pick(ctx.lang, "原始", "Raw"),
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut ctx.view.proxy_settings.mode,
                    ProxySettingsMode::Form,
                    pick(ctx.lang, "表单", "Form"),
                );
                ui.selectable_value(
                    &mut ctx.view.proxy_settings.mode,
                    ProxySettingsMode::Raw,
                    pick(ctx.lang, "原始", "Raw"),
                );
            });
    });

    ui.add_space(6.0);
    match ctx.view.proxy_settings.mode {
        ProxySettingsMode::Form => proxy_settings_form::render(ui, ctx),
        ProxySettingsMode::Raw => proxy_settings_raw::render(ui, ctx),
    }
}
