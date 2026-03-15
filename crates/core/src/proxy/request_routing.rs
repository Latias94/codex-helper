use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{ProxyConfig, ServiceConfig, ServiceConfigManager, UpstreamConfig};
use crate::lb::LoadBalancer;
use crate::logging::log_retry_trace;
use crate::state::RuntimeConfigState;

use super::ProxyService;

fn effective_runtime_config_state(
    state_overrides: &HashMap<String, RuntimeConfigState>,
    station_name: &str,
) -> RuntimeConfigState {
    state_overrides
        .get(station_name)
        .copied()
        .unwrap_or_default()
}

fn runtime_state_allows_general_routing(state: RuntimeConfigState) -> bool {
    state == RuntimeConfigState::Normal
}

fn runtime_state_allows_pinned_routing(state: RuntimeConfigState) -> bool {
    state != RuntimeConfigState::BreakerOpen
}

fn effective_upstream_enabled_override(
    upstream_overrides: &HashMap<String, (Option<bool>, Option<RuntimeConfigState>)>,
    base_url: &str,
) -> bool {
    upstream_overrides
        .get(base_url)
        .and_then(|(enabled, _)| *enabled)
        .unwrap_or(true)
}

fn effective_upstream_runtime_state(
    upstream_overrides: &HashMap<String, (Option<bool>, Option<RuntimeConfigState>)>,
    base_url: &str,
) -> RuntimeConfigState {
    upstream_overrides
        .get(base_url)
        .and_then(|(_, state)| *state)
        .unwrap_or_default()
}

fn upstream_allows_general_routing(
    upstream: &UpstreamConfig,
    upstream_overrides: &HashMap<String, (Option<bool>, Option<RuntimeConfigState>)>,
) -> bool {
    effective_upstream_enabled_override(upstream_overrides, upstream.base_url.as_str())
        && runtime_state_allows_general_routing(effective_upstream_runtime_state(
            upstream_overrides,
            upstream.base_url.as_str(),
        ))
}

fn upstream_allows_pinned_routing(
    upstream: &UpstreamConfig,
    upstream_overrides: &HashMap<String, (Option<bool>, Option<RuntimeConfigState>)>,
) -> bool {
    effective_upstream_enabled_override(upstream_overrides, upstream.base_url.as_str())
        && runtime_state_allows_pinned_routing(effective_upstream_runtime_state(
            upstream_overrides,
            upstream.base_url.as_str(),
        ))
}

