use super::*;

pub(super) fn render_overview_station_summary(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    let Some(snapshot) = ctx.proxy.snapshot() else {
        return;
    };
    if snapshot.stations.is_empty() {
        return;
    }

    let runtime_maps = runtime_station_maps(ctx.proxy);
    let override_count = snapshot
        .stations
        .iter()
        .filter(|cfg| {
            cfg.runtime_enabled_override.is_some()
                || cfg.runtime_level_override.is_some()
                || cfg.runtime_state_override.is_some()
        })
        .count();
    let health_count = runtime_maps.station_health.len();
    let active_station = current_runtime_active_station(ctx.proxy);

    ui.add_space(8.0);
    ui.separator();
    ui.label(pick(ctx.lang, "站点控制摘要", "Stations summary"));
    ui.horizontal(|ui| {
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "站点数", "Stations"),
            snapshot.stations.len()
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "健康记录", "Health records"),
            health_count
        ));
        ui.label(format!(
            "{}: {}",
            pick(ctx.lang, "运行时覆盖", "Runtime overrides"),
            override_count
        ));
        if ui
            .button(pick(ctx.lang, "打开 Stations 页", "Open Stations page"))
            .clicked()
        {
            ctx.view.requested_page = Some(Page::Stations);
        }
    });
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "全局站点覆盖", "Global pinned station"),
        snapshot
            .global_station_override
            .as_deref()
            .unwrap_or_else(|| pick(ctx.lang, "<自动>", "<auto>"))
    ));
    ui.label(format!(
        "{}: {}",
        pick(ctx.lang, "当前 active_station", "Current active_station"),
        active_station.as_deref().unwrap_or_else(|| pick(
            ctx.lang,
            "<未知/仅本机可见>",
            "<unknown/local-only>"
        ))
    ));
    ui.colored_label(
        egui::Color32::from_rgb(120, 120, 120),
        pick(
            ctx.lang,
            "更细的 quick switch、drain、breaker、健康检查已经移到单独的 Stations 页。",
            "Detailed quick switch, drain, breaker, and health controls now live in the dedicated Stations page.",
        ),
    );
}
