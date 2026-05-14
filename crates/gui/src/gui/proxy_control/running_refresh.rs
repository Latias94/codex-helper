use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::ProxyConfig;
use crate::dashboard_core::{
    ControlProfileOption, StationOption, build_dashboard_snapshot, build_profile_options_from_mgr,
    build_station_options_from_mgr,
};
use crate::state::{ProxyState, RuntimeConfigState};

use super::types::RunningRefreshResult;
use super::{ProxyController, ProxyMode, RunningProxy, send_admin_request};

pub(super) async fn effective_default_profile_from_cfg_state(
    state: &ProxyState,
    service_name: &str,
    cfg: &ProxyConfig,
) -> Option<String> {
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    if let Some(name) = state
        .get_runtime_default_profile_override(service_name)
        .await
        && mgr.profiles.contains_key(name.as_str())
    {
        return Some(name);
    }
    mgr.default_profile.clone()
}

pub(super) async fn effective_stations_from_cfg_state(
    state: &ProxyState,
    service_name: &str,
    cfg: &ProxyConfig,
) -> Vec<StationOption> {
    let overrides = state.get_station_meta_overrides(service_name).await;
    let state_overrides = state
        .get_station_runtime_state_overrides(service_name)
        .await;
    list_stations_from_cfg(cfg, service_name, overrides, state_overrides)
}

pub(super) fn list_stations_from_cfg(
    cfg: &ProxyConfig,
    service_name: &str,
    meta_overrides: HashMap<String, (Option<bool>, Option<u8>)>,
    state_overrides: HashMap<String, RuntimeConfigState>,
) -> Vec<StationOption> {
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    build_station_options_from_mgr(mgr, &meta_overrides, &state_overrides)
}

pub(super) fn list_profiles_from_cfg(
    cfg: &ProxyConfig,
    service_name: &str,
    default_name: Option<&str>,
) -> Vec<ControlProfileOption> {
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    build_profile_options_from_mgr(mgr, default_name)
}

impl ProxyController {
    pub fn refresh_running_if_due(
        &mut self,
        rt: &tokio::runtime::Runtime,
        refresh_every: Duration,
    ) {
        let ProxyMode::Running(r) = &mut self.mode else {
            return;
        };
        if let Some(last_refresh) = r.last_refresh
            && last_refresh.elapsed() < refresh_every
        {
            return;
        }
        r.last_refresh = Some(Instant::now());

        let client = self.http_client.clone();
        match rt.block_on(build_running_refresh_result(
            r.state.clone(),
            r.service_name.to_string(),
            r.cfg.clone(),
            client,
            r.admin_port,
        )) {
            Ok(result) => {
                apply_running_refresh_result(r, result);
            }
            Err(err) => {
                r.last_error = Some(err.to_string());
            }
        }
    }
}

pub(super) async fn build_running_refresh_result(
    state: std::sync::Arc<ProxyState>,
    service_name: String,
    cfg: std::sync::Arc<ProxyConfig>,
    client: reqwest::Client,
    admin_port: u16,
) -> anyhow::Result<RunningRefreshResult> {
    let mut snapshot = build_dashboard_snapshot(&state, service_name.as_str(), 600, 21).await;
    let mgr = match service_name.as_str() {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    crate::state::enrich_session_identity_cards_with_runtime(&mut snapshot.session_cards, mgr);
    let configured_active_station = mgr.active.clone();
    let effective_active_station = mgr.active_station().map(|cfg| cfg.name.clone());
    let configured_default_profile = mgr.default_profile.clone();
    let default_profile = effective_default_profile_from_cfg_state(
        state.as_ref(),
        service_name.as_str(),
        cfg.as_ref(),
    )
    .await;
    let profiles = list_profiles_from_cfg(
        cfg.as_ref(),
        service_name.as_str(),
        default_profile.as_deref(),
    );
    let stations =
        effective_stations_from_cfg_state(state.as_ref(), service_name.as_str(), cfg.as_ref())
            .await;
    let routing_explain = fetch_running_routing_explain(client, admin_port).await;

    Ok(RunningRefreshResult {
        snapshot,
        configured_active_station,
        effective_active_station,
        configured_default_profile,
        default_profile,
        profiles,
        stations,
        routing_explain,
    })
}

async fn fetch_running_routing_explain(
    client: reqwest::Client,
    admin_port: u16,
) -> Option<crate::routing_explain::RoutingExplainResponse> {
    send_admin_request(
        client
            .get(format!(
                "http://127.0.0.1:{admin_port}/__codex_helper/api/v1/routing/explain"
            ))
            .timeout(Duration::from_millis(800)),
    )
    .await
    .ok()?
    .json::<crate::routing_explain::RoutingExplainResponse>()
    .await
    .ok()
}

pub(super) fn apply_running_refresh_result(r: &mut RunningProxy, result: RunningRefreshResult) {
    let snap = result.snapshot;
    let global_station_override = snap.effective_global_station_override().map(str::to_owned);
    let global_route_target_override = snap
        .effective_global_route_target_override()
        .map(str::to_owned);
    let station_health = snap.effective_station_health().clone();
    r.last_error = None;
    r.configured_active_station = result.configured_active_station;
    r.effective_active_station = result.effective_active_station;
    r.configured_default_profile = result.configured_default_profile;
    r.default_profile = result.default_profile;
    r.profiles = result.profiles;
    r.stations = result.stations;
    r.active = snap.active;
    r.recent = snap.recent;
    r.session_cards = snap.session_cards;
    r.global_station_override = global_station_override;
    r.global_route_target_override = global_route_target_override;
    r.session_model_overrides = snap.session_model_overrides;
    r.session_station_overrides = snap.session_station_overrides;
    r.session_route_target_overrides = snap.session_route_target_overrides;
    r.session_effort_overrides = snap.session_effort_overrides;
    r.session_service_tier_overrides = snap.session_service_tier_overrides;
    r.session_stats = snap.session_stats;
    r.station_health = station_health;
    r.provider_balances = snap.provider_balances;
    r.health_checks = snap.health_checks;
    r.usage_rollup = snap.usage_rollup;
    r.stats_5m = snap.stats_5m;
    r.stats_1h = snap.stats_1h;
    r.lb_view = snap.lb_view;
    r.routing_explain = result.routing_explain;
}
