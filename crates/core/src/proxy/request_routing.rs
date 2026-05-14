use std::sync::Arc;

use crate::config::{
    ProxyConfig, ProxyConfigV4, RoutingExhaustedActionV4, RoutingPolicyV4, ServiceConfigManager,
    ServiceViewV4, effective_v4_routing, resolved_v4_provider_order,
};
use crate::lb::LoadBalancer;
use crate::logging::log_retry_trace;
use crate::routing_ir::{
    RouteCandidate, RoutePlanTemplate, RouteRequestContext,
    compile_v4_route_plan_template_with_request,
};

use super::ProxyService;
use super::routing_plan::{
    PinnedRoutingSelection, StationRoutingCandidate, StationRoutingMode,
    build_station_routing_plan, resolve_pinned_station_selection,
};

pub(super) enum RequestRouteSelection {
    Legacy { lbs: Vec<LoadBalancer> },
    RouteGraph { template: RoutePlanTemplate },
}

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
            "routing_snapshots": candidate.balance.routing_snapshots,
            "routing_exhausted": candidate.balance.routing_exhausted,
            "routing_ignored_exhausted": candidate.balance.routing_ignored_exhausted,
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
    if balance.routing_snapshots == 0 {
        if balance.exhausted > 0 {
            "ignored_exhausted"
        } else {
            "unknown"
        }
    } else if balance.routing_exhausted == balance.routing_snapshots {
        "all_exhausted"
    } else if balance.routing_exhausted > 0 {
        "partial_exhausted"
    } else if balance.exhausted > 0 {
        "ignored_exhausted"
    } else {
        "available"
    }
}

fn route_candidate_trace_json(
    template: &RoutePlanTemplate,
    candidate: &RouteCandidate,
) -> serde_json::Value {
    let identity = template.candidate_identity(candidate);
    serde_json::json!({
        "provider_id": candidate.provider_id.as_str(),
        "endpoint_id": candidate.endpoint_id.as_str(),
        "provider_endpoint_key": identity.provider_endpoint.stable_key(),
        "preference_group": candidate.preference_group,
        "route_path": &candidate.route_path,
    })
}

pub(super) fn service_view_with_route_target_override(
    service_name: &str,
    view: &ServiceViewV4,
    target: &str,
) -> anyhow::Result<ServiceViewV4> {
    let target = target.trim();
    if target.is_empty() {
        anyhow::bail!("route target override is empty");
    }

    let mut view = view.clone();
    let mut routing = effective_v4_routing(&view);
    let entry_name = routing.entry.clone();
    if target == entry_name {
        anyhow::bail!(
            "[{service_name}] route target override cannot target entry route '{target}'"
        );
    }

    let fallback_order = resolved_v4_provider_order(service_name, &view).unwrap_or_default();
    let node = routing.routes.entry(entry_name).or_default();
    node.strategy = RoutingPolicyV4::ManualSticky;
    node.target = Some(target.to_string());
    if node.children.is_empty() {
        node.children = fallback_order;
    }
    node.prefer_tags.clear();
    node.on_exhausted = RoutingExhaustedActionV4::Continue;
    node.when = None;
    node.then = None;
    node.default_route = None;
    view.routing = Some(routing);
    Ok(view)
}

