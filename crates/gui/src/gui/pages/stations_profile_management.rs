use super::*;

pub(super) fn render_profile_management_entrypoint(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.label(pick(ctx.lang, "控制 profiles", "Control profiles"));
    ui.colored_label(
        egui::Color32::from_rgb(120, 120, 120),
        pick(
            ctx.lang,
            "旧版 GUI routing preset 已停用。现在统一使用代理设置文件里的 [codex.profiles.*]；持久化默认 profile 在“代理设置”页管理，单会话覆盖在“会话”页管理。",
            "Legacy GUI routing presets are retired. Use [codex.profiles.*] in the proxy settings file instead; manage persisted default profiles in Proxy Settings and per-session overrides in Sessions.",
        ),
    );
    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "前往代理设置页", "Open Proxy Settings"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::ProxySettings);
        }
        if ui
            .button(pick(ctx.lang, "前往会话页", "Open Sessions page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Sessions);
        }
    });
}
