use std::collections::HashSet;
use std::sync::OnceLock;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode, Uri};

use crate::config::RetryStrategy;
use crate::lb::{CooldownBackoff, LoadBalancer};
use crate::logging::{BodyPreview, HeaderEntry, RouteAttemptLog, ServiceTierLog, log_retry_trace};
use crate::routing_ir::{
    RoutePlanAttemptState, RoutePlanExecutor, RoutePlanRuntimeState, RoutePlanSkipReason,
    RoutePlanTemplate, SkippedRouteCandidate, compile_legacy_route_plan_template,
};
use crate::state::SessionBinding;

use super::ProxyService;
use super::attempt_execution::{
    ExecuteSelectedUpstreamParams, SelectedUpstreamExecutionOutcome, execute_selected_upstream,
};
use super::attempt_selection::station_upstreams_exhausted;
use super::provider_orchestration::{
    CrossStationFailoverBlockedParams, cross_station_failover_enabled,
    log_cross_station_failover_blocked, log_same_station_failover_trace,
    next_provider_load_balancer, provider_attempt_limit, station_loop_action_after_attempt,
};
use super::request_preparation::RequestFlavor;
use super::retry::{RetryPlan, backoff_sleep};
use super::route_affinity::apply_session_route_affinity_to_runtime;
use super::route_attempts::{UnsupportedModelSkipParams, record_unsupported_model_skip};
use super::route_executor_runtime::route_plan_runtime_state_from_lbs_with_overrides;

pub(super) struct ExecuteProviderChainParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) lbs: &'a [LoadBalancer],
    pub(super) route_plan_template: Option<&'a RoutePlanTemplate>,
    pub(super) method: &'a Method,
    pub(super) uri: &'a Uri,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) client_headers_entries_cache: &'a OnceLock<Vec<HeaderEntry>>,
    pub(super) client_uri: &'a str,
    pub(super) start: &'a Instant,
    pub(super) started_at_ms: u64,
    pub(super) request_id: u64,
    pub(super) request_body_len: usize,
    pub(super) body_for_upstream: &'a Bytes,
    pub(super) request_model: Option<&'a str>,
    pub(super) session_binding: Option<&'a SessionBinding>,
    pub(super) session_override_config: Option<&'a str>,
    pub(super) global_station_override: Option<&'a str>,
    pub(super) override_model: Option<&'a str>,
    pub(super) override_effort: Option<&'a str>,
    pub(super) override_service_tier: Option<&'a str>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) effective_service_tier: Option<&'a str>,
    pub(super) base_service_tier: &'a ServiceTierLog,
    pub(super) session_id: Option<&'a str>,
    pub(super) cwd: Option<&'a str>,
    pub(super) request_flavor: &'a RequestFlavor,
    pub(super) request_body_previews: bool,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
    pub(super) client_body_debug: Option<&'a BodyPreview>,
    pub(super) client_body_warn: Option<&'a BodyPreview>,
    pub(super) plan: &'a RetryPlan,
    pub(super) cooldown_backoff: CooldownBackoff,
}

pub(super) enum ProviderExecutionOutcome {
    Return(Response<Body>),
    Exhausted(ProviderExecutionState),
}

pub(super) struct ProviderExecutionState {
    pub(super) upstream_chain: Vec<String>,
    pub(super) route_attempts: Vec<RouteAttemptLog>,
    pub(super) last_err: Option<(StatusCode, String)>,
}

#[cfg(test)]
static ROUTE_EXECUTOR_REQUEST_PATH_TEST_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(super) fn route_executor_request_path_test_invocations() -> usize {
    ROUTE_EXECUTOR_REQUEST_PATH_TEST_INVOCATIONS.load(Ordering::SeqCst)
}

