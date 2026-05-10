use std::collections::HashSet;

use crate::config::RetryStrategy;
use crate::lb::LoadBalancer;
use crate::logging::log_retry_trace;

use super::attempt_selection::station_upstreams_exhausted;
use super::retry::{RetryLayerOptions, RetryPlan};

pub(super) fn cross_station_failover_enabled(
    strict_multi_config: bool,
    plan: &RetryPlan,
    provider_opt: &RetryLayerOptions,
) -> bool {
    strict_multi_config
        && plan.allow_cross_station_before_first_output
        && provider_opt.strategy == RetryStrategy::Failover
}

pub(super) fn provider_attempt_limit(
    cross_station_failover_enabled: bool,
    provider_max_attempts: u32,
) -> u32 {
    if cross_station_failover_enabled {
        provider_max_attempts
    } else {
        1
    }
}

pub(super) fn next_provider_load_balancer(
    lbs: &[LoadBalancer],
    tried_stations: &HashSet<String>,
) -> Option<LoadBalancer> {
    lbs.iter()
        .find(|lb| !tried_stations.contains(&lb.service.name))
        .cloned()
}

pub(super) fn station_loop_action_after_attempt(
    service_name: &str,
    request_id: u64,
    station_name: &str,
    upstream_total: usize,
    avoid_set: &HashSet<usize>,
) -> bool {
    if station_upstreams_exhausted(upstream_total, avoid_set) {
        log_same_station_failover_trace(
            service_name,
            request_id,
            station_name,
            upstream_total,
            avoid_set,
            true,
        );
        return true;
    }

    if !avoid_set.is_empty() {
        log_same_station_failover_trace(
            service_name,
            request_id,
            station_name,
            upstream_total,
            avoid_set,
            false,
        );
    }
    false
}

pub(super) struct CrossStationFailoverBlockedParams<'a> {
    pub(super) service_name: &'a str,
    pub(super) request_id: u64,
    pub(super) station_name: &'a str,
    pub(super) strict_multi_config: bool,
    pub(super) provider_attempt: u32,
    pub(super) cross_station_failover_enabled: bool,
    pub(super) provider_opt: &'a RetryLayerOptions,
    pub(super) provider_attempt_limit: u32,
    pub(super) allow_cross_station_before_first_output: bool,
}

pub(super) fn log_cross_station_failover_blocked(params: CrossStationFailoverBlockedParams<'_>) {
    let CrossStationFailoverBlockedParams {
        service_name,
        request_id,
        station_name,
        strict_multi_config,
        provider_attempt,
        cross_station_failover_enabled,
        provider_opt,
        provider_attempt_limit,
        allow_cross_station_before_first_output,
    } = params;

    if !(strict_multi_config
        && provider_attempt == 0
        && !cross_station_failover_enabled
        && provider_opt.max_attempts > 1)
    {
        return;
    }

    log_retry_trace(serde_json::json!({
        "event": "cross_station_failover_blocked",
        "service": service_name,
        "request_id": request_id,
        "station_name": station_name,
        "provider_strategy": if provider_opt.strategy == RetryStrategy::Failover { "failover" } else { "same_upstream" },
        "configured_provider_max_attempts": provider_opt.max_attempts,
        "effective_provider_max_attempts": provider_attempt_limit,
        "allow_cross_station_before_first_output": allow_cross_station_before_first_output,
    }));
}

fn sorted_avoid_indices(avoid: &HashSet<usize>) -> Vec<usize> {
    let mut indices = avoid.iter().copied().collect::<Vec<_>>();
    indices.sort_unstable();
    indices
}

pub(super) fn log_same_station_failover_trace(
    service_name: &str,
    request_id: u64,
    station_name: &str,
    upstream_total: usize,
    avoid_set: &HashSet<usize>,
    exhausted: bool,
) {
    let event = if exhausted {
        "same_station_exhausted"
    } else {
        "same_station_failover"
    };
    log_retry_trace(serde_json::json!({
        "event": event,
        "service": service_name,
        "request_id": request_id,
        "station_name": station_name,
        "upstream_total": upstream_total,
        "avoided_indices": sorted_avoid_indices(avoid_set),
        "next_action": if exhausted {
            "consider_next_station"
        } else {
            "retry_another_upstream_within_station"
        },
    }));
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::config::{RetryStrategy, ServiceConfig, UpstreamAuth, UpstreamConfig};
    use crate::lb::LbState;

    fn test_load_balancer(name: &str) -> LoadBalancer {
        LoadBalancer::new(
            Arc::new(ServiceConfig {
                name: name.to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams: vec![UpstreamConfig {
                    base_url: format!("https://{name}.example/v1"),
                    auth: UpstreamAuth::default(),
                    tags: HashMap::new(),
                    supported_models: HashMap::new(),
                    model_mapping: HashMap::new(),
                }],
            }),
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        )
    }

    #[test]
    fn next_provider_load_balancer_skips_tried_station_names() {
        let lbs = vec![test_load_balancer("a"), test_load_balancer("b")];
        let tried = HashSet::from([String::from("a")]);

        let next = next_provider_load_balancer(&lbs, &tried).expect("next");

        assert_eq!(next.service.name, "b");
    }

    #[test]
    fn provider_attempt_limit_respects_cross_station_flag() {
        assert_eq!(provider_attempt_limit(false, 4), 1);
        assert_eq!(provider_attempt_limit(true, 4), 4);
    }

    #[test]
    fn station_loop_action_ignores_out_of_range_avoids() {
        assert!(!station_loop_action_after_attempt(
            "codex",
            1,
            "alpha",
            2,
            &HashSet::from([0usize, 99usize])
        ));
        assert!(station_loop_action_after_attempt(
            "codex",
            1,
            "alpha",
            2,
            &HashSet::from([0usize, 1usize, 99usize])
        ));
    }

    #[test]
    fn cross_station_failover_enabled_requires_failover_strategy_and_guardrail() {
        let provider_opt = RetryLayerOptions {
            max_attempts: 3,
            base_backoff_ms: 0,
            max_backoff_ms: 0,
            jitter_ms: 0,
            retry_status_ranges: Vec::new(),
            retry_error_classes: Vec::new(),
            strategy: RetryStrategy::Failover,
        };
        let mut plan = RetryPlan {
            upstream: provider_opt.clone(),
            route: provider_opt.clone(),
            allow_cross_station_before_first_output: true,
            never_status_ranges: Vec::new(),
            never_error_classes: Vec::new(),
            cloudflare_challenge_cooldown_secs: 0,
            cloudflare_timeout_cooldown_secs: 0,
            transport_cooldown_secs: 0,
            cooldown_backoff_factor: 1,
            cooldown_backoff_max_secs: 0,
        };

        assert!(cross_station_failover_enabled(true, &plan, &provider_opt));
        assert!(!cross_station_failover_enabled(false, &plan, &provider_opt));

        plan.allow_cross_station_before_first_output = false;
        assert!(!cross_station_failover_enabled(true, &plan, &provider_opt));

        let same_upstream_provider = RetryLayerOptions {
            strategy: RetryStrategy::SameUpstream,
            ..provider_opt.clone()
        };
        assert!(!cross_station_failover_enabled(
            true,
            &RetryPlan {
                allow_cross_station_before_first_output: true,
                route: provider_opt.clone(),
                ..plan
            },
            &same_upstream_provider,
        ));
    }
}
