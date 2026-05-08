use std::sync::Arc;

use crate::config::{ProxyConfig, ServiceConfigManager};
use crate::lb::LoadBalancer;
use crate::logging::log_retry_trace;

use super::ProxyService;
use super::routing_plan::{
    PinnedRoutingSelection, StationRoutingCandidate, StationRoutingMode,
    build_station_routing_plan, resolve_pinned_station_selection,
};

fn station_candidate_json(candidate: &StationRoutingCandidate) -> serde_json::Value {
    serde_json::json!({
        "name": candidate.name,
        "level": candidate.level,
        "enabled": candidate.enabled,
        "runtime_state": candidate.runtime_state,
        "upstreams": candidate.upstream_count,
        "balance_state": station_balance_state(candidate),
        "balance": {
            "snapshots": candidate.balance.snapshots,
            "ok": candidate.balance.ok,
            "exhausted": candidate.balance.exhausted,
            "stale": candidate.balance.stale,
            "error": candidate.balance.error,
            "unknown": candidate.balance.unknown,
        },
    })
}

fn station_candidate_names(candidates: &[StationRoutingCandidate]) -> Vec<String> {
    candidates
        .iter()
        .map(|candidate| candidate.name.clone())
        .collect()
}

fn station_balance_state(candidate: &StationRoutingCandidate) -> &'static str {
    let balance = &candidate.balance;
    if balance.snapshots == 0 {
        "unknown"
    } else if balance.exhausted == balance.snapshots {
        "all_exhausted"
    } else if balance.exhausted > 0 {
        "partial_exhausted"
    } else {
        "available"
    }
}

impl ProxyService {
    pub(super) async fn pinned_config(
        &self,
        mgr: &ServiceConfigManager,
        session_id: Option<&str>,
    ) -> Option<(String, &'static str)> {
        if let Some(sid) = session_id
            && let Some(name) = self.state.get_session_station_override(sid).await
            && !name.trim().is_empty()
        {
            return Some((name, "session"));
        }
        if let Some(name) = self.state.get_global_station_override().await
            && !name.trim().is_empty()
        {
            return Some((name, "global"));
        }
        if let Some(sid) = session_id
            && let Some(binding) = self.state.get_session_binding(sid).await
            && let Some(name) = binding.station_name
            && !name.trim().is_empty()
            && mgr.contains_station(name.as_str())
        {
            return Some((name, "profile_default"));
        }
        None
    }