pub(super) fn log_retry_options(service_name: &str, request_id: u64, plan: &RetryPlan) {
    let upstream_opt = &plan.upstream;
    let provider_opt = &plan.route;

    log_retry_trace(serde_json::json!({
        "event": "retry_options",
        "service": service_name,
        "request_id": request_id,
        "upstream": {
            "max_attempts": upstream_opt.max_attempts,
            "base_backoff_ms": upstream_opt.base_backoff_ms,
            "max_backoff_ms": upstream_opt.max_backoff_ms,
            "jitter_ms": upstream_opt.jitter_ms,
            "retry_status_ranges": upstream_opt.retry_status_ranges,
            "retry_error_classes": upstream_opt.retry_error_classes,
            "strategy": retry_strategy_name(upstream_opt.strategy),
        },
        "provider": {
            "max_attempts": provider_opt.max_attempts,
            "base_backoff_ms": provider_opt.base_backoff_ms,
            "max_backoff_ms": provider_opt.max_backoff_ms,
            "jitter_ms": provider_opt.jitter_ms,
            "retry_status_ranges": provider_opt.retry_status_ranges,
            "retry_error_classes": provider_opt.retry_error_classes,
            "strategy": retry_strategy_name(provider_opt.strategy),
        },
        "never_status_ranges": plan.never_status_ranges,
        "never_error_classes": plan.never_error_classes,
        "cloudflare_challenge_cooldown_secs": plan.cloudflare_challenge_cooldown_secs,
        "cloudflare_timeout_cooldown_secs": plan.cloudflare_timeout_cooldown_secs,
        "transport_cooldown_secs": plan.transport_cooldown_secs,
        "cooldown_backoff_factor": plan.cooldown_backoff_factor,
        "cooldown_backoff_max_secs": plan.cooldown_backoff_max_secs,
        "allow_cross_station_before_first_output": plan.allow_cross_station_before_first_output,
    }));
}

pub(super) async fn execute_provider_chain_with_route_executor(
    params: ExecuteProviderChainParams<'_>,
) -> ProviderExecutionOutcome {
    #[cfg(test)]
    ROUTE_EXECUTOR_REQUEST_PATH_TEST_INVOCATIONS.fetch_add(1, Ordering::SeqCst);

    let ExecuteProviderChainParams {
        proxy,
        lbs,
        route_plan_template,
        method,
        uri,
        client_headers,
        client_headers_entries_cache,
        client_uri,
        start,
        started_at_ms,
        request_id,
        request_body_len,
        body_for_upstream,
        request_model,
        session_binding,
        session_override_config,
        global_station_override,
        override_model,
        override_effort,
        override_service_tier,
        effective_effort,
        effective_service_tier,
        base_service_tier,
        session_id,
        cwd,
        request_flavor,
        request_body_previews,
        debug_max,
        warn_max,
        client_body_debug,
        client_body_warn,
        plan,
        cooldown_backoff,
    } = params;

    let provider_opt = &plan.route;
    let total_upstreams = lbs
        .iter()
        .map(|lb| lb.service.upstreams.len())
        .sum::<usize>();
    let legacy_template;
    let template = if let Some(template) = route_plan_template {
        template
    } else {
        legacy_template = compile_legacy_route_plan_template(
            proxy.service_name,
            lbs.iter().map(|lb| lb.service.as_ref()),
        );
        &legacy_template
    };
    let executor = RoutePlanExecutor::new(template);
    let upstream_overrides = proxy
        .state
        .get_upstream_meta_overrides(proxy.service_name)
        .await;
    let mut runtime = route_plan_runtime_state_from_lbs_with_overrides(
        proxy.service_name,
        lbs,
        &upstream_overrides,
    );
    let route_graph_key = route_plan_template.map(|template| template.route_graph_key());
    if let (Some(template), Some(route_graph_key)) =
        (route_plan_template, route_graph_key.as_deref())
    {
        apply_session_route_affinity_to_runtime(
            proxy,
            session_id,
            template,
            route_graph_key,
            &mut runtime,
        )
        .await;
    }
    let mut route_state = RoutePlanAttemptState::default();
    let mut upstream_chain: Vec<String> = Vec::new();
    let mut route_attempts: Vec<RouteAttemptLog> = Vec::new();
    let mut tried_stations: HashSet<String> = HashSet::new();
    let strict_multi_config = lbs.len() > 1;
    let cross_station_failover_enabled =
        cross_station_failover_enabled(strict_multi_config, plan, provider_opt);
    let provider_attempt_limit =
        provider_attempt_limit(cross_station_failover_enabled, provider_opt.max_attempts);
    let mut global_attempt: u32 = 0;
    let mut last_err: Option<(StatusCode, String)> = None;

    for provider_attempt in 0..provider_attempt_limit {
        let Some(lb) = next_provider_load_balancer(lbs, &tried_stations) else {
            break;
        };
        let station_name = lb.service.name.clone();

        if let Some(response) =
            execute_station_upstreams_with_route_executor(ExecuteRouteExecutorStationParams {
                proxy,
                lb: &lb,
                station_name: station_name.as_str(),
                method,
                uri,
                client_headers,
                client_headers_entries_cache,
                client_uri,
                start,
                started_at_ms,
                request_id,
                request_body_len,
                body_for_upstream,
                request_model,
                session_binding,
                session_override_config,
                global_station_override,
                override_model,
                override_effort,
                override_service_tier,
                effective_effort,
                effective_service_tier,
                base_service_tier,
                session_id,
                cwd,
                request_flavor,
                request_body_previews,
                debug_max,
                warn_max,
                client_body_debug,
                client_body_warn,
                plan,
                route_graph_key: route_graph_key.as_deref(),
                provider_attempt,
                total_upstreams,
                cooldown_backoff,
                executor: &executor,
                runtime: &runtime,
                route_state: &mut route_state,
                global_attempt: &mut global_attempt,
                last_err: &mut last_err,
                upstream_chain: &mut upstream_chain,
                route_attempts: &mut route_attempts,
            })
            .await
        {
            return ProviderExecutionOutcome::Return(response);
        }

        tried_stations.insert(station_name.clone());

        log_cross_station_failover_blocked(CrossStationFailoverBlockedParams {
            service_name: proxy.service_name,
            request_id,
            station_name: station_name.as_str(),
            strict_multi_config,
            provider_attempt,
            cross_station_failover_enabled,
            provider_opt,
            provider_attempt_limit,
            allow_cross_station_before_first_output: plan.allow_cross_station_before_first_output,
        });

        if provider_opt.base_backoff_ms > 0 && provider_attempt + 1 < provider_attempt_limit {
            backoff_sleep(provider_opt, provider_attempt).await;
        }
    }

    ProviderExecutionOutcome::Exhausted(ProviderExecutionState {
        upstream_chain,
        route_attempts,
        last_err,
    })
}

