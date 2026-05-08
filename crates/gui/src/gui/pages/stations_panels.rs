use super::stations_detail_controls::{
    render_station_quick_switch_section, render_station_runtime_control_section,
};
use super::stations_detail_health::{
    render_station_balance_section, render_station_breaker_section, render_station_health_section,
};
use super::stations_detail_recent_hits::render_station_recent_hits_section;
use super::stations_detail_summary::render_station_identity_summary;
use super::stations_list_panel::render_station_list_panel;
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_stations_panels(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    runtime_maps: &RuntimeStationMaps,
    filtered: &[StationOption],
    selected_name: &mut Option<String>,
    active_station: Option<&str>,
    configured_active_station: Option<&str>,
    effective_active_station: Option<&str>,
) {
    ui.columns(2, |cols| {
        render_station_list_panel(
            &mut cols[0],
            ctx,
            snapshot,
            runtime_maps,
            filtered,
            selected_name,
            active_station,
        );
        render_station_detail_panel(
            &mut cols[1],
            ctx,
            snapshot,
            runtime_maps,
            filtered,
            selected_name.as_deref(),
            configured_active_station,
            effective_active_station,
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn render_station_detail_panel(
    ui: &mut egui::Ui,
    ctx: &mut PageCtx<'_>,
    snapshot: &GuiRuntimeSnapshot,
    runtime_maps: &RuntimeStationMaps,
    filtered: &[StationOption],
    selected_name: Option<&str>,
    configured_active_station: Option<&str>,
    effective_active_station: Option<&str>,
) {
    ui.heading(pick(ctx.lang, "站点详情", "Station details"));
    ui.add_space(4.0);

    let Some(name) = selected_name else {
        ui.label(pick(ctx.lang, "未选择站点。", "No station selected."));
        return;
    };
    let Some(cfg) = filtered.iter().find(|cfg| cfg.name == name).cloned() else {
        ui.label(pick(
            ctx.lang,
            "当前选中站点不在筛选结果中。",
            "The selected station is not visible under the current filters.",
        ));
        return;
    };

    let health = runtime_maps.station_health.get(cfg.name.as_str());
    let health_status = runtime_maps.health_checks.get(cfg.name.as_str());
    let balances = runtime_maps
        .provider_balances
        .get(cfg.name.as_str())
        .map(Vec::as_slice);
    let lb = runtime_maps.lb_view.get(cfg.name.as_str());
    let referencing_profiles = snapshot
        .profiles
        .iter()
        .filter(|profile| profile.station.as_deref() == Some(cfg.name.as_str()))
        .map(|profile| format_profile_display(profile.name.as_str(), Some(profile)))
        .collect::<Vec<_>>();

    render_station_identity_summary(
        ui,
        ctx,
        &cfg,
        snapshot,
        health,
        health_status,
        balances,
        lb,
        &referencing_profiles,
        configured_active_station,
        effective_active_station,
    );

    render_station_quick_switch_section(ui, ctx, &cfg, snapshot);

    ui.add_space(8.0);
    ui.separator();
    render_station_runtime_control_section(ui, ctx, &cfg, snapshot);

    ui.add_space(8.0);
    ui.separator();
    render_station_health_section(ui, ctx, &cfg, health, health_status);

    ui.add_space(8.0);
    ui.separator();
    render_station_balance_section(ui, ctx, &cfg, balances);

    ui.add_space(8.0);
    ui.separator();
    render_station_breaker_section(ui, ctx, &cfg, lb);

    ui.add_space(8.0);
    ui.separator();
    render_station_recent_hits_section(ui, ctx, &cfg, snapshot);
}
