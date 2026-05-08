use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode, Uri};

use crate::config::RetryStrategy;
use crate::lb::{CooldownBackoff, LoadBalancer};
use crate::logging::{BodyPreview, HeaderEntry, RouteAttemptLog, ServiceTierLog, log_retry_trace};
use crate::state::SessionBinding;

use super::ProxyService;
use super::attempt_execution::{
    ExecuteSelectedUpstreamParams, SelectedUpstreamExecutionOutcome, execute_selected_upstream,
};
use super::attempt_selection::{select_supported_upstream, station_upstreams_exhausted};
use super::provider_orchestration::{
    CrossStationFailoverBlockedParams, cross_station_failover_enabled,
    log_cross_station_failover_blocked, log_same_station_failover_trace,
    next_provider_load_balancer, provider_attempt_limit, station_loop_action_after_attempt,
};
use super::request_preparation::RequestFlavor;
use super::retry::{RetryPlan, backoff_sleep};

pub(super) struct ExecuteProviderChainParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) lbs: &'a [LoadBalancer],
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

pub(super) async fn execute_provider_chain(
    params: ExecuteProviderChainParams<'_>,
) -> ProviderExecutionOutcome {
    let ExecuteProviderChainParams {
        proxy,
        lbs,
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
    let mut avoid: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut upstream_chain: Vec<String> = Vec::new();
    let mut route_attempts: Vec<RouteAttemptLog> = Vec::new();
    let mut avoided_total: usize = 0;
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

        if let Some(response) = execute_station_upstreams(ExecuteStationUpstreamsParams {
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
            provider_attempt,
            total_upstreams,
            strict_multi_config,
            cooldown_backoff,
            avoid: &mut avoid,
            global_attempt: &mut global_attempt,
            avoided_total: &mut avoided_total,
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

struct ExecuteStationUpstreamsParams<'a> {
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
    provider_attempt: u32,
    total_upstreams: usize,
    strict_multi_config: bool,
    cooldown_backoff: CooldownBackoff,
    avoid: &'a mut HashMap<String, HashSet<usize>>,
    global_attempt: &'a mut u32,
    avoided_total: &'a mut usize,
    last_err: &'a mut Option<(StatusCode, String)>,
    upstream_chain: &'a mut Vec<String>,
    route_attempts: &'a mut Vec<RouteAttemptLog>,
}

async fn execute_station_upstreams(
    params: ExecuteStationUpstreamsParams<'_>,
) -> Option<Response<Body>> {
    let ExecuteStationUpstreamsParams {
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
        provider_attempt,
        total_upstreams,
        strict_multi_config,
        cooldown_backoff,
        avoid,
        global_attempt,
        avoided_total,
        last_err,
        upstream_chain,
        route_attempts,
    } = params;

    'upstreams: loop {
        let avoid_set = avoid.entry(station_name.to_string()).or_default();
        let upstream_total = lb.service.upstreams.len();
        if station_upstreams_exhausted(upstream_total, avoid_set) {
            log_same_station_failover_trace(
                proxy.service_name,
                request_id,
                station_name,
                upstream_total,
                avoid_set,
                true,
            );
            break 'upstreams;
        }

        let selected = select_supported_upstream(
            lb,
            request_model,
            strict_multi_config,
            avoid_set,
            upstream_chain,
            route_attempts,
            avoided_total,
            provider_attempt,
            plan.route.max_attempts,
            total_upstreams,
        );
        let Some(selected) = selected else {
            break 'upstreams;
        };

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
            upstream_opt: &plan.upstream,
            provider_opt: &plan.route,
            provider_attempt,
            total_upstreams,
            cooldown_backoff,
            global_attempt,
            avoid_set,
            avoided_total,
            last_err,
            upstream_chain,
            route_attempts,
        })
        .await
        {
            SelectedUpstreamExecutionOutcome::ContinueStation => {}
            SelectedUpstreamExecutionOutcome::Return(response) => return Some(response),
        }

        if station_loop_action_after_attempt(
            proxy.service_name,
            request_id,
            station_name,
            lb.service.upstreams.len(),
            avoid_set,
        ) {
            break 'upstreams;
        }
    }

    None
}

fn retry_strategy_name(strategy: RetryStrategy) -> &'static str {
    if strategy == RetryStrategy::Failover {
        "failover"
    } else {
        "same_upstream"
    }
}