struct ExecuteRouteExecutorStationParams<'a, 'route> {
    proxy: &'a ProxyService,
    lb: &'a LoadBalancer,
    station_name: &'a str,
    method: &'a Method,
    uri: &'a Uri,
    client_headers: &'a HeaderMap,
    client_headers_entries_cache: &'a OnceLock<Vec<HeaderEntry>>,
    client_uri: &'a str,
    start: &'a Instant,
    started_at_ms: u64,
    request_id: u64,
    request_body_len: usize,
    body_for_upstream: &'a Bytes,
    request_model: Option<&'a str>,
    session_binding: Option<&'a SessionBinding>,
    session_override_config: Option<&'a str>,
    global_station_override: Option<&'a str>,
    override_model: Option<&'a str>,
    override_effort: Option<&'a str>,
    override_service_tier: Option<&'a str>,
    effective_effort: Option<&'a str>,
    effective_service_tier: Option<&'a str>,
    base_service_tier: &'a ServiceTierLog,
    session_id: Option<&'a str>,
    cwd: Option<&'a str>,
    request_flavor: &'a RequestFlavor,
    request_body_previews: bool,
    debug_max: usize,
    warn_max: usize,
    client_body_debug: Option<&'a BodyPreview>,
    client_body_warn: Option<&'a BodyPreview>,
    plan: &'a RetryPlan,
    route_graph_key: Option<&'a str>,
    provider_attempt: u32,
    total_upstreams: usize,
    cooldown_backoff: CooldownBackoff,
    executor: &'a RoutePlanExecutor<'route>,
    runtime: &'a RoutePlanRuntimeState,
    route_state: &'a mut RoutePlanAttemptState,
    global_attempt: &'a mut u32,
    last_err: &'a mut Option<(StatusCode, String)>,
    upstream_chain: &'a mut Vec<String>,
    route_attempts: &'a mut Vec<RouteAttemptLog>,
}

