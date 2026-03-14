use super::*;

pub(super) fn render_running_proxy_summary(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let Some(running) = ctx.proxy.running() else {
        return;
    };

    ui.label(format!(
        "{}: 127.0.0.1:{} ({})",
        pick(ctx.lang, "运行中", "Running"),
        running.port,
        running.service_name
    ));
    if let Some(error) = running.last_error.as_deref() {
        ui.colored_label(egui::Color32::from_rgb(200, 120, 40), error);
    }

    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "活跃请求", "Active requests"),
        running.active.len()
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "最近请求(<=200)", "Recent (<=200)"),
        running.recent.len()
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "全局覆盖(Pinned)", "Global override (pinned)"),
        running
            .global_station_override
            .as_deref()
            .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
    ));

    let active_name = match running.service_name {
        "claude" => running.cfg.claude.active.clone(),
        _ => running.cfg.codex.active.clone(),
    };
    let active_fallback = match running.service_name {
        "claude" => running
            .cfg
            .claude
            .active_station()
            .map(|config| config.name.clone()),
        _ => running
            .cfg
            .codex
            .active_station()
            .map(|config| config.name.clone()),
    };
    let active_display = active_name
        .clone()
        .or(active_fallback.clone())
        .unwrap_or_else(|| "-".to_string());
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "当前站点(active)", "Active station"),
        active_display
    ));

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.colored_label(
            egui::Color32::from_rgb(120, 120, 120),
            pick(
                ctx.lang,
                "默认 active_station / global pin / drain / breaker 已移到 Stations 页集中操作。",
                "Default active_station / global pin / drain / breaker now live in the Stations page.",
            ),
        );
        if ui
            .button(pick(ctx.lang, "打开 Stations 页", "Open Stations page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Stations);
        }
    });

    let warnings =
        crate::config::model_routing_warnings(running.cfg.as_ref(), running.service_name);
    if !warnings.is_empty() {
        ui.add_space(4.0);
        ui.label(pick(
            ctx.lang,
            "模型路由配置警告（建议处理）：",
            "Model routing warnings (recommended to fix):",
        ));
        egui::ScrollArea::vertical()
            .id_salt("overview_model_routing_warnings_scroll")
            .max_height(120.0)
            .show(ui, |ui| {
                for warning in warnings {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), warning);
                }
            });
    }
}
