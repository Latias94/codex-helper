use super::proxy_settings_document::parse_proxy_settings_document;
use super::*;
use crate::gui::proxy_control::ProxyController;

#[derive(Debug, Default)]
pub(super) struct RuntimeStationMaps {
    pub(super) station_health: HashMap<String, StationHealth>,
    pub(super) health_checks: HashMap<String, HealthCheckStatus>,
    pub(super) provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub(super) lb_view: HashMap<String, LbConfigView>,
}

pub(super) fn runtime_station_maps(proxy: &ProxyController) -> RuntimeStationMaps {
    match proxy.kind() {
        ProxyModeKind::Running => proxy
            .running()
            .map(|running| RuntimeStationMaps {
                station_health: running.station_health.clone(),
                health_checks: running.health_checks.clone(),
                provider_balances: running.provider_balances.clone(),
                lb_view: running.lb_view.clone(),
            })
            .unwrap_or_default(),
        ProxyModeKind::Attached => proxy
            .attached()
            .map(|attached| RuntimeStationMaps {
                station_health: attached.station_health.clone(),
                health_checks: attached.health_checks.clone(),
                provider_balances: attached.provider_balances.clone(),
                lb_view: attached.lb_view.clone(),
            })
            .unwrap_or_default(),
        _ => RuntimeStationMaps::default(),
    }
}

pub(super) fn current_runtime_active_station(proxy: &ProxyController) -> Option<String> {
    let snapshot = proxy.snapshot()?;
    snapshot
        .effective_active_station
        .or(snapshot.configured_active_station)
}

pub(super) fn refresh_proxy_settings_editor_from_disk_if_running(ctx: &mut PageCtx<'_>) {
    if !matches!(ctx.proxy.kind(), ProxyModeKind::Running) {
        return;
    }
    let new_path = crate::config::config_file_path();
    if let Ok(text) = std::fs::read_to_string(&new_path) {
        *ctx.proxy_settings_text = text.clone();
        if let Ok(parsed) = parse_proxy_settings_document(&text) {
            ctx.view.proxy_settings.working = Some(parsed);
        }
    }
}
