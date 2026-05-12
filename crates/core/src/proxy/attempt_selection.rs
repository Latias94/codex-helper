use std::collections::HashSet;

#[cfg(test)]
use crate::lb::{LoadBalancer, SelectedUpstream};
#[cfg(test)]
use crate::logging::RouteAttemptLog;
#[cfg(test)]
use crate::model_routing;

#[cfg(test)]
use super::route_attempts::{UnsupportedModelSkipParams, record_unsupported_model_skip};

#[cfg(test)]
pub(super) struct SelectSupportedUpstreamParams<'a> {
    pub lb: &'a LoadBalancer,
    pub request_model: Option<&'a str>,
    pub strict_multi_config: bool,
    pub avoid_set: &'a mut HashSet<usize>,
    pub upstream_chain: &'a mut Vec<String>,
    pub route_attempts: &'a mut Vec<RouteAttemptLog>,
    pub avoided_total: &'a mut usize,
    pub provider_attempt: u32,
    pub provider_max_attempts: u32,
    pub total_upstreams: usize,
}

pub(super) fn station_upstreams_exhausted(
    upstream_total: usize,
    avoid_set: &HashSet<usize>,
) -> bool {
    upstream_total > 0
        && avoid_set
            .iter()
            .filter(|&&idx| idx < upstream_total)
            .count()
            >= upstream_total
}

#[cfg(test)]
pub(super) fn select_supported_upstream(
    params: SelectSupportedUpstreamParams<'_>,
) -> Option<SelectedUpstream> {
    let SelectSupportedUpstreamParams {
        lb,
        request_model,
        strict_multi_config,
        avoid_set,
        upstream_chain,
        route_attempts,
        avoided_total,
        provider_attempt,
        provider_max_attempts,
        total_upstreams,
    } = params;

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

        let selected = select_supported_upstream(SelectSupportedUpstreamParams {
            lb: &lb,
            request_model: Some("gpt-5"),
            strict_multi_config: false,
            avoid_set: &mut avoid_set,
            upstream_chain: &mut upstream_chain,
            route_attempts: &mut route_attempts,
            avoided_total: &mut avoided_total,
            provider_attempt: 0,
            provider_max_attempts: 2,
            total_upstreams: 2,
        })
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

        let selected = select_supported_upstream(SelectSupportedUpstreamParams {
            lb: &lb,
            request_model: Some("gpt-5"),
            strict_multi_config: true,
            avoid_set: &mut avoid_set,
            upstream_chain: &mut upstream_chain,
            route_attempts: &mut route_attempts,
            avoided_total: &mut avoided_total,
            provider_attempt: 0,
            provider_max_attempts: 2,
            total_upstreams: 2,
        });

        assert!(selected.is_none());
        assert_eq!(avoid_set.len(), 2);
        assert_eq!(avoided_total, 2);
        assert_eq!(upstream_chain.len(), 2);
        assert_eq!(route_attempts.len(), 2);
    }

    #[test]
    fn station_upstreams_exhausted_counts_only_valid_upstream_indices() {
        assert!(!station_upstreams_exhausted(
            2,
            &HashSet::from([0usize, 99usize])
        ));
        assert!(station_upstreams_exhausted(
            2,
            &HashSet::from([0usize, 1usize, 99usize])
        ));
        assert!(!station_upstreams_exhausted(
            3,
            &HashSet::from([0usize, 2usize])
        ));
        assert!(!station_upstreams_exhausted(
            0,
            &HashSet::from([0usize, 1usize])
        ));
    }
}
