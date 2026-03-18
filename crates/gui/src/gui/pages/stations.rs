use super::stations_empty_state::{
    render_stations_proxy_unavailable_state, render_stations_runtime_empty_state,
};
use super::stations_panels::render_stations_panels;
pub(super) use super::stations_profile_management::render_profile_management_entrypoint;
use super::stations_retry_panel::render_retry_panel;
use super::stations_runtime_summary::render_stations_runtime_summary;
use super::*;

pub(super) fn render(ui: &mut egui::Ui, ctx: &mut PageCtx<'_>) {
    ui.heading(pick(ctx.lang, "站点", "Stations"));
    ui.label(pick(
        ctx.lang,
        "面向 operator 的运行态站点面板：在这里集中查看站点能力、健康、熔断/冷却状态，并执行 quick switch 与运行时控制。",
        "Operator-focused runtime station panel: inspect station capabilities, health, breaker/cooldown state, and perform quick switch plus runtime control here.",
    ));

    let Some(snapshot) = ctx.proxy.snapshot() else {
        render_stations_proxy_unavailable_state(ui, ctx);
        return;
    };

    if snapshot.stations.is_empty() {
        render_stations_runtime_empty_state(ui, ctx);
        return;
    }

    let runtime_maps = runtime_station_maps(ctx.proxy);
    let active_station = current_runtime_active_station(ctx.proxy);
    let configured_active_station = snapshot.configured_active_station.clone();
    let effective_active_station = snapshot
        .effective_active_station
        .clone()
        .or(active_station.clone());
    let supports_persisted_station_settings = snapshot.supports_persisted_station_settings;
    let mut stations = snapshot.stations.clone();
    stations.sort_by(|a, b| {
        a.level
            .clamp(1, 10)
            .cmp(&b.level.clamp(1, 10))
            .then_with(|| a.name.cmp(&b.name))
    });

    let search_query = ctx.view.stations.search.trim().to_ascii_lowercase();
    let enabled_only = ctx.view.stations.enabled_only;
    let overrides_only = ctx.view.stations.overrides_only;
    let filtered = stations
        .into_iter()
        .filter(|cfg| {
            if enabled_only && !cfg.enabled {
                return false;
            }
            if overrides_only
                && cfg.runtime_enabled_override.is_none()
                && cfg.runtime_level_override.is_none()
                && cfg.runtime_state_override.is_none()
            {
                return false;
            }
            if search_query.is_empty() {
                return true;
            }
            let alias = cfg.alias.as_deref().unwrap_or("");
            let capability = format_runtime_config_capability_label(ctx.lang, &cfg.capabilities);
            let haystack = format!(
                "{} {} {} {}",
                cfg.name.to_ascii_lowercase(),
                alias.to_ascii_lowercase(),
                format_runtime_station_health_status(
                    runtime_maps.station_health.get(cfg.name.as_str()),
                    runtime_maps.health_checks.get(cfg.name.as_str())
                )
                .to_ascii_lowercase(),
                capability.to_ascii_lowercase(),
            );
            haystack.contains(search_query.as_str())
        })
        .collect::<Vec<_>>();

    if ctx
        .view
        .stations
        .selected_name
        .as_ref()
        .is_none_or(|name| !filtered.iter().any(|cfg| cfg.name == *name))
    {
        ctx.view.stations.selected_name = filtered.first().map(|cfg| cfg.name.clone());
    }
    let mut selected_name = ctx.view.stations.selected_name.clone();

    ui.add_space(8.0);
    render_stations_runtime_summary(
        ui,
        ctx,
        &snapshot,
        configured_active_station.as_deref(),
        effective_active_station.as_deref(),
        supports_persisted_station_settings,
    );

    ui.add_space(8.0);
    render_retry_panel(ui, ctx, &snapshot);

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(pick(ctx.lang, "搜索", "Search"));
        ui.add_sized(
            [320.0, 20.0],
            egui::TextEdit::singleline(&mut ctx.view.stations.search).hint_text(pick(
                ctx.lang,
                "按 station / alias / health / capability 过滤…",
                "Filter by station / alias / health / capability...",
            )),
        );
        ui.checkbox(
            &mut ctx.view.stations.enabled_only,
            pick(ctx.lang, "仅启用", "Enabled only"),
        );
        ui.checkbox(
            &mut ctx.view.stations.overrides_only,
            pick(ctx.lang, "仅运行时覆盖", "Overrides only"),
        );
        if ui.button(pick(ctx.lang, "清空", "Clear")).clicked() {
            ctx.view.stations.search.clear();
            ctx.view.stations.enabled_only = false;
            ctx.view.stations.overrides_only = false;
        }
    });

    ui.add_space(6.0);
    render_stations_panels(
        ui,
        ctx,
        &snapshot,
        &runtime_maps,
        &filtered,
        &mut selected_name,
        active_station.as_deref(),
        configured_active_station.as_deref(),
        effective_active_station.as_deref(),
        supports_persisted_station_settings,
    );
    ctx.view.stations.selected_name = selected_name;
}