fn filtered_service_for_routing(
    svc: &ServiceConfig,
    upstream_overrides: &HashMap<String, (Option<bool>, Option<RuntimeConfigState>)>,
    pinned: bool,
) -> Option<ServiceConfig> {
    let upstreams = svc
        .upstreams
        .iter()
        .filter(|upstream| {
            if pinned {
                upstream_allows_pinned_routing(upstream, upstream_overrides)
            } else {
                upstream_allows_general_routing(upstream, upstream_overrides)
            }
        })
        .cloned()
        .collect::<Vec<_>>();
    if upstreams.is_empty() {
        return None;
    }

    Some(ServiceConfig {
        upstreams,
        ..svc.clone()
    })
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
        if let Some((name, source)) = self.pinned_config(mgr, session_id).await {
            let runtime_state = effective_runtime_config_state(&state_overrides, name.as_str());
            if !runtime_state_allows_pinned_routing(runtime_state) {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "pinned_blocked_breaker_open",
                    "pinned_source": source,
                    "pinned_name": name,
                    "runtime_state": "breaker_open",
                    "active_station": mgr.active.as_deref(),
                    "station_count": mgr.station_count(),
                }));
                return Vec::new();
            }
            let svc = if let Some(svc) = mgr.station(&name) {
                filtered_service_for_routing(svc, &upstream_overrides, true)
            } else {
                mgr.active_station()
                    .and_then(|svc| filtered_service_for_routing(svc, &upstream_overrides, true))
            };
            if let Some(svc) = svc {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "pinned",
                    "pinned_source": source,
                    "pinned_name": name,
                    "runtime_state": format!("{runtime_state:?}").to_ascii_lowercase(),
                    "selected_station": svc.name,
                    "selected_level": svc.level.clamp(1, 10),
                    "selected_upstreams": svc.upstreams.len(),
                    "active_station": mgr.active.as_deref(),
                    "station_count": mgr.station_count(),
                }));
                return vec![LoadBalancer::new(Arc::new(svc), self.lb_states.clone())];
            }
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

        let active_name = mgr.active.as_deref();
        let mut configs = mgr
            .stations()
            .iter()
            .filter_map(|(name, svc)| {
                let (enabled_ovr, _) = meta_overrides
                    .get(name.as_str())
                    .copied()
                    .unwrap_or((None, None));
                let enabled = enabled_ovr.unwrap_or(svc.enabled);
                let runtime_state = effective_runtime_config_state(&state_overrides, name.as_str());
                if !runtime_state_allows_general_routing(runtime_state)
                    || !(enabled || active_name.is_some_and(|n| n == name.as_str()))
                {
                    return None;
                }

                filtered_service_for_routing(svc, &upstream_overrides, false)
                    .map(|svc| (name.clone(), svc))
            })
            .collect::<Vec<_>>();

        let has_multi_level = {
            let mut levels = configs
                .iter()
                .map(|(name, svc)| {
                    let (_, level_ovr) = meta_overrides
                        .get(name.as_str())
                        .copied()
                        .unwrap_or((None, None));
                    level_ovr.unwrap_or(svc.level).clamp(1, 10)
                })
                .collect::<Vec<_>>();
            levels.sort_unstable();
            levels.dedup();
            levels.len() > 1
        };

        if !has_multi_level {
            let eligible_details = || {
                configs
                    .iter()
                    .map(|(name, svc)| {
                        let (_, level_ovr) = meta_overrides
                            .get(name.as_str())
                            .copied()
                            .unwrap_or((None, None));
                        serde_json::json!({
                            "name": (*name).clone(),
                            "level": level_ovr.unwrap_or(svc.level).clamp(1, 10),
                            "enabled": svc.enabled,
                            "runtime_state": format!(
                                "{:?}",
                                effective_runtime_config_state(&state_overrides, name.as_str())
                            )
                            .to_ascii_lowercase(),
                            "upstreams": svc.upstreams.len(),
                        })
                    })
                    .collect::<Vec<_>>()
            };

            let mut ordered = configs
                .iter()
                .map(|(name, svc)| (name.clone(), svc.clone()))
                .collect::<Vec<_>>();
            ordered.sort_by(|(a, _), (b, _)| a.cmp(b));
            if let Some(active) = active_name
                && let Some(pos) = ordered.iter().position(|(n, _)| n == active)
            {
                let item = ordered.remove(pos);
                ordered.insert(0, item);
            }

            let lbs = ordered
                .into_iter()
                .map(|(_, svc)| LoadBalancer::new(Arc::new(svc), self.lb_states.clone()))
                .collect::<Vec<_>>();
            if !lbs.is_empty() {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "single_level_multi",
                    "active_station": active_name,
                    "selected_stations": lbs.iter().map(|lb| lb.service.name.clone()).collect::<Vec<_>>(),
                    "eligible_stations": configs.iter().map(|(n, _)| (*n).clone()).collect::<Vec<_>>(),
                    "eligible_details": eligible_details(),
                    "eligible_count": configs.len(),
                }));
                return lbs;
            }

            if let Some(svc) = mgr
                .active_station()
                .filter(|svc| {
                    runtime_state_allows_general_routing(effective_runtime_config_state(
                        &state_overrides,
                        svc.name.as_str(),
                    ))
                })
                .and_then(|svc| filtered_service_for_routing(svc, &upstream_overrides, false))
            {
                log_retry_trace(serde_json::json!({
                    "event": "lbs_for_request",
                    "service": self.service_name,
                    "session_id": session_id,
                    "mode": "single_level_fallback_active_station",
                    "active_station": active_name,
                    "selected_station": svc.name,
                    "selected_level": svc.level.clamp(1, 10),
                    "selected_upstreams": svc.upstreams.len(),
                    "eligible_stations": configs.iter().map(|(n, _)| (*n).clone()).collect::<Vec<_>>(),
                    "eligible_details": eligible_details(),
                    "eligible_count": configs.len(),
                }));
                return vec![LoadBalancer::new(Arc::new(svc), self.lb_states.clone())];
            }

            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "single_level_empty",
                "active_station": active_name,
                "eligible_stations": configs.iter().map(|(n, _)| (*n).clone()).collect::<Vec<_>>(),
                "eligible_details": eligible_details(),
                "eligible_count": configs.len(),
            }));
            return Vec::new();
        }

        configs.sort_by(|(a_name, a), (b_name, b)| {
            let a_level = meta_overrides
                .get(a_name.as_str())
                .and_then(|(_, l)| *l)
                .unwrap_or(a.level)
                .clamp(1, 10);
            let b_level = meta_overrides
                .get(b_name.as_str())
                .and_then(|(_, l)| *l)
                .unwrap_or(b.level)
                .clamp(1, 10);
            let a_active = active_name.is_some_and(|n| n == a_name.as_str());
            let b_active = active_name.is_some_and(|n| n == b_name.as_str());
            a_level
                .cmp(&b_level)
                .then_with(|| b_active.cmp(&a_active))
                .then_with(|| a_name.cmp(b_name))
        });

        let lbs = configs
            .into_iter()
            .map(|(_, svc)| LoadBalancer::new(Arc::new(svc.clone()), self.lb_states.clone()))
            .collect::<Vec<_>>();
        if !lbs.is_empty() {
            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "multi_level",
                "active_station": active_name,
                "eligible_stations": lbs.iter().map(|lb| serde_json::json!({
                    "name": lb.service.name,
                    "level": lb.service.level.clamp(1, 10),
                    "upstreams": lb.service.upstreams.len(),
                })).collect::<Vec<_>>(),
                "eligible_count": lbs.len(),
            }));
            return lbs;
        }

        if let Some(svc) = mgr
            .active_station()
            .filter(|svc| {
                runtime_state_allows_general_routing(effective_runtime_config_state(
                    &state_overrides,
                    svc.name.as_str(),
                ))
            })
            .and_then(|svc| filtered_service_for_routing(svc, &upstream_overrides, false))
        {
            log_retry_trace(serde_json::json!({
                "event": "lbs_for_request",
                "service": self.service_name,
                "session_id": session_id,
                "mode": "multi_level_fallback_active_station",
                "active_station": active_name,
                "selected_station": svc.name,
                "selected_level": svc.level.clamp(1, 10),
                "selected_upstreams": svc.upstreams.len(),
            }));
            return vec![LoadBalancer::new(Arc::new(svc), self.lb_states.clone())];
        }
        log_retry_trace(serde_json::json!({
            "event": "lbs_for_request",
            "service": self.service_name,
            "session_id": session_id,
            "mode": "multi_level_empty",
            "active_station": active_name,
        }));
        Vec::new()
    }
}
