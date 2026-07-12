use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::dashboard_core::WindowStats;
use crate::dashboard_core::types::{ControlProfileOption, StationOption};
use crate::dashboard_core::window_stats::compute_window_stats;
use crate::policy_actions::PolicyActionProjection;
use crate::quota_analytics::QuotaAnalyticsView;
use crate::service_status::ServiceStatusSnapshot;
use crate::state::{
    ActiveRequest, FinishedRequest, HealthCheckStatus, LbConfigView, ProviderBalanceSnapshot,
    ProxyState, SessionIdentityCard, SessionIdentityCardBuildInputs, SessionStats, StationHealth,
    UsageDayView, UsageRollupView, build_session_identity_cards_from_parts,
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
    pub global_route_target_override: Option<String>,
    #[serde(default)]
    pub session_model_overrides: HashMap<String, String>,
    #[serde(default)]
    pub session_station_overrides: HashMap<String, String>,
    #[serde(default)]
    pub session_route_target_overrides: HashMap<String, String>,
    #[serde(default)]
    pub session_effort_overrides: HashMap<String, String>,
    #[serde(default)]
    pub session_service_tier_overrides: HashMap<String, String>,
    pub session_stats: HashMap<String, SessionStats>,
    #[serde(default)]
    pub station_health: HashMap<String, StationHealth>,
    #[serde(default)]
    pub provider_balances: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    /// API v1 compatibility projection. Longitudinal quota history lives in `quota_analytics`.
    #[serde(default)]
    pub provider_balance_history: HashMap<String, Vec<ProviderBalanceSnapshot>>,
    pub health_checks: HashMap<String, HealthCheckStatus>,
    pub lb_view: HashMap<String, LbConfigView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_actions: Vec<PolicyActionProjection>,
    #[serde(default)]
    pub usage_day: UsageDayView,
    #[serde(default)]
    pub quota_analytics: QuotaAnalyticsView,
    pub usage_rollup: UsageRollupView,
    pub stats_5m: WindowStats,
    pub stats_1h: WindowStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_status: Option<ServiceStatusSnapshot>,
}

impl DashboardSnapshot {
    pub fn effective_global_station_override(&self) -> Option<&str> {
        self.global_station_override.as_deref()
    }

