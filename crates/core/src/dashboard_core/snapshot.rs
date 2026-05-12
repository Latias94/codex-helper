use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::dashboard_core::WindowStats;
use crate::dashboard_core::types::{ControlProfileOption, StationOption};
use crate::dashboard_core::window_stats::compute_window_stats;
use crate::state::{
    ActiveRequest, FinishedRequest, HealthCheckStatus, LbConfigView, ProviderBalanceSnapshot,
    ProxyState, SessionIdentityCard, SessionIdentityCardBuildInputs, SessionStats, StationHealth,
    UsageRollupView, build_session_identity_cards_from_parts,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSnapshot {
    pub refreshed_at_ms: u64,
    pub active: Vec<ActiveRequest>,
    pub recent: Vec<FinishedRequest>,
    #[serde(default)]
    pub session_cards: Vec<SessionIdentityCard>,
    #[serde(default)]
    pub global_station_override: Option<String>,
    #[serde(default)]
    pub session_model_overrides: HashMap<String, String>,
    #[serde(default)]
    pub session_station_overrides: HashMap<String, String>,
    #[serde(default)]
    pub session_effort_overrides: HashMap<String, String>,
    #[serde(default)]
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    #[serde(default)]
    pub station_health: HashMap<String, StationHealth>,
    #[serde(default)]
    pub provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub lb_view: HashMap<String, LbConfigView>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
}

impl DashboardSnapshot {
    pub fn effective_global_station_override(&self) -> Option<&str> {
        self.global_station_override.as_deref()
    }

    pub fn effective_station_health(&self) -> &HashMap<String, StationHealth> {
        &self.station_health
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiV1Snapshot {
    pub api_version: u32,
    pub service_name: String,
    pub runtime_loaded_at_ms: Option<u64>,
    pub runtime_source_mtime_ms: Option<u64>,
    #[serde(default)]
    pub stations: Vec<StationOption>,
    #[serde(default)]
    pub configured_active_station: Option<String>,
    #[serde(default)]
    pub effective_active_station: Option<String>,
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profiles: Vec<ControlProfileOption>,
    pub snapshot: DashboardSnapshot,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub async fn build_dashboard_snapshot(
    state: &ProxyState,
    service_name: &str,
    recent_limit: usize,
    stats_days: usize,
) -> DashboardSnapshot {
    let now = now_ms();
    let recent_limit = recent_limit.clamp(1, 2_000);
    let recent_for_stats = recent_limit.max(2_000);

    let (
        active,
        mut recent_all,
        global_station_override,
        session_model,
        session_cfg,
        session_effort,
        session_service_tier,
        session_bindings,
        session_route_affinities,
        session_stats,
        usage_rollup,
        station_health,
        provider_balances,
        health_checks,
        lb_view,
    ) = tokio::join!(
        state.list_active_requests(),
        state.list_recent_finished(recent_for_stats),
        state.get_global_station_override(),
        state.list_session_model_overrides(),
        state.list_session_station_overrides(),
        state.list_session_effort_overrides(),
        state.list_session_service_tier_overrides(),
        state.list_session_bindings(),
        state.list_session_route_affinities(),
        state.list_session_stats(),
        state.get_usage_rollup_view(service_name, 12, stats_days),
        state.get_station_health(service_name),
        state.get_provider_balance_view(service_name),
        state.list_health_checks(service_name),
        state.get_lb_view(),
    );

    let stats_5m = compute_window_stats(&recent_all, now, 5 * 60_000, |_| true);
    let stats_1h = compute_window_stats(&recent_all, now, 60 * 60_000, |_| true);
    let mut session_cards =
        build_session_identity_cards_from_parts(SessionIdentityCardBuildInputs {
            active: &active,
            recent: &recent_all,
            overrides: &session_effort,
            station_overrides: &session_cfg,
            model_overrides: &session_model,
            service_tier_overrides: &session_service_tier,
            bindings: &session_bindings,
            route_affinities: &session_route_affinities,
            global_station_override: global_station_override.as_deref(),
            stats: &session_stats,
        });
    state
        .enrich_session_identity_cards_with_cached_host_transcripts(&mut session_cards)
        .await;

    if recent_all.len() > recent_limit {
        recent_all.truncate(recent_limit);
    }

    DashboardSnapshot {
        refreshed_at_ms: now,
        active,
        recent: recent_all,
        session_cards,
        global_station_override,
        session_model_overrides: session_model,
        session_station_overrides: session_cfg,
        session_effort_overrides: session_effort,
        session_service_tier_overrides: session_service_tier,
        session_stats,
        station_health,
        provider_balances,
        health_checks,
        lb_view,
        usage_rollup,
        stats_5m,
        stats_1h,
    }
}
