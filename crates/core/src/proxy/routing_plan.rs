use std::collections::HashMap;

use crate::config::{ServiceConfig, ServiceConfigManager, UpstreamConfig};
use crate::state::RuntimeConfigState;

#[derive(Debug, Clone)]
pub(super) struct StationRoutingCandidate {
    pub(super) name: String,
    pub(super) service: ServiceConfig,
    pub(super) level: u8,
    pub(super) enabled: bool,
    pub(super) runtime_state: RuntimeConfigState,
    pub(super) upstream_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StationRoutingMode {
    SingleLevelMulti,
    SingleLevelFallbackActiveStation,
    SingleLevelEmpty,
    MultiLevel,
    MultiLevelFallbackActiveStation,
    MultiLevelEmpty,
}

#[derive(Debug, Clone)]
pub(super) struct StationRoutingPlan {
    pub(super) mode: StationRoutingMode,
    pub(super) active_station: Option<String>,
    pub(super) eligible_stations: Vec<StationRoutingCandidate>,
    pub(super) selected_stations: Vec<StationRoutingCandidate>,
}

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

#[derive(Debug)]
pub(super) enum PinnedRoutingSelection {
    BlockedBreakerOpen,
    Missing,
    Selected(StationRoutingCandidate),
}

fn station_candidate(
    name: &str,
    svc: &ServiceConfig,
    runtime_state: RuntimeConfigState,
    level: u8,
    enabled: bool,
) -> StationRoutingCandidate {
    StationRoutingCandidate {
        name: name.to_string(),
        service: svc.clone(),
        level,
        enabled,
        runtime_state,
        upstream_count: svc.upstreams.len(),
    }
}

pub(super) fn build_station_routing_plan(
    mgr: &ServiceConfigManager,
    active_name: Option<&str>,
    meta_overrides: &HashMap<String, (Option<bool>, Option<u8>)>,
    state_overrides: &HashMap<String, RuntimeConfigState>,
    upstream_overrides: &HashMap<String, (Option<bool>, Option<RuntimeConfigState>)>,
) -> StationRoutingPlan {
    let mut eligible_stations = mgr
        .stations()
        .iter()
        .filter_map(|(name, svc)| {
            let (enabled_ovr, level_ovr) = meta_overrides
                .get(name.as_str())
                .copied()
                .unwrap_or((None, None));
            let enabled = enabled_ovr.unwrap_or(svc.enabled);
            let runtime_state = effective_runtime_config_state(state_overrides, name.as_str());
            if !runtime_state_allows_general_routing(runtime_state)
                || !(enabled || active_name.is_some_and(|n| n == name.as_str()))
            {
                return None;
            }

            filtered_service_for_routing(svc, upstream_overrides, false).map(|svc| {
                let level = level_ovr.unwrap_or(svc.level).clamp(1, 10);
                station_candidate(name, &svc, runtime_state, level, enabled)
            })
        })
        .collect::<Vec<_>>();

    let has_multi_level = {
        let mut levels = eligible_stations
            .iter()
            .map(|candidate| candidate.level)
            .collect::<Vec<_>>();
        levels.sort_unstable();
        levels.dedup();
        levels.len() > 1
    };

    if eligible_stations.is_empty() {
        let maybe_active = mgr
            .active_station()
            .filter(|svc| {
                runtime_state_allows_general_routing(effective_runtime_config_state(
                    state_overrides,
                    svc.name.as_str(),
                ))
            })
            .and_then(|svc| filtered_service_for_routing(svc, upstream_overrides, false));
        if let Some(svc) = maybe_active {
            let candidate = station_candidate(
                svc.name.as_str(),
                &svc,
                effective_runtime_config_state(state_overrides, svc.name.as_str()),
                svc.level.clamp(1, 10),
                svc.enabled,
            );
            return StationRoutingPlan {
                mode: if has_multi_level {
                    StationRoutingMode::MultiLevelFallbackActiveStation
                } else {
                    StationRoutingMode::SingleLevelFallbackActiveStation
                },
                active_station: active_name.map(ToOwned::to_owned),
                eligible_stations,
                selected_stations: vec![candidate],
            };
        }

        return StationRoutingPlan {
            mode: if has_multi_level {
                StationRoutingMode::MultiLevelEmpty
            } else {
                StationRoutingMode::SingleLevelEmpty
            },
            active_station: active_name.map(ToOwned::to_owned),
            eligible_stations,
            selected_stations: Vec::new(),
        };
    }

    if !has_multi_level {
        let mut selected_stations = eligible_stations.clone();
        selected_stations.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(active) = active_name
            && let Some(pos) = selected_stations
                .iter()
                .position(|candidate| candidate.name == active)
        {
            let item = selected_stations.remove(pos);
            selected_stations.insert(0, item);
        }
        return StationRoutingPlan {
            mode: StationRoutingMode::SingleLevelMulti,
            active_station: active_name.map(ToOwned::to_owned),
            eligible_stations,
            selected_stations,
        };
    }

    eligible_stations.sort_by(|a, b| {
        let a_active = active_name.is_some_and(|n| n == a.name.as_str());
        let b_active = active_name.is_some_and(|n| n == b.name.as_str());
        a.level
            .cmp(&b.level)
            .then_with(|| b_active.cmp(&a_active))
            .then_with(|| a.name.cmp(&b.name))
    });

    StationRoutingPlan {
        mode: StationRoutingMode::MultiLevel,
        active_station: active_name.map(ToOwned::to_owned),
        selected_stations: eligible_stations.clone(),
        eligible_stations,
    }
}

pub(super) fn resolve_pinned_station_selection(
    mgr: &ServiceConfigManager,
    name: &str,
    state_overrides: &HashMap<String, RuntimeConfigState>,
    upstream_overrides: &HashMap<String, (Option<bool>, Option<RuntimeConfigState>)>,
) -> PinnedRoutingSelection {
    let runtime_state = effective_runtime_config_state(state_overrides, name);
    if !runtime_state_allows_pinned_routing(runtime_state) {
        return PinnedRoutingSelection::BlockedBreakerOpen;
    }

    let Some(base_svc) = mgr.station(name) else {
        let Some(active_svc) = mgr.active_station() else {
            return PinnedRoutingSelection::Missing;
        };
        let Some(svc) = filtered_service_for_routing(active_svc, upstream_overrides, true) else {
            return PinnedRoutingSelection::Missing;
        };
        return PinnedRoutingSelection::Selected(station_candidate(
            svc.name.as_str(),
            &svc,
            effective_runtime_config_state(state_overrides, svc.name.as_str()),
            svc.level.clamp(1, 10),
            svc.enabled,
        ));
    };
    let Some(svc) = filtered_service_for_routing(base_svc, upstream_overrides, true) else {
        return PinnedRoutingSelection::Missing;
    };

    PinnedRoutingSelection::Selected(station_candidate(
        name,
        &svc,
        runtime_state,
        svc.level.clamp(1, 10),
        svc.enabled,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServiceConfigManager, UpstreamAuth};
    use std::collections::BTreeMap;

    fn upstream(base_url: &str) -> UpstreamConfig {
        UpstreamConfig {
            base_url: base_url.to_string(),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }
    }

    fn service(
        name: &str,
        enabled: bool,
        level: u8,
        upstreams: Vec<UpstreamConfig>,
    ) -> ServiceConfig {
        ServiceConfig {
            name: name.to_string(),
            alias: None,
            enabled,
            level,
            upstreams,
        }
    }

    fn manager(active: Option<&str>, services: Vec<ServiceConfig>) -> ServiceConfigManager {
        let configs = services
            .into_iter()
            .map(|svc| (svc.name.clone(), svc))
            .collect::<HashMap<_, _>>();
        ServiceConfigManager {
            active: active.map(str::to_string),
            default_profile: None,
            profiles: BTreeMap::new(),
            configs,
        }
    }

    fn names(stations: &[StationRoutingCandidate]) -> Vec<String> {
        stations
            .iter()
            .map(|station| station.name.clone())
            .collect()
    }

    #[test]
    fn auto_single_level_prefers_active_then_alphabetical() {
        let mgr = manager(
            Some("beta"),
            vec![
                service("gamma", true, 1, vec![upstream("https://gamma.example/v1")]),
                service("alpha", true, 1, vec![upstream("https://alpha.example/v1")]),
                service("beta", true, 1, vec![upstream("https://beta.example/v1")]),
            ],
        );

        let plan = build_station_routing_plan(
            &mgr,
            mgr.active.as_deref(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(plan.mode, StationRoutingMode::SingleLevelMulti);
        assert_eq!(
            names(&plan.selected_stations),
            vec!["beta", "alpha", "gamma"]
        );
    }

    #[test]
    fn auto_multi_level_sorts_by_level_active_and_name() {
        let mgr = manager(
            Some("beta"),
            vec![
                service("delta", true, 2, vec![upstream("https://delta.example/v1")]),
                service("alpha", true, 2, vec![upstream("https://alpha.example/v1")]),
                service("gamma", true, 1, vec![upstream("https://gamma.example/v1")]),
                service("beta", true, 1, vec![upstream("https://beta.example/v1")]),
            ],
        );

        let plan = build_station_routing_plan(
            &mgr,
            mgr.active.as_deref(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(plan.mode, StationRoutingMode::MultiLevel);
        assert_eq!(
            names(&plan.selected_stations),
            vec!["beta", "gamma", "alpha", "delta"]
        );
    }

    #[test]
    fn auto_keeps_active_even_when_disabled() {
        let mgr = manager(
            Some("beta"),
            vec![
                service(
                    "alpha",
                    false,
                    1,
                    vec![upstream("https://alpha.example/v1")],
                ),
                service("beta", false, 1, vec![upstream("https://beta.example/v1")]),
                service("gamma", true, 1, vec![upstream("https://gamma.example/v1")]),
            ],
        );

        let plan = build_station_routing_plan(
            &mgr,
            mgr.active.as_deref(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(plan.mode, StationRoutingMode::SingleLevelMulti);
        assert_eq!(names(&plan.selected_stations), vec!["beta", "gamma"]);
    }

    #[test]
    fn auto_falls_back_to_stable_active_station_when_no_candidate_is_auto_eligible() {
        let mgr = manager(
            None,
            vec![
                service(
                    "alpha",
                    false,
                    1,
                    vec![upstream("https://alpha.example/v1")],
                ),
                service("beta", false, 1, vec![upstream("https://beta.example/v1")]),
            ],
        );

        let plan = build_station_routing_plan(
            &mgr,
            mgr.active.as_deref(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(
            plan.mode,
            StationRoutingMode::SingleLevelFallbackActiveStation
        );
        assert_eq!(names(&plan.selected_stations), vec!["alpha"]);
    }

    #[test]
    fn resolve_pinned_station_selection_keeps_half_open_but_blocks_breaker_open() {
        let mgr = manager(
            Some("beta"),
            vec![
                service("alpha", true, 1, vec![upstream("https://alpha.example/v1")]),
                service("beta", true, 1, vec![upstream("https://beta.example/v1")]),
            ],
        );
        let half_open = HashMap::from([("alpha".to_string(), RuntimeConfigState::HalfOpen)]);
        let breaker_open = HashMap::from([("beta".to_string(), RuntimeConfigState::BreakerOpen)]);

        match resolve_pinned_station_selection(&mgr, "alpha", &half_open, &HashMap::new()) {
            PinnedRoutingSelection::Selected(candidate) => {
                assert_eq!(candidate.name, "alpha");
                assert_eq!(candidate.runtime_state, RuntimeConfigState::HalfOpen);
            }
            other => panic!("unexpected pinned selection: {other:?}"),
        }

        assert!(matches!(
            resolve_pinned_station_selection(&mgr, "beta", &breaker_open, &HashMap::new()),
            PinnedRoutingSelection::BlockedBreakerOpen
        ));
    }

    #[test]
    fn resolve_pinned_station_selection_falls_back_to_active_when_pinned_station_is_missing() {
        let mgr = manager(
            Some("beta"),
            vec![
                service("alpha", true, 1, vec![upstream("https://alpha.example/v1")]),
                service("beta", true, 1, vec![upstream("https://beta.example/v1")]),
            ],
        );

        match resolve_pinned_station_selection(&mgr, "missing", &HashMap::new(), &HashMap::new()) {
            PinnedRoutingSelection::Selected(candidate) => {
                assert_eq!(candidate.name, "beta");
            }
            other => panic!("unexpected pinned selection: {other:?}"),
        }
    }

    #[test]
    fn resolve_pinned_station_selection_keeps_missing_when_pinned_station_has_no_routable_upstreams()
     {
        let mgr = manager(
            Some("beta"),
            vec![
                service(
                    "alpha",
                    true,
                    1,
                    vec![upstream("https://alpha-half-open.example/v1")],
                ),
                service("beta", true, 1, vec![upstream("https://beta.example/v1")]),
            ],
        );
        let upstream_overrides = HashMap::from([(
            "https://alpha-half-open.example/v1".to_string(),
            (None, Some(RuntimeConfigState::BreakerOpen)),
        )]);

        assert!(matches!(
            resolve_pinned_station_selection(&mgr, "alpha", &HashMap::new(), &upstream_overrides),
            PinnedRoutingSelection::Missing
        ));
    }
}