async fn execute_station_upstreams_with_route_executor(
    params: ExecuteRouteExecutorStationParams<'_, '_>,
) -> Option<Response<Body>> {
    let ExecuteRouteExecutorStationParams {
        proxy,
        lb,
        station_name,
        method,
        uri,
        client_headers,
        client_headers_entries_cache,
        client_uri,
        start,
        started_at_ms,
        request_id,
        request_body_len,
        body_for_upstream,
        request_model,
        session_binding,
        session_override_config,
        global_station_override,
        override_model,
        override_effort,
        override_service_tier,
        effective_effort,
        effective_service_tier,
        base_service_tier,
        session_id,
        cwd,
        request_flavor,
        request_body_previews,
        debug_max,
        warn_max,
        client_body_debug,
        client_body_warn,
        plan,
        route_graph_key,
        provider_attempt,
        total_upstreams,
        cooldown_backoff,
        executor,
        runtime,
        route_state,
        global_attempt,
        last_err,
        upstream_chain,
        route_attempts,
    } = params;

    'upstreams: loop {
        let upstream_total = lb.service.upstreams.len();
        let avoid_snapshot =
            hash_set_from_indices(&route_state.avoid_for_station_name(station_name));
        if station_upstreams_exhausted(upstream_total, &avoid_snapshot) {
            log_same_station_failover_trace(
                proxy.service_name,
                request_id,
                station_name,
                upstream_total,
                &avoid_snapshot,
                true,
            );
            break 'upstreams;
        }

        let selection = executor.select_supported_station_candidate_with_runtime_state(
            route_state,
            runtime,
            station_name,
            request_model,
        );
        record_executor_unsupported_model_skips(
            upstream_chain,
            route_attempts,
            &selection.skipped,
            provider_attempt,
            plan.route.max_attempts,
        );

        let avoid_for_station = selection.avoid_for_station.clone();
        let mut avoided_total = selection.avoided_total;
        let Some(selected) = selection.selected else {
            break 'upstreams;
        };
        let selected = selected.selected_upstream;
        let mut avoid_set = hash_set_from_indices(&avoid_for_station);

        match execute_selected_upstream(ExecuteSelectedUpstreamParams {
            proxy,
            lb,
            selected: &selected,
            method,
            uri,
            client_headers,
            client_headers_entries_cache,
            client_uri,
            start,
            started_at_ms,
            request_id,
            request_body_len,
            body_for_upstream,
            request_model,
            session_binding,
            session_override_config,
            global_station_override,
            override_model,
            override_effort,
            override_service_tier,
            effective_effort,
            effective_service_tier,
            base_service_tier,
            session_id,
            cwd,
            request_flavor,
            request_body_previews,
            debug_max,
            warn_max,
            client_body_debug,
            client_body_warn,
            plan,
            route_graph_key,
            upstream_opt: &plan.upstream,
            provider_opt: &plan.route,
            provider_attempt,
            total_upstreams,
            cooldown_backoff,
            global_attempt,
            avoid_set: &mut avoid_set,
            avoided_total: &mut avoided_total,
            last_err,
            upstream_chain,
            route_attempts,
        })
        .await
        {
            SelectedUpstreamExecutionOutcome::ContinueStation => {}
            SelectedUpstreamExecutionOutcome::Return(response) => return Some(response),
        }

        sync_route_state_from_avoid_set(route_state, station_name, &avoid_set);
        debug_assert_eq!(route_state.avoided_total(), avoided_total);

        if station_loop_action_after_attempt(
            proxy.service_name,
            request_id,
            station_name,
            lb.service.upstreams.len(),
            &avoid_set,
        ) {
            break 'upstreams;
        }
    }

    None
}

fn record_executor_unsupported_model_skips(
    upstream_chain: &mut Vec<String>,
    route_attempts: &mut Vec<RouteAttemptLog>,
    skipped: &[SkippedRouteCandidate<'_>],
    provider_attempt: u32,
    provider_max_attempts: u32,
) {
    for skipped in skipped {
        let RoutePlanSkipReason::UnsupportedModel { requested_model } = &skipped.reason else {
            continue;
        };
        let avoid_set = hash_set_from_indices(&skipped.avoid_for_station);
        record_unsupported_model_skip(
            upstream_chain,
            route_attempts,
            UnsupportedModelSkipParams {
                selected: &skipped.selected_upstream,
                requested_model,
                provider_attempt,
                provider_max_attempts,
                avoid_set: &avoid_set,
                avoided_total: skipped.avoided_total,
                total_upstreams: skipped.total_upstreams,
            },
        );
    }
}

fn sync_route_state_from_avoid_set(
    route_state: &mut RoutePlanAttemptState,
    station_name: &str,
    avoid_set: &HashSet<usize>,
) {
    for index in avoid_set {
        route_state.avoid_upstream(station_name, *index);
    }
}

fn hash_set_from_indices(indices: &[usize]) -> HashSet<usize> {
    indices.iter().copied().collect()
}

fn retry_strategy_name(strategy: RetryStrategy) -> &'static str {
    if strategy == RetryStrategy::Failover {
        "failover"
    } else {
        "same_upstream"
    }
}
