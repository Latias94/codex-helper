use std::collections::HashMap;

use crate::lb::LoadBalancer;
use crate::routing_ir::{
    RoutePlanRuntimeState, RoutePlanStationRuntimeState, RoutePlanUpstreamRuntimeState,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::RuntimeConfigState;

pub(super) fn route_plan_runtime_state_from_lbs(lbs: &[LoadBalancer]) -> RoutePlanRuntimeState {
    route_plan_runtime_state_from_lbs_with_overrides("", lbs, &HashMap::new())
}

pub(super) fn route_plan_runtime_state_from_lbs_with_overrides(
    service_name: &str,
    lbs: &[LoadBalancer],
    upstream_overrides: &HashMap<String, (Option<bool>, Option<RuntimeConfigState>)>,
) -> RoutePlanRuntimeState {
    let mut runtime = RoutePlanRuntimeState::default();
    let now = std::time::Instant::now();

    for lb in lbs {
        let state = match lb.states.lock() {
            Ok(mut states) => {
                let entry = states.entry(lb.service.name.clone()).or_default();
                entry.ensure_layout(&lb.service.upstreams);
                entry.clone()
            }
            Err(error) => {
                let mut states = error.into_inner();
                let entry = states.entry(lb.service.name.clone()).or_default();
                entry.ensure_layout(&lb.service.upstreams);
                entry.clone()
            }
        };

        let mut station = RoutePlanStationRuntimeState {
            last_good_index: state.last_good_index,
            ..RoutePlanStationRuntimeState::default()
        };
        for idx in 0..lb.service.upstreams.len() {
            let upstream = &lb.service.upstreams[idx];
            let cooldown_until = state.cooldown_until.get(idx).and_then(|until| *until);
            let cooldown_active = cooldown_until.is_some_and(|until| now < until);
            let failure_count = if cooldown_until.is_some_and(|until| now >= until) {
                0
            } else {
                state.failure_counts.get(idx).copied().unwrap_or_default()
            };
            let override_key = upstream.tags.get("provider_id").and_then(|provider_id| {
                upstream.tags.get("endpoint_id").map(|endpoint_id| {
                    ProviderEndpointKey::new(
                        service_name,
                        provider_id.as_str(),
                        endpoint_id.as_str(),
                    )
                    .stable_key()
                })
            });
            let (enabled_override, state_override) = override_key
                .as_deref()
                .and_then(|key| upstream_overrides.get(key).copied())
                .or_else(|| upstream_overrides.get(upstream.base_url.as_str()).copied())
                .unwrap_or((None, None));
            station.set_upstream(
                idx,
                RoutePlanUpstreamRuntimeState {
                    runtime_disabled: enabled_override == Some(false)
                        || state_override.is_some_and(|state| state != RuntimeConfigState::Normal),
                    failure_count,
                    cooldown_active,
                    usage_exhausted: state.usage_exhausted.get(idx).copied().unwrap_or(false),
                    missing_auth: false,
                },
            );
        }
        runtime.set_station(lb.service.name.clone(), station);
    }

    runtime
}
