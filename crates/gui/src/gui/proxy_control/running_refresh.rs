use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::ProxyConfig;
use crate::dashboard_core::{
    ControlProfileOption, StationOption, build_dashboard_snapshot, build_profile_options_from_mgr,
    build_station_options_from_mgr,
};
use crate::state::{ProxyState, RuntimeConfigState};

use super::{ProxyController, ProxyMode};

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

        let state = r.state.clone();
        let service_name = r.service_name.to_string();
        let cfg = r.cfg.clone();
        let fut = async move {
            let mut snapshot =
                build_dashboard_snapshot(&state, service_name.as_str(), 600, 21).await;
            let mgr = match service_name.as_str() {
                "claude" => &cfg.claude,
                _ => &cfg.codex,
            };
            crate::state::enrich_session_identity_cards_with_runtime(
                &mut snapshot.session_cards,
                mgr,
            );
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
            let stations = effective_stations_from_cfg_state(
                state.as_ref(),
                service_name.as_str(),
                cfg.as_ref(),
            )
            .await;
            Ok::<_, anyhow::Error>((
                snapshot,
                configured_active_station,
                effective_active_station,
                configured_default_profile,
                default_profile,
                profiles,
                stations,
            ))
        };

        match rt.block_on(fut) {
            Ok((
                snap,
                configured_active_station,
                effective_active_station,
                configured_default_profile,
                default_profile,
                profiles,
                stations,
            )) => {
                let global_station_override =
                    snap.effective_global_station_override().map(str::to_owned);
                let station_health = snap.effective_station_health().clone();
                r.last_error = None;
                r.configured_active_station = configured_active_station;
                r.effective_active_station = effective_active_station;
                r.configured_default_profile = configured_default_profile;
                r.default_profile = default_profile;
                r.profiles = profiles;
                r.stations = stations;
                r.active = snap.active;
                r.recent = snap.recent;
                r.session_cards = snap.session_cards;
                r.global_station_override = global_station_override;
                r.session_model_overrides = snap.session_model_overrides;
                r.session_station_overrides = snap.session_station_overrides;
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
            }
            Err(err) => {
                r.last_error = Some(err.to_string());
            }
        }
    }
}
