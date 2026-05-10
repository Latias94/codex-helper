use super::stations_detail_health::format_station_balance_summary;
use super::*;

pub(super) fn render_station_list_panel(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    runtime_maps: &RuntimeStationMaps,
    filtered: &[StationOption],
    selected_name: &mut Option<String>,
    active_station: Option<&str>,
) {
    ui.heading(pick(ctx.lang, "站点列表", "Stations"));
    ui.add_space(4.0);
    if filtered.is_empty() {
        ui.label(pick(
            ctx.lang,
            "筛选后没有匹配站点。",
            "No stations matched the current filters.",
        ));
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("stations_page_list_scroll")
        .max_height(560.0)
        .show(ui, |ui| {
            for cfg in filtered {
                let is_selected = selected_name.as_deref() == Some(cfg.name.as_str());
                let is_active = active_station == Some(cfg.name.as_str());
                let is_pinned =
                    snapshot.global_station_override.as_deref() == Some(cfg.name.as_str());
                let health_label = format_runtime_station_health_status(
                    runtime_maps.station_health.get(cfg.name.as_str()),
                    runtime_maps.health_checks.get(cfg.name.as_str()),
                );
                let breaker_label =
                    format_runtime_lb_summary(runtime_maps.lb_view.get(cfg.name.as_str()));
                let balance_label = format_station_balance_summary(
                    runtime_maps
                        .provider_balances
                        .get(cfg.name.as_str())
                        .map(Vec::as_slice),
                );

                let mut label = format!("L{} {}", cfg.level.clamp(1, 10), cfg.name);
                if let Some(alias) = cfg.alias.as_deref()
                    && !alias.trim().is_empty()
                {
                    label.push_str(&format!(" ({alias})"));
                }
                if is_active {
                    label = format!("★ {label}");
                } else if is_pinned {
                    label = format!("◆ {label}");
                }
                if !cfg.enabled {
                    label.push_str("  [off]");
                }

                let capability_hover =
                    runtime_config_capability_hover_text(ctx.lang, &cfg.capabilities);
                let hover = format!(
                    "health: {health_label}\nbalance/quota: {balance_label}\nbreaker: {breaker_label}\n{}\nsource: {}",
                    capability_hover,
                    format_runtime_station_source(ctx.lang, cfg)
                );
                if ui
                    .selectable_label(is_selected, label)
                    .on_hover_text(hover)
                    .clicked()
                {
                    *selected_name = Some(cfg.name.clone());
                }
                ui.small(format!(
                    "{}  |  {}  |  {}",
                    health_label,
                    balance_label,
                    format_runtime_config_capability_label(ctx.lang, &cfg.capabilities)
                ));
                ui.add_space(4.0);
            }
        });
}
