use super::*;

pub(super) fn render_stations_proxy_unavailable_state(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.add_space(8.0);
    ui.label(pick(
        ctx.lang,
        "当前没有运行中的本地代理，也没有附着到远端代理。请先在“总览”页启动或附着。",
        "No running or attached proxy is available. Start or attach one from Overview first.",
    ));
    if ui
        .button(pick(ctx.lang, "前往总览", "Go to Overview"))
        .clicked()
    {
        ctx.view.requested_page = Some(Page::Overview);
    }
}

pub(super) fn render_stations_runtime_empty_state(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.add_space(8.0);
    ui.label(pick(
        ctx.lang,
        "当前运行态没有可见站点。你可以先去“代理设置”页或设置文件里定义 station/provider。",
        "No stations are visible in the current runtime. Define stations/providers in Proxy Settings first.",
    ));
    ui.horizontal(|ui| {
        if ui
            .button(pick(ctx.lang, "前往代理设置页", "Open Proxy Settings"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::ProxySettings);
        }
        if ui
            .button(pick(ctx.lang, "返回总览", "Back to Overview"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Overview);
        }
    });
}
