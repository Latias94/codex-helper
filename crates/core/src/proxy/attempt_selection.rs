use std::collections::HashSet;

use crate::lb::{LoadBalancer, SelectedUpstream};
use crate::logging::RouteAttemptLog;
use crate::model_routing;

use super::route_attempts::{UnsupportedModelSkipParams, record_unsupported_model_skip};

pub(super) fn station_upstreams_exhausted(
    upstream_total: usize,
    avoid_set: &HashSet<usize>,
) -> bool {
    upstream_total > 0 && avoid_set.len() >= upstream_total
}

pub(super) fn select_supported_upstream(
    lb: &LoadBalancer,
    request_model: Option<&str>,
    strict_multi_config: bool,
    avoid_set: &mut HashSet<usize>,
    upstream_chain: &mut Vec<String>,
    route_attempts: &mut Vec<RouteAttemptLog>,
    avoided_total: &mut usize,
    provider_attempt: u32,
    provider_max_attempts: u32,
    total_upstreams: usize,
) -> Option<SelectedUpstream> {
    loop {
        let upstream_total = lb.service.upstreams.len();
        if station_upstreams_exhausted(upstream_total, avoid_set) {
            return None;
        }

        let next = {
            let avoid_ref: &HashSet<usize> = &*avoid_set;
            if strict_multi_config {
                lb.select_upstream_avoiding_strict(avoid_ref)
            } else {
                lb.select_upstream_avoiding(avoid_ref)
            }
        };
        let selected = next?;

        if let Some(requested_model) = request_model {
            let supported = model_routing::is_model_supported(
                &selected.upstream.supported_models,
                &selected.upstream.model_mapping,
                requested_model,
            );
            if !supported {
                if avoid_set.insert(selected.index) {
                    *avoided_total = avoided_total.saturating_add(1);
                }
                record_unsupported_model_skip(
                    upstream_chain,
                    route_attempts,
                    UnsupportedModelSkipParams {
                        selected: &selected,
                        requested_model,
                        provider_attempt,
                        provider_max_attempts,
                        avoid_set,
                        avoided_total: *avoided_total,
                        total_upstreams,
                    },
                );
                continue;
            }
        }

        return Some(selected);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::config::{ServiceConfig, UpstreamAuth, UpstreamConfig};
    use crate::lb::LbState;

    fn test_upstream(base_url: &str, supported_models: &[&str]) -> UpstreamConfig {
        let mut supported = HashMap::new();
        for model in supported_models {
            supported.insert((*model).to_string(), true);
        }
        UpstreamConfig {
            base_url: base_url.to_string(),
            auth: UpstreamAuth::default(),
            tags: HashMap::new(),
            supported_models: supported,
            model_mapping: HashMap::new(),
        }
    }

    fn test_load_balancer(upstreams: Vec<UpstreamConfig>) -> LoadBalancer {
        let service = ServiceConfig {
            name: "alpha".to_string(),
            alias: None,
            enabled: true,
            level: 1,
            upstreams,
        };
        LoadBalancer::new(
            Arc::new(service),
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        )
    }

    #[test]
    fn select_supported_upstream_skips_unsupported_candidate() {
        let lb = test_load_balancer(vec![
            test_upstream("https://one.example/v1", &["gpt-4.1"]),
            test_upstream("https://two.example/v1", &["gpt-5"]),
        ]);
        let mut avoid_set = HashSet::new();
        let mut upstream_chain = Vec::new();
        let mut route_attempts = Vec::new();
        let mut avoided_total = 0;

        let selected = select_supported_upstream(
            &lb,
            Some("gpt-5"),
            false,
            &mut avoid_set,
            &mut upstream_chain,
            &mut route_attempts,
            &mut avoided_total,
            0,
            2,
            2,
        )
        .expect("selected");

        assert_eq!(selected.index, 1);
        assert!(avoid_set.contains(&0));
        assert_eq!(avoided_total, 1);
        assert_eq!(upstream_chain.len(), 1);
        assert!(upstream_chain[0].contains("skipped_unsupported_model=gpt-5"));
        assert_eq!(route_attempts.len(), 1);
        assert_eq!(route_attempts[0].decision, "skipped_capability_mismatch");
        assert_eq!(route_attempts[0].provider_attempt, Some(1));
        assert_eq!(route_attempts[0].provider_max_attempts, Some(2));
        assert_eq!(route_attempts[0].avoid_for_station, vec![0]);
    }

    #[test]
    fn select_supported_upstream_returns_none_when_all_candidates_are_unsupported() {
        let lb = test_load_balancer(vec![
            test_upstream("https://one.example/v1", &["gpt-4.1"]),
            test_upstream("https://two.example/v1", &["gpt-4o"]),
        ]);
        let mut avoid_set = HashSet::new();
        let mut upstream_chain = Vec::new();
        let mut route_attempts = Vec::new();
        let mut avoided_total = 0;

        let selected = select_supported_upstream(
            &lb,
            Some("gpt-5"),
            true,
            &mut avoid_set,
            &mut upstream_chain,
            &mut route_attempts,
            &mut avoided_total,
            0,
            2,
            2,
        );

        assert!(selected.is_none());
        assert_eq!(avoid_set.len(), 2);
        assert_eq!(avoided_total, 2);
        assert_eq!(upstream_chain.len(), 2);
        assert_eq!(route_attempts.len(), 2);
    }

    #[test]
    fn station_upstreams_exhausted_checks_full_avoid_set() {
        let avoid_set = HashSet::from([0usize, 2usize]);

        assert!(station_upstreams_exhausted(2, &avoid_set));
        assert!(!station_upstreams_exhausted(3, &avoid_set));
        assert!(!station_upstreams_exhausted(0, &avoid_set));
    }
}