impl ProxyService {
    pub(super) async fn route_target_override_for_session(
        &self,
        session_id: Option<&str>,
    ) -> Option<(String, &'static str)> {
        if let Some(sid) = session_id
            && let Some(target) = self.state.get_session_route_target_override(sid).await
            && !target.trim().is_empty()
        {
            return Some((target, "session_route_target_override"));
        }
        if let Some(target) = self.state.get_global_route_target_override().await
            && !target.trim().is_empty()
        {
            return Some((target, "global_route_target_override"));
        }
        None
    }

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
        v4: Option<&ProxyConfigV4>,
        request: &RouteRequestContext,
        session_id: Option<&str>,
    ) -> RequestRouteSelection {
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
            .get_provider_balance_summary_view(self.service_name)
            .await;
        let route_target_override = if v4.is_some() {
            self.route_target_override_for_session(session_id).await
        } else {
            None
        };
        if let Some(v4) = v4
            && let Some(selection) = self.v4_route_selection_for_request(
                v4,
                request,
                route_target_override
                    .as_ref()
                    .map(|(target, _)| target.as_str()),
            )
        {
            if let Some((target, source)) = route_target_override {
                log_retry_trace(serde_json::json!({
                    "event": "route_target_override_applied",
                    "service": self.service_name,
                    "session_id": session_id,
                    "source": source,
                    "target": target,
                }));
            }
            return selection;
        }

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
                    return RequestRouteSelection::empty();
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
                    return RequestRouteSelection::empty();
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
                    return RequestRouteSelection::legacy(vec![LoadBalancer::new(
                        Arc::new(candidate.service),
                        self.lb_states.clone(),
                    )]);
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
            .into()
    }

    pub(super) fn v4_route_selection_for_request(
        &self,
        v4: &ProxyConfigV4,
        request: &RouteRequestContext,
        route_target_override: Option<&str>,
    ) -> Option<RequestRouteSelection> {
        let base_view = match self.service_name {
            "claude" => &v4.claude,
            _ => &v4.codex,
        };
        let override_view;
        let view = if let Some(target) = route_target_override {
            match service_view_with_route_target_override(self.service_name, base_view, target) {
                Ok(view) => {
                    override_view = view;
                    &override_view
                }
                Err(err) => {
                    log_retry_trace(serde_json::json!({
                        "event": "lbs_for_request",
                        "service": self.service_name,
                        "mode": "v4_route_target_override_error",
                        "target": target,
                        "error": err.to_string(),
                    }));
                    return Some(RequestRouteSelection::empty());
                }
            }
        } else {
            base_view
        };
        let template =
            match compile_v4_route_plan_template_with_request(self.service_name, view, request) {
                Ok(template) => template,
                Err(err) => {
                    log_retry_trace(serde_json::json!({
                        "event": "lbs_for_request",
                        "service": self.service_name,
                        "mode": "v4_route_plan_compile_error",
                        "error": err.to_string(),
                    }));
                    return Some(RequestRouteSelection::empty());
                }
            };
        log_retry_trace(serde_json::json!({
            "event": "lbs_for_request",
            "service": self.service_name,
            "mode": "v4_route_plan_ir",
            "entry": template.entry.as_str(),
            "candidate_count": template.candidates.len(),
            "provider_endpoint_candidates": template
                .candidates
                .iter()
                .map(|candidate| route_candidate_trace_json(&template, candidate))
                .collect::<Vec<_>>(),
        }));
        Some(RequestRouteSelection::route_graph(template))
    }
}

impl RequestRouteSelection {
    fn empty() -> Self {
        Self::Legacy { lbs: Vec::new() }
    }

    fn legacy(lbs: Vec<LoadBalancer>) -> Self {
        Self::Legacy { lbs }
    }

    fn route_graph(template: RoutePlanTemplate) -> Self {
        Self::RouteGraph { template }
    }

    pub(super) fn is_empty(&self) -> bool {
        match self {
            Self::Legacy { lbs } => lbs.is_empty(),
            Self::RouteGraph { template } => template.candidates.is_empty(),
        }
    }

    pub(super) fn legacy_lbs(&self) -> Option<&[LoadBalancer]> {
        match self {
            Self::Legacy { lbs } => Some(lbs),
            Self::RouteGraph { .. } => None,
        }
    }
}

impl From<Vec<LoadBalancer>> for RequestRouteSelection {
    fn from(lbs: Vec<LoadBalancer>) -> Self {
        Self::legacy(lbs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::ServiceConfig;
    use crate::dashboard_core::StationRoutingBalanceSummary;
    use crate::state::RuntimeConfigState;

    fn candidate(balance: StationRoutingBalanceSummary) -> StationRoutingCandidate {
        StationRoutingCandidate {
            name: "alpha".to_string(),
            service: ServiceConfig {
                name: "alpha".to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: vec![],
            },
            level: 1,
            enabled: true,
            runtime_state: RuntimeConfigState::Normal,
            upstream_count: 0,
            balance,
        }
    }

    #[test]
    fn balance_state_marks_ignored_exhaustion_separately() {
        let candidate = candidate(StationRoutingBalanceSummary {
            snapshots: 1,
            exhausted: 1,
            routing_ignored_exhausted: 1,
            ..StationRoutingBalanceSummary::default()
        });

        assert_eq!(station_balance_state(&candidate), "ignored_exhausted");
        let json = station_candidate_json(&candidate);
        assert_eq!(
            json["balance"]["routing_ignored_exhausted"].as_u64(),
            Some(1)
        );
    }

    #[test]
    fn balance_state_marks_trusted_all_exhausted() {
        let candidate = candidate(StationRoutingBalanceSummary {
            snapshots: 1,
            exhausted: 1,
            routing_snapshots: 1,
            routing_exhausted: 1,
            ..StationRoutingBalanceSummary::default()
        });

        assert_eq!(station_balance_state(&candidate), "all_exhausted");
    }
}
