use crate::lb::LoadBalancer;
use crate::routing_ir::{
    RoutePlanRuntimeState, RoutePlanStationRuntimeState, RoutePlanUpstreamRuntimeState,
};

pub(super) fn route_plan_runtime_state_from_lbs(lbs: &[LoadBalancer]) -> RoutePlanRuntimeState {
    let mut runtime = RoutePlanRuntimeState::default();
    let now = std::time::Instant::now();

    for lb in lbs {
        let mut state = match lb.states.lock() {
            Ok(states) => states
                .get(lb.service.name.as_str())
                .cloned()
                .unwrap_or_default(),
            Err(error) => error
                .into_inner()
                .get(lb.service.name.as_str())
                .cloned()
                .unwrap_or_default(),
        };
        state.ensure_layout(&lb.service.upstreams);

        let mut station = RoutePlanStationRuntimeState {
            last_good_index: state.last_good_index,
            ..RoutePlanStationRuntimeState::default()
        };
        for idx in 0..lb.service.upstreams.len() {
            let cooldown_until = state.cooldown_until.get(idx).and_then(|until| *until);
            let cooldown_active = cooldown_until.is_some_and(|until| now < until);
            let failure_count = if cooldown_until.is_some_and(|until| now >= until) {
                0
            } else {
                state.failure_counts.get(idx).copied().unwrap_or_default()
            };
            station.set_upstream(
                idx,
                RoutePlanUpstreamRuntimeState {
                    failure_count,
                    cooldown_active,
                    usage_exhausted: state.usage_exhausted.get(idx).copied().unwrap_or(false),
                },
            );
        }
        runtime.set_station(lb.service.name.clone(), station);
    }

    runtime
}
