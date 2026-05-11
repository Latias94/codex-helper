use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use crate::lb::{LoadBalancer, SelectedUpstream};
use crate::logging::log_retry_trace;
use crate::model_routing;
use crate::routing_ir::{
    RouteCandidate, RoutePlanAttemptState, RoutePlanExecutor, RoutePlanSkipReason,
    compile_legacy_route_plan_template,
};

use super::route_executor_runtime::route_plan_runtime_state_from_lbs;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct RouteExecutorShadowAttempt {
    pub(super) decision: &'static str,
    pub(super) station_name: String,
    pub(super) upstream_index: usize,
    pub(super) upstream_base_url: String,
    pub(super) provider_id: Option<String>,
    pub(super) endpoint_id: Option<String>,
    pub(super) route_path: Vec<String>,
    pub(super) avoid_for_station: Vec<usize>,
    pub(super) avoided_total: usize,
    pub(super) total_upstreams: usize,
    pub(super) reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RouteExecutorShadowReport {
    pub(super) matches: bool,
    pub(super) legacy_attempts: Vec<RouteExecutorShadowAttempt>,
    pub(super) executor_attempts: Vec<RouteExecutorShadowAttempt>,
}

pub(super) fn maybe_log_route_executor_shadow_diff(
    service_name: &str,
    request_id: u64,
    lbs: &[LoadBalancer],
    request_model: Option<&str>,
) {
    if !route_executor_shadow_enabled() {
        return;
    }

    let report = route_executor_shadow_report(service_name, lbs, request_model);
    if report.matches {
        return;
    }

    log_retry_trace(serde_json::json!({
        "event": "route_executor_shadow_mismatch",
        "service": service_name,
        "request_id": request_id,
        "request_model": request_model,
        "legacy_attempts": report.legacy_attempts,
        "executor_attempts": report.executor_attempts,
    }));
}

pub(super) fn route_executor_shadow_report(
    service_name: &str,
    lbs: &[LoadBalancer],
    request_model: Option<&str>,
) -> RouteExecutorShadowReport {
    let legacy_attempts = legacy_shadow_attempts(lbs, request_model);
    let executor_attempts = executor_shadow_attempts(service_name, lbs, request_model);
    RouteExecutorShadowReport {
        matches: legacy_attempts == executor_attempts,
        legacy_attempts,
        executor_attempts,
    }
}

fn route_executor_shadow_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_bool("CODEX_HELPER_ROUTE_EXECUTOR_SHADOW_TRACE"))
}