    pub fn effective_global_route_target_override(&self) -> Option<&str> {
        self.global_route_target_override.as_deref()
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
    service_status_config: Option<&crate::config::ServiceStatusConfig>,
    runtime_config: Option<&crate::config::ProxyConfig>,
) -> DashboardSnapshot {
    let now = now_ms();
    let recent_limit = clamp_recent_limit(recent_limit);
    let recent_for_stats = recent_limit;

    let (
        active,
        mut recent_all,
        global_station_override,
        global_route_target_override,
        session_model,
        session_cfg,
        session_route_target,
        session_effort,
        session_service_tier,
        session_bindings,
        session_route_affinities,
        session_stats,
        mut usage_day,
        quota_analytics,
        usage_rollup,
        station_health,
        provider_balances,
        health_checks,
        lb_view,
        policy_actions,
    ) = tokio::join!(
        state.list_active_requests(),
        state.list_recent_finished(recent_for_stats),
        state.get_global_station_override(),
        state.get_global_route_target_override(),
        state.list_session_model_overrides(),
        state.list_session_station_overrides(),
        state.list_session_route_target_overrides(),
        state.list_session_effort_overrides(),
        state.list_session_service_tier_overrides(),
        state.list_session_bindings(),
        state.list_session_route_affinities(),
        state.list_session_stats(),
        state.get_usage_day_view(service_name, 12, now),
        state.quota_analytics_view(service_name, now),
        state.get_usage_rollup_view(service_name, 12, stats_days),
        state.get_station_health(service_name),
        state.get_provider_balance_view(service_name),
        state.list_health_checks(service_name),
        state.get_lb_view(),
        state.active_policy_action_projections(service_name, now),
    );
    usage_day.retry_gate =
        crate::state::UsageRetryGateSummary::from_policy_actions(&policy_actions);
    let provider_balance_history = provider_balances.clone();

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
    let service_status = match service_status_config {
        Some(config) => Some(
            crate::service_status::refresh_service_status_snapshot(
                config,
                runtime_config,
                service_name,
            )
            .await,
        ),
        None => None,
    };

    DashboardSnapshot {
        refreshed_at_ms: now,
        active,
        recent: recent_all,
        session_cards,
        global_station_override,
        global_route_target_override,
        session_model_overrides: session_model,
        session_station_overrides: session_cfg,
        session_route_target_overrides: session_route_target,
        session_effort_overrides: session_effort,
        session_service_tier_overrides: session_service_tier,
        session_stats,
        station_health,
        provider_balances,
        provider_balance_history,
        health_checks,
        lb_view,
        policy_actions,
        usage_day,
        quota_analytics,
        usage_rollup,
        stats_5m,
        stats_1h,
        service_status,
    }
}

fn clamp_recent_limit(recent_limit: usize) -> usize {
    recent_limit.clamp(1, crate::state::recent_finished_max())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::quota_analytics::{QuotaAnalyticsSupport, QuotaAnalyticsView};

    #[test]
    fn dashboard_recent_limit_clamps_to_retention() {
        let retention = crate::state::recent_finished_max();

        assert_eq!(clamp_recent_limit(retention.saturating_mul(2)), retention);
        assert_eq!(clamp_recent_limit(0), 1);
    }

    #[test]
    fn api_v1_snapshot_preserves_legacy_provider_balance_history_shape() {
        let snapshot = ApiV1Snapshot {
            api_version: 1,
            service_name: "codex".to_string(),
            runtime_loaded_at_ms: None,
            runtime_source_mtime_ms: None,
            stations: Vec::new(),
            configured_active_station: None,
            effective_active_station: None,
            default_profile: None,
            profiles: Vec::new(),
            snapshot: empty_dashboard_snapshot(),
        };

        let encoded = serde_json::to_value(snapshot).expect("serialize v1 snapshot");
        assert_eq!(encoded["api_version"], 1);
        assert!(
            encoded["snapshot"]["provider_balance_history"].is_object(),
            "v1 clients require the legacy provider_balance_history object"
        );
        assert!(
            encoded["snapshot"]["quota_analytics"].is_object(),
            "quota analytics must remain available beside the compatibility field"
        );

        let mut without_compatibility_field = encoded;
        without_compatibility_field["snapshot"]
            .as_object_mut()
            .expect("snapshot object")
            .remove("provider_balance_history");
        let decoded: ApiV1Snapshot = serde_json::from_value(without_compatibility_field)
            .expect("missing compatibility field defaults safely");
        assert!(decoded.snapshot.provider_balance_history.is_empty());
    }

    #[test]
    fn quota_analytics_distinguishes_legacy_unsupported_from_supported_empty() {
        let mut snapshot = empty_dashboard_snapshot();
        snapshot.quota_analytics = QuotaAnalyticsView {
            support: QuotaAnalyticsSupport::Supported,
            generated_at_ms: 42,
            ..QuotaAnalyticsView::default()
        };

        let encoded = serde_json::to_value(&snapshot).expect("serialize snapshot");
        let supported: DashboardSnapshot =
            serde_json::from_value(encoded.clone()).expect("supported snapshot");
        assert_eq!(
            supported.quota_analytics.support,
            QuotaAnalyticsSupport::Supported
        );
        assert!(supported.quota_analytics.pools.is_empty());

        let mut legacy = encoded;
        legacy
            .as_object_mut()
            .expect("snapshot object")
            .remove("quota_analytics");
        let unsupported: DashboardSnapshot =
            serde_json::from_value(legacy).expect("legacy snapshot");
        assert_eq!(
            unsupported.quota_analytics.support,
            QuotaAnalyticsSupport::Unsupported
        );
    }

    #[tokio::test]
    async fn dashboard_builder_populates_supported_quota_analytics() {
        let state = ProxyState::new();

        let snapshot = build_dashboard_snapshot(&state, "codex", 1, 1, None, None).await;

        assert_eq!(
            snapshot.quota_analytics.support,
            QuotaAnalyticsSupport::Supported
        );
        assert_eq!(
            snapshot.quota_analytics.generated_at_ms,
            snapshot.refreshed_at_ms
        );
    }

    fn empty_dashboard_snapshot() -> DashboardSnapshot {
        DashboardSnapshot {
            refreshed_at_ms: 0,
            active: Vec::new(),
            recent: Vec::new(),
            session_cards: Vec::new(),
            global_station_override: None,
            global_route_target_override: None,
            session_model_overrides: HashMap::new(),
            session_station_overrides: HashMap::new(),
            session_route_target_overrides: HashMap::new(),
            session_effort_overrides: HashMap::new(),
            session_service_tier_overrides: HashMap::new(),
            session_stats: HashMap::new(),
            station_health: HashMap::new(),
            provider_balances: HashMap::new(),
            provider_balance_history: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            policy_actions: Vec::new(),
            usage_day: UsageDayView::default(),
            usage_rollup: UsageRollupView::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            service_status: None,
            quota_analytics: QuotaAnalyticsView::default(),
        }
    }
}
