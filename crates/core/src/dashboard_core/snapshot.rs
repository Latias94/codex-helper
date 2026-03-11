use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::dashboard_core::WindowStats;
use crate::dashboard_core::types::{ConfigOption, ControlProfileOption};
use crate::dashboard_core::window_stats::compute_window_stats;
use crate::state::{
    ActiveRequest, ConfigHealth, FinishedRequest, HealthCheckStatus, LbConfigView, ProxyState,
    SessionIdentityCard, SessionStats, UsageRollupView, build_session_identity_cards_from_parts,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSnapshot {
    pub refreshed_at_ms: u64,
    pub active: Vec<ActiveRequest>,
    pub recent: Vec<FinishedRequest>,
    #[serde(default)]
    pub session_cards: Vec<SessionIdentityCard>,
    pub global_override: Option<String>,
    #[serde(default)]
    pub session_model_overrides: HashMap<String, String>,
    #[serde(default)]
    pub session_config_overrides: HashMap<String, String>,
    #[serde(default)]
    pub session_effort_overrides: HashMap<String, String>,
    #[serde(default)]
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    pub config_health: HashMap<String, ConfigHealth>,
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub lb_view: HashMap<String, LbConfigView>,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiV1Snapshot {
    pub api_version: u32,
    pub service_name: String,
    pub runtime_loaded_at_ms: Option<u64>,
    pub runtime_source_mtime_ms: Option<u64>,
    pub configs: Vec<ConfigOption>,
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
        global_override,
        session_model,
        session_cfg,
        session_effort,
        session_service_tier,
        session_stats,
        usage_rollup,
        config_health,
        health_checks,
        lb_view,
    ) = tokio::join!(
        state.list_active_requests(),
        state.list_recent_finished(recent_for_stats),
        state.get_global_config_override(),
        state.list_session_model_overrides(),
        state.list_session_config_overrides(),
        state.list_session_effort_overrides(),
        state.list_session_service_tier_overrides(),
        state.list_session_stats(),
        state.get_usage_rollup_view(service_name, 12, stats_days),
        state.get_config_health(service_name),
        state.list_health_checks(service_name),
        state.get_lb_view(),
    );

    let stats_5m = compute_window_stats(&recent_all, now, 5 * 60_000, |_| true);
    let stats_1h = compute_window_stats(&recent_all, now, 60 * 60_000, |_| true);
    let session_cards = build_session_identity_cards_from_parts(
        &active,
        &recent_all,
        &session_effort,
        &session_cfg,
        &session_model,
        &session_service_tier,
        global_override.as_deref(),
        &session_stats,
    );

    if recent_all.len() > recent_limit {
        recent_all.truncate(recent_limit);
    }

    DashboardSnapshot {
        refreshed_at_ms: now,
        active,
        recent: recent_all,
        session_cards,
        global_override,
        session_model_overrides: session_model,
        session_config_overrides: session_cfg,
        session_effort_overrides: session_effort,
        session_service_tier_overrides: session_service_tier,
        session_stats,
        config_health,
        health_checks,
        lb_view,
        usage_rollup,
        stats_5m,
        stats_1h,
    }
}