fn env_bool(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

fn executor_shadow_attempts(
    service_name: &str,
    lbs: &[LoadBalancer],
    request_model: Option<&str>,
) -> Vec<RouteExecutorShadowAttempt> {
    let template =
        compile_legacy_route_plan_template(service_name, lbs.iter().map(|lb| lb.service.as_ref()));
    let executor = RoutePlanExecutor::new(&template);
    let mut state = RoutePlanAttemptState::default();
    let runtime = route_plan_runtime_state_from_lbs(lbs);
    let mut attempts = Vec::new();

    loop {
        let selection = executor.select_supported_candidate_with_runtime_state(
            &mut state,
            &runtime,
            request_model,
        );
        attempts.extend(selection.skipped.into_iter().map(|skipped| {
            shadow_attempt(
                "skipped_capability_mismatch",
                &skipped.selected_upstream,
                ShadowRouteMetadata::from_candidate(skipped.candidate),
                skipped.avoid_for_station,
                skipped.avoided_total,
                skipped.total_upstreams,
                Some(skip_reason(&skipped.reason)),
            )
        }));

        let Some(selected) = selection.selected else {
            break;
        };
        attempts.push(shadow_attempt(
            "selected",
            &selected.selected_upstream,
            ShadowRouteMetadata::from_candidate(selected.candidate),
            selection.avoid_for_station,
            selection.avoided_total,
            selection.total_upstreams,
            None,
        ));
        state.avoid_selected(&selected);
    }

    attempts
}

fn legacy_shadow_attempts(
    lbs: &[LoadBalancer],
    request_model: Option<&str>,
) -> Vec<RouteExecutorShadowAttempt> {
    let total_upstreams = lbs
        .iter()
        .map(|lb| lb.service.upstreams.len())
        .sum::<usize>();
    let mut attempts = Vec::new();
    let mut avoided_total = 0usize;

    for lb in lbs {
        let shadow_lb = clone_load_balancer_with_state_snapshot(lb);
        let mut avoid = HashSet::new();

        while !station_upstreams_exhausted(shadow_lb.service.upstreams.len(), &avoid) {
            let Some(selected) = shadow_lb.select_upstream_avoiding_strict(&avoid) else {
                break;
            };

            if let Some(requested_model) = request_model
                && !model_routing::is_model_supported(
                    &selected.upstream.supported_models,
                    &selected.upstream.model_mapping,
                    requested_model,
                )
            {
                if avoid.insert(selected.index) {
                    avoided_total = avoided_total.saturating_add(1);
                }
                attempts.push(shadow_attempt(
                    "skipped_capability_mismatch",
                    &selected,
                    ShadowRouteMetadata::from_legacy_selected(&selected),
                    sorted_avoid_set(&avoid),
                    avoided_total,
                    total_upstreams,
                    Some("unsupported_model"),
                ));
                continue;
            }

            attempts.push(shadow_attempt(
                "selected",
                &selected,
                ShadowRouteMetadata::from_legacy_selected(&selected),
                sorted_avoid_set(&avoid),
                avoided_total,
                total_upstreams,
                None,
            ));
            if avoid.insert(selected.index) {
                avoided_total = avoided_total.saturating_add(1);
            }
        }
    }

    attempts
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShadowRouteMetadata {
    endpoint_id: Option<String>,
    route_path: Vec<String>,
}

impl ShadowRouteMetadata {
    fn from_candidate(candidate: &RouteCandidate) -> Self {
        Self {
            endpoint_id: Some(candidate.endpoint_id.clone()),
            route_path: candidate.route_path.clone(),
        }
    }

    fn from_legacy_selected(selected: &SelectedUpstream) -> Self {
        Self {
            endpoint_id: Some(legacy_selected_endpoint_id(selected)),
            route_path: vec![
                "legacy".to_string(),
                selected.station_name.clone(),
                legacy_selected_provider_id(selected),
            ],
        }
    }
}

fn legacy_selected_provider_id(selected: &SelectedUpstream) -> String {
    selected
        .upstream
        .tags
        .get("provider_id")
        .cloned()
        .unwrap_or_else(|| format!("{}#{}", selected.station_name, selected.index))
}

fn legacy_selected_endpoint_id(selected: &SelectedUpstream) -> String {
    selected
        .upstream
        .tags
        .get("endpoint_id")
        .cloned()
        .unwrap_or_else(|| selected.index.to_string())
}

fn clone_load_balancer_with_state_snapshot(lb: &LoadBalancer) -> LoadBalancer {
    let state_snapshot = match lb.states.lock() {
        Ok(states) => states.clone(),
        Err(error) => error.into_inner().clone(),
    };
    LoadBalancer::new(lb.service.clone(), Arc::new(Mutex::new(state_snapshot)))
}

fn shadow_attempt(
    decision: &'static str,
    selected: &SelectedUpstream,
    route_metadata: ShadowRouteMetadata,
    avoid_for_station: Vec<usize>,
    avoided_total: usize,
    total_upstreams: usize,
    reason: Option<&'static str>,
) -> RouteExecutorShadowAttempt {
    RouteExecutorShadowAttempt {
        decision,
        station_name: selected.station_name.clone(),
        upstream_index: selected.index,
        upstream_base_url: selected.upstream.base_url.clone(),
        provider_id: selected.upstream.tags.get("provider_id").cloned(),
        endpoint_id: route_metadata.endpoint_id,
        route_path: route_metadata.route_path,
        avoid_for_station,
        avoided_total,
        total_upstreams,
        reason,
    }
}

fn skip_reason(reason: &RoutePlanSkipReason) -> &'static str {
    match reason {
        RoutePlanSkipReason::UnsupportedModel { .. } => "unsupported_model",
    }
}

fn sorted_avoid_set(avoid: &HashSet<usize>) -> Vec<usize> {
    let mut values = avoid.iter().copied().collect::<Vec<_>>();
    values.sort_unstable();
    values
}

fn station_upstreams_exhausted(upstream_total: usize, avoid: &HashSet<usize>) -> bool {
    upstream_total > 0
        && avoid.iter().filter(|&&idx| idx < upstream_total).count() >= upstream_total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServiceConfig, UpstreamAuth, UpstreamConfig};
    use crate::lb::{COOLDOWN_SECS, CooldownBackoff, LbState};
    use std::collections::HashMap;

    fn upstream(base_url: &str, provider_id: &str, supported_models: &[&str]) -> UpstreamConfig {
        UpstreamConfig {
            base_url: base_url.to_string(),
            auth: UpstreamAuth::default(),
            tags: HashMap::from([("provider_id".to_string(), provider_id.to_string())]),
            supported_models: supported_models
                .iter()
                .map(|model| ((*model).to_string(), true))
                .collect(),
            model_mapping: HashMap::new(),
        }
    }

    fn load_balancer(name: &str, upstreams: Vec<UpstreamConfig>) -> LoadBalancer {
        LoadBalancer::new(
            Arc::new(ServiceConfig {
                name: name.to_string(),
                alias: None,
                enabled: true,
                level: 1,
                upstreams,
            }),
            Arc::new(Mutex::new(HashMap::<String, LbState>::new())),
        )
    }

    #[test]
    fn shadow_report_matches_legacy_for_model_skip_and_failover_order() {
        let lb = load_balancer(
            "routing",
            vec![
                upstream("https://old.example/v1", "old", &["gpt-4.1"]),
                upstream("https://new.example/v1", "new", &["gpt-5"]),
            ],
        );

        let report = route_executor_shadow_report("codex", &[lb], Some("gpt-5"));

        assert!(report.matches);
        assert_eq!(
            report
                .executor_attempts
                .iter()
                .map(|attempt| attempt.decision)
                .collect::<Vec<_>>(),
            vec!["skipped_capability_mismatch", "selected"]
        );
        assert_eq!(report.executor_attempts[0].avoid_for_station, vec![0]);
        assert_eq!(report.executor_attempts[1].avoid_for_station, vec![0]);
        assert_eq!(report.executor_attempts[0].station_name, "routing");
        assert_eq!(report.executor_attempts[0].upstream_index, 0);
        assert_eq!(
            report.executor_attempts[0].endpoint_id.as_deref(),
            Some("0")
        );
        assert_eq!(
            report.executor_attempts[0].route_path,
            vec!["legacy", "routing", "old"]
        );
    }

    #[test]
    fn shadow_report_keeps_station_scoped_avoid_indices() {
        let alpha = load_balancer(
            "alpha",
            vec![upstream("https://alpha.example/v1", "alpha", &[])],
        );
        let beta = load_balancer(
            "beta",
            vec![
                upstream("https://beta-one.example/v1", "beta-one", &[]),
                upstream("https://beta-two.example/v1", "beta-two", &[]),
            ],
        );

        let report = route_executor_shadow_report("codex", &[alpha, beta], None);

        assert!(report.matches);
        assert_eq!(
            report
                .executor_attempts
                .iter()
                .map(|attempt| (attempt.station_name.as_str(), attempt.upstream_index))
                .collect::<Vec<_>>(),
            vec![("alpha", 0), ("beta", 0), ("beta", 1)]
        );
        assert_eq!(
            report.executor_attempts[1].avoid_for_station,
            Vec::<usize>::new()
        );
        assert_eq!(report.executor_attempts[2].avoid_for_station, vec![0]);
    }

    #[test]
    fn shadow_report_matches_last_good_state_without_mutating_real_state() {
        let lb = load_balancer(
            "routing",
            vec![
                upstream("https://primary.example/v1", "primary", &[]),
                upstream("https://sticky.example/v1", "sticky", &[]),
            ],
        );
        lb.record_result_with_backoff(
            1,
            true,
            COOLDOWN_SECS,
            CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        );

        let before = lb
            .select_upstream_avoiding_strict(&HashSet::new())
            .expect("selected before shadow");
        assert_eq!(before.index, 1);

        let report = route_executor_shadow_report("codex", std::slice::from_ref(&lb), None);

        assert!(report.matches);
        assert_eq!(report.legacy_attempts[0].upstream_index, 1);
        assert_eq!(report.executor_attempts[0].upstream_index, 1);

        let after = lb
            .select_upstream_avoiding_strict(&HashSet::new())
            .expect("selected after shadow");
        assert_eq!(after.index, 1);
    }

    #[test]
    fn shadow_report_matches_cooldown_state_without_mutating_real_state() {
        let lb = load_balancer(
            "routing",
            vec![
                upstream("https://cooldown.example/v1", "cooldown", &[]),
                upstream("https://ready.example/v1", "ready", &[]),
            ],
        );
        lb.penalize_with_backoff(
            0,
            60,
            "test",
            CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        );

        let report = route_executor_shadow_report("codex", std::slice::from_ref(&lb), None);

        assert!(report.matches);
        assert_eq!(
            report
                .executor_attempts
                .iter()
                .map(|attempt| attempt.upstream_index)
                .collect::<Vec<_>>(),
            vec![1]
        );

        let selected = lb
            .select_upstream_avoiding_strict(&HashSet::new())
            .expect("selected after shadow");
        assert_eq!(selected.index, 1);
    }

    #[test]
    fn shadow_report_skips_exhausted_station_and_continues_to_next_station() {
        let alpha = load_balancer(
            "alpha",
            vec![upstream("https://alpha.example/v1", "alpha", &[])],
        );
        alpha.penalize_with_backoff(
            0,
            60,
            "test",
            CooldownBackoff {
                factor: 1,
                max_secs: 0,
            },
        );
        let beta = load_balancer(
            "beta",
            vec![upstream("https://beta.example/v1", "beta", &[])],
        );

        let report = route_executor_shadow_report("codex", &[alpha, beta], None);

        assert!(report.matches);
        assert_eq!(
            report
                .executor_attempts
                .iter()
                .map(|attempt| (attempt.station_name.as_str(), attempt.upstream_index))
                .collect::<Vec<_>>(),
            vec![("beta", 0)]
        );
    }

    #[test]
    fn shadow_report_matches_usage_exhaustion_fallback_state() {
        let lb = load_balancer(
            "routing",
            vec![
                upstream("https://limited.example/v1", "limited", &[]),
                upstream("https://available.example/v1", "available", &[]),
            ],
        );
        {
            let mut guard = lb.states.lock().expect("lb state lock");
            let entry = guard
                .entry("routing".to_string())
                .or_insert_with(LbState::default);
            entry.ensure_layout(&lb.service.upstreams);
            entry.usage_exhausted[0] = true;
        }

        let report = route_executor_shadow_report("codex", std::slice::from_ref(&lb), None);

        assert!(report.matches);
        assert_eq!(
            report
                .executor_attempts
                .iter()
                .map(|attempt| attempt.upstream_index)
                .collect::<Vec<_>>(),
            vec![1, 0]
        );
    }

    #[test]
    fn shadow_report_matches_all_usage_exhausted_second_round_fallback() {
        let lb = load_balancer(
            "routing",
            vec![
                upstream("https://limited-one.example/v1", "limited-one", &[]),
                upstream("https://limited-two.example/v1", "limited-two", &[]),
            ],
        );
        {
            let mut guard = lb.states.lock().expect("lb state lock");
            let entry = guard
                .entry("routing".to_string())
                .or_insert_with(LbState::default);
            entry.ensure_layout(&lb.service.upstreams);
            entry.usage_exhausted[0] = true;
            entry.usage_exhausted[1] = true;
        }

        let report = route_executor_shadow_report("codex", std::slice::from_ref(&lb), None);

        assert!(report.matches);
        assert_eq!(
            report
                .executor_attempts
                .iter()
                .map(|attempt| attempt.upstream_index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }
}