    pub(super) async fn lbs_for_request(
        &self,
        cfg: &ProxyConfig,
        session_id: Option<&str>,
    ) -> Vec<LoadBalancer> {
        let mgr = self.service_manager(cfg);
        let meta_overrides = self
            .state
            .get_station_meta_overrides(self.service_name)
            .await;
        let state_overrides = self
            .state
            .get_station_runtime_state_overrides(self.service_name)
            .await;
        let upstream_overrides = self
            .state
            .get_upstream_meta_overrides(self.service_name)
            .await;
        let provider_balances = self
            .state
            .get_provider_balance_view(self.service_name)
            .await;
        if let Some((name, source)) = self.pinned_config(mgr, session_id).await {
            let pinned_runtime_state = state_overrides
                .get(name.as_str())
                .copied()
                .unwrap_or_default();
            match resolve_pinned_station_selection(
                mgr,
                name.as_str(),
                &state_overrides,
                &upstream_overrides,
            ) {
                PinnedRoutingSelection::BlockedBreakerOpen => {
                    log_retry_trace(serde_json::json!({
                        "event": "lbs_for_request",
                        "service": self.service_name,
                        "session_id": session_id,
                        "mode": "pinned_blocked_breaker_open",
                        "pinned_source": source,
                        "pinned_name": name,
                        "runtime_state": pinned_runtime_state,
                        "active_station": mgr.active.as_deref(),
                        "station_count": mgr.station_count(),
                    }));
                    return Vec::new();
                }
                PinnedRoutingSelection::Missing => {
                    log_retry_trace(serde_json::json!({
                        "event": "lbs_for_request",
                        "service": self.service_name,
                        "session_id": session_id,
                        "mode": "pinned",
                        "pinned_source": source,
                        "pinned_name": name,
                        "selected_station": null,
                        "active_station": mgr.active.as_deref(),
                        "station_count": mgr.station_count(),
                        "note": "pinned_station_missing_or_all_upstreams_filtered",
                    }));
                    return Vec::new();
                }
                PinnedRoutingSelection::Selected(candidate) => {
                    log_retry_trace(serde_json::json!({
                        "event": "lbs_for_request",
                        "service": self.service_name,
                        "session_id": session_id,
                        "mode": "pinned",
                        "pinned_source": source,
                        "pinned_name": name,
                        "runtime_state": pinned_runtime_state,
                        "selected_station": candidate.name,
                        "selected_level": candidate.level,
                        "selected_upstreams": candidate.upstream_count,
                        "active_station": mgr.active.as_deref(),
                        "station_count": mgr.station_count(),
                    }));
                    return vec![LoadBalancer::new(
                        Arc::new(candidate.service),
                        self.lb_states.clone(),
                    )];
                }
            }
        }

        let plan = build_station_routing_plan(
            mgr,
            mgr.active.as_deref(),
            &meta_overrides,
            &state_overrides,
            &upstream_overrides,
            &provider_balances,
        );
        let selected_station_names = station_candidate_names(&plan.selected_stations);
        let eligible_station_names = station_candidate_names(&plan.eligible_stations);

        match plan.mode {
            StationRoutingMode::SingleLevelMulti => {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "single_level_multi",
                    "active_station": plan.active_station.as_deref(),
                    "selected_stations": selected_station_names,
                    "eligible_stations": eligible_station_names,
                    "eligible_details": plan.eligible_stations.iter().map(station_candidate_json).collect::<Vec<_>>(),
                    "eligible_count": plan.eligible_stations.len(),
                }));
            }
            StationRoutingMode::SingleLevelFallbackActiveStation => {
                let selected = plan.selected_stations.first();
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "single_level_fallback_active_station",
                    "active_station": plan.active_station.as_deref(),
                    "selected_station": selected.map(|candidate| candidate.name.clone()),
                    "selected_level": selected.map(|candidate| candidate.level),
                    "selected_upstreams": selected.map(|candidate| candidate.upstream_count),
                    "eligible_stations": eligible_station_names,
                    "eligible_details": plan.eligible_stations.iter().map(station_candidate_json).collect::<Vec<_>>(),
                    "eligible_count": plan.eligible_stations.len(),
                }));
            }
            StationRoutingMode::SingleLevelEmpty => {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "single_level_empty",
                    "active_station": plan.active_station.as_deref(),
                    "eligible_stations": eligible_station_names,
                    "eligible_details": plan.eligible_stations.iter().map(station_candidate_json).collect::<Vec<_>>(),
                    "eligible_count": plan.eligible_stations.len(),
                }));
            }
            StationRoutingMode::MultiLevel => {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "multi_level",
                    "active_station": plan.active_station.as_deref(),
                    "eligible_stations": plan.selected_stations.iter().map(|candidate| serde_json::json!({
                        "name": candidate.name,
                        "level": candidate.level,
                        "upstreams": candidate.upstream_count,
                    })).collect::<Vec<_>>(),
                    "selected_details": plan.selected_stations.iter().map(station_candidate_json).collect::<Vec<_>>(),
                    "eligible_count": plan.selected_stations.len(),
                }));
            }
            StationRoutingMode::MultiLevelFallbackActiveStation => {
                let selected = plan.selected_stations.first();
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "multi_level_fallback_active_station",
                    "active_station": plan.active_station.as_deref(),
                    "selected_station": selected.map(|candidate| candidate.name.clone()),
                    "selected_level": selected.map(|candidate| candidate.level),
                    "selected_upstreams": selected.map(|candidate| candidate.upstream_count),
                    "selected_details": selected.map(station_candidate_json),
                }));
            }
            StationRoutingMode::MultiLevelEmpty => {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "multi_level_empty",
                    "active_station": plan.active_station.as_deref(),
                }));
            }
        }

        plan.selected_stations
            .into_iter()
            .map(|candidate| LoadBalancer::new(Arc::new(candidate.service), self.lb_states.clone()))
            .collect::<Vec<_>>()
    }
}
