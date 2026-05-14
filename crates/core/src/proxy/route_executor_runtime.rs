use std::collections::HashMap;

use crate::lb::LoadBalancer;
use crate::routing_ir::{RoutePlanRuntimeState, RoutePlanUpstreamRuntimeState};
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::RuntimeConfigState;

pub(super) fn route_plan_runtime_state_from_lbs(
    service_name: &str,
    lbs: &[LoadBalancer],
) -> RoutePlanRuntimeState {
    route_plan_runtime_state_from_lbs_with_overrides(service_name, lbs, &HashMap::new())
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
                entry.ensure_layout(lb.service.name.as_str(), &lb.service.upstreams);
                entry.clone()
            }
            Err(error) => {
                let mut states = error.into_inner();
                let entry = states.entry(lb.service.name.clone()).or_default();
                entry.ensure_layout(lb.service.name.as_str(), &lb.service.upstreams);
                entry.clone()
            }
        };

        for idx in 0..lb.service.upstreams.len() {
            let upstream = &lb.service.upstreams[idx];
            let provider_endpoint_key = provider_endpoint_key_for_upstream(
                service_name,
                lb.service.name.as_str(),
                idx,
                upstream,
            );
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
            let upstream_state = RoutePlanUpstreamRuntimeState {
                runtime_disabled: enabled_override == Some(false)
                    || state_override.is_some_and(|state| state != RuntimeConfigState::Normal),
                failure_count,
                cooldown_active,
                usage_exhausted: state.usage_exhausted.get(idx).copied().unwrap_or(false),
                missing_auth: false,
            };
            runtime.set_provider_endpoint(provider_endpoint_key.clone(), upstream_state);
            if state.last_good_index == Some(idx) && runtime.affinity_provider_endpoint().is_none()
            {
                runtime.set_affinity_provider_endpoint(Some(provider_endpoint_key));
            }
        }
    }

    runtime
}

fn provider_endpoint_key_for_upstream(
    service_name: &str,
    station_name: &str,
    upstream_index: usize,
    upstream: &crate::config::UpstreamConfig,
) -> ProviderEndpointKey {
    let provider_id = upstream
        .tags
        .get("provider_id")
        .cloned()
        .unwrap_or_else(|| format!("{station_name}#{upstream_index}"));
    let endpoint_id = upstream
        .tags
        .get("endpoint_id")
        .cloned()
        .unwrap_or_else(|| upstream_index.to_string());

    ProviderEndpointKey::new(service_name, provider_id, endpoint_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServiceConfig, UpstreamAuth, UpstreamConfig};
    use crate::lb::{FAILURE_THRESHOLD, LbState, LoadBalancer};
    use std::sync::{Arc, Mutex};

    fn upstream(base_url: &str, provider_id: &str) -> UpstreamConfig {
        UpstreamConfig {
            base_url: base_url.to_string(),
            auth: UpstreamAuth::default(),
            tags: HashMap::from([
                ("provider_id".to_string(), provider_id.to_string()),
                ("endpoint_id".to_string(), "default".to_string()),
            ]),
            supported_models: HashMap::new(),
            model_mapping: HashMap::new(),
        }
    }

    fn load_balancer(
        name: &str,
        upstreams: Vec<UpstreamConfig>,
        states: Arc<Mutex<HashMap<String, LbState>>>,
    ) -> LoadBalancer {
        LoadBalancer::new(
            Arc::new(ServiceConfig {
                name: name.to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams,
            }),
            states,
        )
    }

    #[test]
    fn route_plan_runtime_state_migrates_reordered_lb_state_to_provider_endpoint_keys() {
        let states = Arc::new(Mutex::new(HashMap::new()));
        let initial = load_balancer(
            "routing",
            vec![
                upstream("https://primary.example/v1", "primary"),
                upstream("https://backup.example/v1", "backup"),
            ],
            states.clone(),
        );

        {
            let mut guard = states.lock().expect("lb state lock");
            let entry = guard.entry("routing".to_string()).or_default();
            entry.ensure_layout(initial.service.name.as_str(), &initial.service.upstreams);
            entry.failure_counts[0] = FAILURE_THRESHOLD;
            entry.cooldown_until[0] =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(30));
            entry.usage_exhausted[1] = true;
            entry.last_good_index = Some(1);
        }

        let reordered = load_balancer(
            "routing",
            vec![
                upstream("https://backup.example/v1", "backup"),
                upstream("https://primary.example/v1", "primary"),
            ],
            states,
        );

        let runtime = route_plan_runtime_state_from_lbs("codex", &[reordered]);

        let primary =
            runtime.provider_endpoint(&ProviderEndpointKey::new("codex", "primary", "default"));
        assert_eq!(primary.failure_count, FAILURE_THRESHOLD);
        assert!(primary.cooldown_active);
        assert!(!primary.usage_exhausted);

        let backup =
            runtime.provider_endpoint(&ProviderEndpointKey::new("codex", "backup", "default"));
        assert_eq!(backup.failure_count, 0);
        assert!(!backup.cooldown_active);
        assert!(backup.usage_exhausted);
        assert_eq!(
            runtime.affinity_provider_endpoint(),
            Some(&ProviderEndpointKey::new("codex", "backup", "default"))
        );
    }
}
