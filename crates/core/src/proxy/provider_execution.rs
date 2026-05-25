use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::OnceLock;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode, Uri};

use crate::config::{RetryStrategy, RoutingAffinityPolicyV5};
use crate::lb::{CooldownBackoff, LoadBalancer};
use crate::logging::{BodyPreview, HeaderEntry, RouteAttemptLog, ServiceTierLog, log_retry_trace};
use crate::routing_ir::{
    RouteCandidate, RoutePlanAttemptState, RoutePlanExecutor, RoutePlanRuntimeState,
    RoutePlanSkipReason, SelectedRouteCandidate, SkippedRouteCandidate,
    SkippedStationRouteCandidate, compile_legacy_route_plan_template,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::{SessionBinding, SessionIdentitySource};
use crate::usage_providers;

use super::ProxyService;
use super::attempt_execution::{
    ExecuteSelectedUpstreamParams, SelectedUpstreamExecutionOutcome, execute_selected_upstream,
};
use super::attempt_selection::station_upstreams_exhausted;
use super::attempt_target::AttemptTarget;
use super::concurrency_limits::{ConcurrencyAcquireError, ConcurrencyPermit};
use super::provider_orchestration::{
    CrossStationFailoverBlockedParams, cross_station_failover_enabled,
    log_cross_station_failover_blocked, log_same_station_failover_trace,
    next_provider_load_balancer, provider_attempt_limit, station_loop_action_after_attempt,
};
use super::request_preparation::RequestFlavor;
use super::request_routing::RequestRouteSelection;
use super::retry::{RetryLayerOptions, RetryPlan, backoff_sleep};
use super::route_affinity::apply_session_route_affinity_for_template;
use super::route_attempts::{UnsupportedModelSkipParams, record_unsupported_model_skip};
use super::route_executor_runtime::route_plan_runtime_state_from_lbs_with_overrides;
use super::route_unavailability::route_unavailable_report;

const COMPACT_ROUTE_UNAVAILABLE_WAIT_MAX_SECS: u64 = 10;

#[derive(Clone, Copy)]
struct CompactProviderFailoverPolicy {
    strict_affinity: bool,
    allow_provider_failover: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestContinuityDecision {
    StatelessOrSessionPreferred,
    ProviderStateBound { requires_known_affinity: bool },
}

#[derive(Clone, Copy)]
struct ProviderChainAttemptPolicy {
    compact: CompactProviderFailoverPolicy,
    continuity: RequestContinuityDecision,
    strict_multi_config: bool,
    cross_station_failover_enabled: bool,
    provider_attempt_limit: u32,
}

fn request_continuity_decision(
    request_flavor: &RequestFlavor,
    affinity_policy: Option<RoutingAffinityPolicyV5>,
) -> RequestContinuityDecision {
    if request_flavor.is_remote_compaction_request()
        && (request_flavor.remote_compaction_requires_affinity
            || matches!(affinity_policy, Some(RoutingAffinityPolicyV5::Hard)))
    {
        RequestContinuityDecision::ProviderStateBound {
            requires_known_affinity: request_flavor.remote_compaction_requires_affinity,
        }
    } else {
        RequestContinuityDecision::StatelessOrSessionPreferred
    }
}

fn compact_provider_failover_policy(
    request_flavor: &RequestFlavor,
    affinity_policy: Option<RoutingAffinityPolicyV5>,
) -> CompactProviderFailoverPolicy {
    let strict_affinity = matches!(
        request_continuity_decision(request_flavor, affinity_policy),
        RequestContinuityDecision::ProviderStateBound { .. }
    );
    CompactProviderFailoverPolicy {
        strict_affinity,
        allow_provider_failover: !request_flavor.is_remote_compaction_request() || !strict_affinity,
    }
}

impl RequestContinuityDecision {
    fn trace_label(self) -> &'static str {
        match self {
            Self::StatelessOrSessionPreferred => "stateless_or_session_preferred",
            Self::ProviderStateBound { .. } => "provider_state_bound",
        }
    }
}

impl ProviderChainAttemptPolicy {
    fn route_graph(
        request_flavor: &RequestFlavor,
        affinity_policy: Option<RoutingAffinityPolicyV5>,
    ) -> Self {
        let continuity = request_continuity_decision(request_flavor, affinity_policy);
        Self {
            compact: compact_provider_failover_policy(request_flavor, affinity_policy),
            continuity,
            strict_multi_config: false,
            cross_station_failover_enabled: false,
            provider_attempt_limit: 1,
        }
    }

    fn legacy(
        request_flavor: &RequestFlavor,
        plan: &RetryPlan,
        provider_opt: &RetryLayerOptions,
        strict_multi_config: bool,
    ) -> Self {
        let cross_station_failover_enabled =
            cross_station_failover_enabled(strict_multi_config, plan, provider_opt);
        let provider_attempt_limit_value =
            provider_attempt_limit(cross_station_failover_enabled, provider_opt.max_attempts);
        let continuity = request_continuity_decision(request_flavor, None);
        Self {
            compact: compact_provider_failover_policy(request_flavor, None),
            continuity,
            strict_multi_config,
            cross_station_failover_enabled,
            provider_attempt_limit: provider_attempt_limit_value,
        }
    }

    fn allow_provider_failover(self) -> bool {
        self.compact.allow_provider_failover
    }

    fn requires_known_affinity(self) -> bool {
        matches!(
            self.continuity,
            RequestContinuityDecision::ProviderStateBound {
                requires_known_affinity: true
            }
        )
    }

    fn continuity_class(self) -> &'static str {
        self.continuity.trace_label()
    }

    fn provider_failover_blocked_reason(self) -> Option<&'static str> {
        if self.allow_provider_failover() {
            None
        } else if matches!(
            self.continuity,
            RequestContinuityDecision::ProviderStateBound { .. }
        ) {
            Some("provider_state_bound")
        } else {
            Some("provider_failover_disabled")
        }
    }

    fn missing_affinity_trace_reason(self) -> &'static str {
        debug_assert!(self.requires_known_affinity());
        "state_bound_compact_missing_affinity"
    }
}

pub(super) struct ExecuteProviderChainParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) route_selection: &'a RequestRouteSelection,
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
    pub(super) session_identity_source: Option<SessionIdentitySource>,
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

#[derive(Clone, Copy)]
struct ProviderExecutionContext<'a> {
    proxy: &'a ProxyService,
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
    session_identity_source: Option<SessionIdentitySource>,
    cwd: Option<&'a str>,
    request_flavor: &'a RequestFlavor,
    request_body_previews: bool,
    debug_max: usize,
    warn_max: usize,
    client_body_debug: Option<&'a BodyPreview>,
    client_body_warn: Option<&'a BodyPreview>,
    plan: &'a RetryPlan,
    cooldown_backoff: CooldownBackoff,
}

struct SelectedAttemptExecutionParams<'a> {
    legacy_lb: Option<&'a LoadBalancer>,
    target: &'a AttemptTarget,
    route_graph_key: Option<&'a str>,
    allow_provider_failover: bool,
    provider_attempt: u32,
    total_upstreams: usize,
    global_attempt: &'a mut u32,
    avoid_set: &'a mut HashSet<usize>,
    avoided_total: &'a mut usize,
    last_err: &'a mut Option<(StatusCode, String)>,
    upstream_chain: &'a mut Vec<String>,
    route_attempts: &'a mut Vec<RouteAttemptLog>,
    concurrency_permit: Option<ConcurrencyPermit>,
}

impl<'a> ProviderExecutionContext<'a> {
    fn from_params(params: &ExecuteProviderChainParams<'a>) -> Self {
        Self {
            proxy: params.proxy,
            method: params.method,
            uri: params.uri,
            client_headers: params.client_headers,
            client_headers_entries_cache: params.client_headers_entries_cache,
            client_uri: params.client_uri,
            start: params.start,
            started_at_ms: params.started_at_ms,
            request_id: params.request_id,
            request_body_len: params.request_body_len,
            body_for_upstream: params.body_for_upstream,
            request_model: params.request_model,
            session_binding: params.session_binding,
            session_override_config: params.session_override_config,
            global_station_override: params.global_station_override,
            override_model: params.override_model,
            override_effort: params.override_effort,
            override_service_tier: params.override_service_tier,
            effective_effort: params.effective_effort,
            effective_service_tier: params.effective_service_tier,
            base_service_tier: params.base_service_tier,
            session_id: params.session_id,
            session_identity_source: params.session_identity_source,
            cwd: params.cwd,
            request_flavor: params.request_flavor,
            request_body_previews: params.request_body_previews,
            debug_max: params.debug_max,
            warn_max: params.warn_max,
            client_body_debug: params.client_body_debug,
            client_body_warn: params.client_body_warn,
            plan: params.plan,
            cooldown_backoff: params.cooldown_backoff,
        }
    }

    fn upstream_opt(self) -> &'a RetryLayerOptions {
        &self.plan.upstream
    }

    fn provider_opt(self) -> &'a RetryLayerOptions {
        &self.plan.route
    }

    async fn execute_selected_attempt<'attempt>(
        self,
        params: SelectedAttemptExecutionParams<'attempt>,
    ) -> SelectedUpstreamExecutionOutcome
    where
        'a: 'attempt,
    {
        execute_selected_upstream(ExecuteSelectedUpstreamParams {
            proxy: self.proxy,
            legacy_lb: params.legacy_lb,
            target: params.target,
            method: self.method,
            uri: self.uri,
            client_headers: self.client_headers,
            client_headers_entries_cache: self.client_headers_entries_cache,
            client_uri: self.client_uri,
            start: self.start,
            started_at_ms: self.started_at_ms,
            request_id: self.request_id,
            request_body_len: self.request_body_len,
            body_for_upstream: self.body_for_upstream,
            request_model: self.request_model,
            session_binding: self.session_binding,
            session_override_config: self.session_override_config,
            global_station_override: self.global_station_override,
            override_model: self.override_model,
            override_effort: self.override_effort,
            override_service_tier: self.override_service_tier,
            effective_effort: self.effective_effort,
            effective_service_tier: self.effective_service_tier,
            base_service_tier: self.base_service_tier,
            session_id: self.session_id,
            session_identity_source: self.session_identity_source,
            cwd: self.cwd,
            request_flavor: self.request_flavor,
            request_body_previews: self.request_body_previews,
            debug_max: self.debug_max,
            warn_max: self.warn_max,
            client_body_debug: self.client_body_debug,
            client_body_warn: self.client_body_warn,
            plan: self.plan,
            route_graph_key: params.route_graph_key,
            upstream_opt: self.upstream_opt(),
            provider_opt: self.provider_opt(),
            allow_provider_failover: params.allow_provider_failover,
            provider_attempt: params.provider_attempt,
            total_upstreams: params.total_upstreams,
            cooldown_backoff: self.cooldown_backoff,
            global_attempt: params.global_attempt,
            avoid_set: params.avoid_set,
            avoided_total: params.avoided_total,
            last_err: params.last_err,
            upstream_chain: params.upstream_chain,
            route_attempts: params.route_attempts,
            concurrency_permit: params.concurrency_permit,
        })
        .await
    }
}

pub(super) fn apply_concurrency_snapshots_to_runtime(
    proxy: &ProxyService,
    template: &crate::routing_ir::RoutePlanTemplate,
    runtime: &mut RoutePlanRuntimeState,
) {
    for candidate in &template.candidates {
        let Some(limit) = candidate.concurrency.max_concurrent_requests else {
            continue;
        };
        if limit == 0 {
            continue;
        }
        let provider_endpoint = template.candidate_provider_endpoint_key(candidate);
        let Some(key) = candidate
            .concurrency
            .limit_key(proxy.service_name, &provider_endpoint)
        else {
            continue;
        };
        let snapshot = proxy.concurrency_limiter.snapshot(key.as_str(), limit);
        let mut state = runtime.provider_endpoint(&provider_endpoint);
        state.concurrency_saturated = snapshot.saturated;
        state.concurrency_active = Some(snapshot.active);
        state.concurrency_limit = Some(snapshot.limit);
        runtime.set_provider_endpoint(provider_endpoint, state);
    }
}

async fn refresh_route_graph_runtime_for_request(
    proxy: &ProxyService,
    template: &crate::routing_ir::RoutePlanTemplate,
    session_id: Option<&str>,
) -> RoutePlanRuntimeState {
    let mut runtime = proxy
        .state
        .route_plan_runtime_state_for_provider_endpoints(proxy.service_name)
        .await;
    apply_concurrency_snapshots_to_runtime(proxy, template, &mut runtime);
    apply_session_route_affinity_for_template(proxy, session_id, template, &mut runtime).await;
    runtime
}

pub(super) fn try_acquire_candidate_concurrency_permit(
    proxy: &ProxyService,
    template: &crate::routing_ir::RoutePlanTemplate,
    candidate: &RouteCandidate,
) -> Result<Option<super::concurrency_limits::ConcurrencyPermit>, ConcurrencyAcquireError> {
    let Some(limit) = candidate.concurrency.max_concurrent_requests else {
        return Ok(None);
    };
    if limit == 0 {
        return Ok(None);
    }
    let provider_endpoint = template.candidate_provider_endpoint_key(candidate);
    let Some(key) = candidate
        .concurrency
        .limit_key(proxy.service_name, &provider_endpoint)
    else {
        return Ok(None);
    };
    proxy.concurrency_limiter.try_acquire(key, limit).map(Some)
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

    let ctx = ProviderExecutionContext::from_params(&params);
    let provider_opt = ctx.provider_opt();
    match params.route_selection {
        RequestRouteSelection::RouteGraph { template } => {
            let executor = RoutePlanExecutor::new(template);
            let route_graph_key = template.route_graph_key();
            let total_upstreams = template.candidates.len();
            let mut runtime =
                refresh_route_graph_runtime_for_request(ctx.proxy, template, ctx.session_id).await;
            let mut route_state = RoutePlanAttemptState::default();
            let provider_chain_policy = ProviderChainAttemptPolicy::route_graph(
                ctx.request_flavor,
                Some(template.affinity_policy),
            );
            let mut upstream_chain: Vec<String> = Vec::new();
            let mut route_attempts: Vec<RouteAttemptLog> = Vec::new();
            let mut global_attempt: u32 = 0;
            let mut last_err: Option<(StatusCode, String)> = None;

            if provider_chain_policy.requires_known_affinity()
                && runtime.affinity_provider_endpoint().is_none()
            {
                let reason = provider_chain_policy.missing_affinity_trace_reason();
                last_err = Some((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "state-bound compact request requires existing route affinity".to_string(),
                ));
                log_retry_trace(serde_json::json!({
                    "event": "route_continuity_blocked",
                    "service": ctx.proxy.service_name,
                    "request_id": ctx.request_id,
                    "reason": reason,
                    "continuity_class": provider_chain_policy.continuity_class(),
                    "affinity_source": "none",
                    "provider_failover_allowed": provider_chain_policy.allow_provider_failover(),
                    "provider_failover_blocked_reason": reason,
                    "balance_signal_authoritative": false,
                }));
                return ProviderExecutionOutcome::Exhausted(ProviderExecutionState {
                    upstream_chain,
                    route_attempts,
                    last_err,
                });
            }

            let route_graph_loop = RouteGraphAttemptLoop {
                params: ExecuteRouteGraphExecutorParams {
                    ctx,
                    route_graph_key: Some(route_graph_key.as_str()),
                    provider_attempt: 0,
                    total_upstreams,
                    executor: &executor,
                    runtime: &mut runtime,
                    route_state: &mut route_state,
                    global_attempt: &mut global_attempt,
                    last_err: &mut last_err,
                    upstream_chain: &mut upstream_chain,
                    route_attempts: &mut route_attempts,
                    policy: provider_chain_policy,
                },
                compact_route_unavailable_waited: false,
            };
            if let Some(response) = route_graph_loop.run().await {
                return ProviderExecutionOutcome::Return(response);
            }

            ProviderExecutionOutcome::Exhausted(ProviderExecutionState {
                upstream_chain,
                route_attempts,
                last_err,
            })
        }
        RequestRouteSelection::Legacy { lbs } => {
            let legacy_template = compile_legacy_route_plan_template(
                ctx.proxy.service_name,
                lbs.iter().map(|lb| lb.service.as_ref()),
            );
            let executor = RoutePlanExecutor::new(&legacy_template);
            let route_graph_key = legacy_template.route_graph_key();
            let total_upstreams = lbs
                .iter()
                .map(|lb| lb.service.upstreams.len())
                .sum::<usize>();
            let upstream_overrides = ctx
                .proxy
                .state
                .get_upstream_meta_overrides(ctx.proxy.service_name)
                .await;
            let mut runtime = route_plan_runtime_state_from_lbs_with_overrides(
                ctx.proxy.service_name,
                lbs,
                &upstream_overrides,
            );
            apply_session_route_affinity_for_template(
                ctx.proxy,
                ctx.session_id,
                &legacy_template,
                &mut runtime,
            )
            .await;
            let mut route_state = RoutePlanAttemptState::default();
            let mut upstream_chain: Vec<String> = Vec::new();
            let mut route_attempts: Vec<RouteAttemptLog> = Vec::new();
            let strict_multi_config = lbs.len() > 1;
            let provider_chain_policy = ProviderChainAttemptPolicy::legacy(
                ctx.request_flavor,
                ctx.plan,
                provider_opt,
                strict_multi_config,
            );
            let mut global_attempt: u32 = 0;
            let mut last_err: Option<(StatusCode, String)> = None;
            let mut tried_stations: HashSet<String> = HashSet::new();

            if provider_chain_policy.requires_known_affinity()
                && runtime.affinity_provider_endpoint().is_none()
            {
                let reason = provider_chain_policy.missing_affinity_trace_reason();
                last_err = Some((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "state-bound compact request requires existing route affinity".to_string(),
                ));
                log_retry_trace(serde_json::json!({
                    "event": "route_continuity_blocked",
                    "service": ctx.proxy.service_name,
                    "request_id": ctx.request_id,
                    "reason": reason,
                    "continuity_class": provider_chain_policy.continuity_class(),
                    "affinity_source": "none",
                    "provider_failover_allowed": provider_chain_policy.allow_provider_failover(),
                    "provider_failover_blocked_reason": reason,
                    "balance_signal_authoritative": false,
                }));
                return ProviderExecutionOutcome::Exhausted(ProviderExecutionState {
                    upstream_chain,
                    route_attempts,
                    last_err,
                });
            }

            for provider_attempt in 0..provider_chain_policy.provider_attempt_limit {
                let Some(lb) = next_provider_load_balancer(lbs, &tried_stations) else {
                    break;
                };
                let station_name = lb.service.name.clone();

                let station_loop = LegacyAttemptLoop {
                    params: ExecuteRouteExecutorStationParams {
                        ctx,
                        lb: &lb,
                        station_name: station_name.as_str(),
                        route_graph_key: Some(route_graph_key.as_str()),
                        provider_attempt,
                        total_upstreams,
                        executor: &executor,
                        runtime: &runtime,
                        route_state: &mut route_state,
                        global_attempt: &mut global_attempt,
                        last_err: &mut last_err,
                        upstream_chain: &mut upstream_chain,
                        route_attempts: &mut route_attempts,
                        policy: provider_chain_policy,
                    },
                };
                match station_loop.run().await {
                    SelectedUpstreamExecutionOutcome::ContinueStation => {}
                    SelectedUpstreamExecutionOutcome::StopProviderChain => {
                        return ProviderExecutionOutcome::Exhausted(ProviderExecutionState {
                            upstream_chain,
                            route_attempts,
                            last_err,
                        });
                    }
                    SelectedUpstreamExecutionOutcome::Return(response) => {
                        return ProviderExecutionOutcome::Return(response);
                    }
                }

                tried_stations.insert(station_name.clone());

                log_cross_station_failover_blocked(CrossStationFailoverBlockedParams {
                    service_name: ctx.proxy.service_name,
                    request_id: ctx.request_id,
                    station_name: station_name.as_str(),
                    strict_multi_config: provider_chain_policy.strict_multi_config,
                    provider_attempt,
                    cross_station_failover_enabled: provider_chain_policy
                        .cross_station_failover_enabled,
                    provider_opt,
                    provider_attempt_limit: provider_chain_policy.provider_attempt_limit,
                    allow_cross_station_before_first_output: ctx
                        .plan
                        .allow_cross_station_before_first_output,
                });

                if provider_opt.base_backoff_ms > 0
                    && provider_attempt + 1 < provider_chain_policy.provider_attempt_limit
                {
                    backoff_sleep(provider_opt, provider_attempt).await;
                }
            }

            ProviderExecutionOutcome::Exhausted(ProviderExecutionState {
                upstream_chain,
                route_attempts,
                last_err,
            })
        }
    }
}

struct ExecuteRouteExecutorStationParams<'a, 'route> {
    ctx: ProviderExecutionContext<'a>,
    lb: &'a LoadBalancer,
    station_name: &'a str,
    route_graph_key: Option<&'a str>,
    provider_attempt: u32,
    total_upstreams: usize,
    executor: &'a RoutePlanExecutor<'route>,
    runtime: &'a RoutePlanRuntimeState,
    route_state: &'a mut RoutePlanAttemptState,
    global_attempt: &'a mut u32,
    last_err: &'a mut Option<(StatusCode, String)>,
    upstream_chain: &'a mut Vec<String>,
    route_attempts: &'a mut Vec<RouteAttemptLog>,
    policy: ProviderChainAttemptPolicy,
}

struct ExecuteRouteGraphExecutorParams<'a, 'route> {
    ctx: ProviderExecutionContext<'a>,
    route_graph_key: Option<&'a str>,
    provider_attempt: u32,
    total_upstreams: usize,
    executor: &'a RoutePlanExecutor<'route>,
    runtime: &'a mut RoutePlanRuntimeState,
    route_state: &'a mut RoutePlanAttemptState,
    global_attempt: &'a mut u32,
    last_err: &'a mut Option<(StatusCode, String)>,
    upstream_chain: &'a mut Vec<String>,
    route_attempts: &'a mut Vec<RouteAttemptLog>,
    policy: ProviderChainAttemptPolicy,
}

struct RouteGraphAttemptLoop<'a, 'route> {
    params: ExecuteRouteGraphExecutorParams<'a, 'route>,
    compact_route_unavailable_waited: bool,
}

struct LegacyAttemptLoop<'a, 'route> {
    params: ExecuteRouteExecutorStationParams<'a, 'route>,
}

impl<'a, 'route> RouteGraphAttemptLoop<'a, 'route> {
    async fn run(self) -> Option<Response<Body>> {
        let RouteGraphAttemptLoop {
            params,
            mut compact_route_unavailable_waited,
        } = self;
        let ExecuteRouteGraphExecutorParams {
            ctx,
            route_graph_key,
            provider_attempt,
            total_upstreams,
            executor,
            runtime,
            route_state,
            global_attempt,
            last_err,
            upstream_chain,
            route_attempts,
            policy,
        } = params;

        loop {
            let selection = if ctx.request_flavor.is_remote_compaction_request()
                && runtime.affinity_provider_endpoint().is_some()
            {
                let affinity_selection = executor.select_affinity_candidate_with_runtime_state(
                    route_state,
                    &*runtime,
                    ctx.request_model,
                );
                if affinity_selection.selected.is_some() || policy.compact.strict_affinity {
                    affinity_selection
                } else {
                    executor.select_supported_candidate_with_runtime_state(
                        route_state,
                        &*runtime,
                        ctx.request_model,
                    )
                }
            } else {
                executor.select_supported_candidate_with_runtime_state(
                    route_state,
                    &*runtime,
                    ctx.request_model,
                )
            };
            record_executor_unsupported_model_skips(
                ctx.proxy.service_name,
                upstream_chain,
                route_attempts,
                &selection.skipped,
                provider_attempt,
                ctx.plan.route.max_attempts,
            );

            let avoided_candidate_indices = selection.avoided_candidate_indices.clone();
            let mut avoided_total = selection.avoided_total;
            let Some(selected) = selection.selected else {
                if let Some(report) = route_unavailable_report(
                    ctx.proxy.service_name,
                    ctx.request_id,
                    executor,
                    &*runtime,
                    route_state,
                    ctx.request_model,
                ) {
                    if ctx.request_flavor.is_remote_compaction_request()
                        && !compact_route_unavailable_waited
                        && route_graph_key.is_some()
                        && let Some(wait_secs) =
                            report.short_cooldown_wait_secs(COMPACT_ROUTE_UNAVAILABLE_WAIT_MAX_SECS)
                    {
                        compact_route_unavailable_waited = true;
                        log_retry_trace(serde_json::json!({
                            "event": "compact_route_unavailable_wait",
                            "service": ctx.proxy.service_name,
                            "request_id": ctx.request_id,
                            "wait_secs": wait_secs,
                            "reason": "short_cooldown",
                        }));
                        tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
                        *runtime = refresh_route_graph_runtime_for_request(
                            ctx.proxy,
                            executor.template(),
                            ctx.session_id,
                        )
                        .await;
                        continue;
                    }
                    enqueue_usage_probes_for_provider_endpoints(
                        ctx.proxy,
                        report.provider_endpoints_to_probe.iter(),
                    )
                    .await;
                    route_attempts.extend(report.route_attempts.clone());
                    *last_err = Some(report.failure_status_message());
                }
                break;
            };
            log_route_graph_selection_explain(
                ctx.proxy.service_name,
                ctx.request_id,
                executor,
                &*runtime,
                route_state,
                ctx.request_model,
                &selected,
                policy,
            );
            let selected_candidate = selected.candidate;
            let mut avoid_set = hash_set_from_indices(&avoided_candidate_indices);

            let target = AttemptTarget::from_candidate(ctx.proxy.service_name, selected_candidate);
            let concurrency_permit = match try_acquire_candidate_concurrency_permit(
                ctx.proxy,
                executor.template(),
                selected_candidate,
            ) {
                Ok(permit) => permit,
                Err(ConcurrencyAcquireError::Saturated { active, limit }) => {
                    let provider_endpoint = executor
                        .template()
                        .candidate_provider_endpoint_key(selected_candidate);
                    route_state.avoid_provider_endpoint(provider_endpoint.clone());
                    log_retry_trace(serde_json::json!({
                        "event": "route_candidate_concurrency_saturated",
                        "service": ctx.proxy.service_name,
                        "request_id": ctx.request_id,
                        "provider_endpoint_key": provider_endpoint.stable_key(),
                        "active": active,
                        "limit": limit,
                    }));
                    continue;
                }
            };

            match ctx
                .execute_selected_attempt(SelectedAttemptExecutionParams {
                    legacy_lb: None,
                    target: &target,
                    route_graph_key,
                    allow_provider_failover: policy.allow_provider_failover(),
                    provider_attempt,
                    total_upstreams,
                    global_attempt,
                    avoid_set: &mut avoid_set,
                    avoided_total: &mut avoided_total,
                    last_err,
                    upstream_chain,
                    route_attempts,
                    concurrency_permit,
                })
                .await
            {
                SelectedUpstreamExecutionOutcome::ContinueStation => {}
                SelectedUpstreamExecutionOutcome::StopProviderChain => {
                    return None;
                }
                SelectedUpstreamExecutionOutcome::Return(response) => {
                    return Some(response);
                }
            }

            if avoid_set.contains(&selected_candidate.stable_index) {
                route_state.avoid_candidate(executor.template(), selected_candidate);
            }
            debug_assert_eq!(route_state.avoided_total(), avoided_total);
        }

        None
    }
}

impl<'a, 'route> LegacyAttemptLoop<'a, 'route> {
    async fn run(self) -> SelectedUpstreamExecutionOutcome {
        let LegacyAttemptLoop { params } = self;
        let ExecuteRouteExecutorStationParams {
            ctx,
            lb,
            station_name,
            route_graph_key,
            provider_attempt,
            total_upstreams,
            executor,
            runtime,
            route_state,
            global_attempt,
            last_err,
            upstream_chain,
            route_attempts,
            policy,
        } = params;

        'upstreams: loop {
            let upstream_total = lb.service.upstreams.len();
            let avoid_snapshot =
                hash_set_from_indices(&route_state.avoid_for_station_name(station_name));
            if station_upstreams_exhausted(upstream_total, &avoid_snapshot) {
                log_same_station_failover_trace(
                    ctx.proxy.service_name,
                    ctx.request_id,
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
                ctx.request_model,
            );
            record_executor_station_unsupported_model_skips(
                ctx.proxy.service_name,
                upstream_chain,
                route_attempts,
                &selection.skipped,
                provider_attempt,
                ctx.plan.route.max_attempts,
            );

            let avoid_for_station = selection.avoid_for_station.clone();
            let mut avoided_total = selection.avoided_total;
            let Some(selected) = selection.selected else {
                break 'upstreams;
            };
            let selected = selected.selected_upstream;
            let selected_station_name = selected.station_name.clone();
            let mut avoid_set = hash_set_from_indices(&avoid_for_station);

            let target = AttemptTarget::legacy(selected.clone());

            match ctx
                .execute_selected_attempt(SelectedAttemptExecutionParams {
                    legacy_lb: Some(lb),
                    target: &target,
                    route_graph_key,
                    allow_provider_failover: policy.allow_provider_failover(),
                    provider_attempt,
                    total_upstreams,
                    global_attempt,
                    avoid_set: &mut avoid_set,
                    avoided_total: &mut avoided_total,
                    last_err,
                    upstream_chain,
                    route_attempts,
                    concurrency_permit: None,
                })
                .await
            {
                SelectedUpstreamExecutionOutcome::ContinueStation => {}
                SelectedUpstreamExecutionOutcome::StopProviderChain => {
                    return SelectedUpstreamExecutionOutcome::StopProviderChain;
                }
                SelectedUpstreamExecutionOutcome::Return(response) => {
                    return SelectedUpstreamExecutionOutcome::Return(response);
                }
            }

            sync_route_state_from_avoid_set(
                route_state,
                selected_station_name.as_str(),
                &avoid_set,
            );
            debug_assert_eq!(route_state.avoided_total(), avoided_total);

            if station_loop_action_after_attempt(
                ctx.proxy.service_name,
                ctx.request_id,
                selected_station_name.as_str(),
                lb.service.upstreams.len(),
                &avoid_set,
            ) {
                break 'upstreams;
            }
        }

        SelectedUpstreamExecutionOutcome::ContinueStation
    }
}

fn log_route_graph_selection_explain(
    service_name: &str,
    request_id: u64,
    executor: &RoutePlanExecutor<'_>,
    runtime: &RoutePlanRuntimeState,
    route_state: &RoutePlanAttemptState,
    request_model: Option<&str>,
    selected: &SelectedRouteCandidate<'_>,
    policy: ProviderChainAttemptPolicy,
) {
    let selected_group = selected.candidate.preference_group;
    if selected_group == 0 {
        return;
    }

    let template = executor.template();
    let selected_provider_endpoint_key = selected.provider_endpoint.stable_key();
    let runtime_reason_map = executor
        .explain_candidate_skip_reasons_with_runtime_state(runtime, request_model)
        .into_iter()
        .map(|skip| {
            let reasons = skip
                .reasons
                .iter()
                .map(RoutePlanSkipReason::code)
                .collect::<Vec<_>>();
            (skip.provider_endpoint.stable_key(), reasons)
        })
        .collect::<BTreeMap<_, _>>();

    let mut skipped_groups = BTreeSet::new();
    let mut skipped_candidates = Vec::new();
    for candidate in executor.iter_candidates() {
        if candidate.preference_group >= selected_group {
            continue;
        }

        let provider_endpoint_key = template
            .candidate_provider_endpoint_key(candidate)
            .stable_key();
        let mut reasons = BTreeSet::new();
        if route_state.avoids_candidate(template, candidate) {
            reasons.insert("attempt_avoided");
        }
        if let Some(runtime_reasons) = runtime_reason_map.get(provider_endpoint_key.as_str()) {
            reasons.extend(runtime_reasons.iter().copied());
        }
        if reasons.is_empty() {
            reasons.insert("not_selected");
        }
        skipped_groups.insert(candidate.preference_group);
        skipped_candidates.push(serde_json::json!({
            "provider_id": candidate.provider_id.as_str(),
            "endpoint_id": candidate.endpoint_id.as_str(),
            "provider_endpoint_key": provider_endpoint_key,
            "preference_group": candidate.preference_group,
            "route_path": &candidate.route_path,
            "reasons": reasons.into_iter().collect::<Vec<_>>(),
        }));
    }

    if skipped_candidates.is_empty() {
        return;
    }

    let affinity_provider_endpoint_key = runtime
        .affinity_provider_endpoint()
        .map(|key| key.stable_key());
    let affinity_source = if affinity_provider_endpoint_key.is_some() {
        "session_route_affinity"
    } else {
        "none"
    };
    let selected_matches_affinity = affinity_provider_endpoint_key
        .as_deref()
        .is_some_and(|key| key == selected_provider_endpoint_key);

    log_retry_trace(serde_json::json!({
        "event": "route_graph_selection_explain",
        "service": service_name,
        "request_id": request_id,
        "request_model": request_model,
        "continuity": {
            "class": policy.continuity_class(),
            "provider_failover_allowed": policy.allow_provider_failover(),
            "provider_failover_blocked_reason": policy.provider_failover_blocked_reason(),
            "balance_signal_authoritative": false,
        },
        "affinity": {
            "policy": routing_affinity_policy_trace_label(template.affinity_policy),
            "provider_endpoint_key": affinity_provider_endpoint_key,
            "source": affinity_source,
            "selected_matches_affinity": selected_matches_affinity,
        },
        "selected": {
            "provider_id": selected.candidate.provider_id.as_str(),
            "endpoint_id": selected.candidate.endpoint_id.as_str(),
            "provider_endpoint_key": selected_provider_endpoint_key,
            "preference_group": selected_group,
            "route_path": &selected.candidate.route_path,
        },
        "skipped_higher_priority_groups": skipped_groups.into_iter().collect::<Vec<_>>(),
        "skipped_higher_priority_candidates": skipped_candidates,
    }));
}

fn routing_affinity_policy_trace_label(policy: RoutingAffinityPolicyV5) -> &'static str {
    match policy {
        RoutingAffinityPolicyV5::Off => "off",
        RoutingAffinityPolicyV5::PreferredGroup => "preferred_group",
        RoutingAffinityPolicyV5::FallbackSticky => "fallback_sticky",
        RoutingAffinityPolicyV5::Hard => "hard",
    }
}

async fn enqueue_usage_probes_for_provider_endpoints<'a>(
    proxy: &ProxyService,
    provider_endpoints: impl IntoIterator<Item = &'a ProviderEndpointKey>,
) {
    let provider_endpoints = provider_endpoints.into_iter().cloned().collect::<Vec<_>>();
    if provider_endpoints.is_empty() {
        return;
    }

    let cfg_snapshot = proxy.config.snapshot().await;
    for provider_endpoint in provider_endpoints {
        usage_providers::enqueue_poll_for_codex_provider_endpoint(
            proxy.client.clone(),
            cfg_snapshot.clone(),
            proxy.lb_states.clone(),
            proxy.state.clone(),
            proxy.service_name,
            provider_endpoint,
        );
    }
}

fn record_executor_unsupported_model_skips(
    service_name: &str,
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
        let avoid_set = hash_set_from_indices(&skipped.avoided_candidate_indices);
        let target = AttemptTarget::from_candidate(service_name, skipped.candidate);
        record_unsupported_model_skip(
            upstream_chain,
            route_attempts,
            UnsupportedModelSkipParams {
                target: &target,
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

fn record_executor_station_unsupported_model_skips(
    _service_name: &str,
    upstream_chain: &mut Vec<String>,
    route_attempts: &mut Vec<RouteAttemptLog>,
    skipped: &[SkippedStationRouteCandidate<'_>],
    provider_attempt: u32,
    provider_max_attempts: u32,
) {
    for skipped in skipped {
        let RoutePlanSkipReason::UnsupportedModel { requested_model } = &skipped.reason else {
            continue;
        };
        let avoid_set = hash_set_from_indices(&skipped.avoid_for_station);
        let target = AttemptTarget::legacy(skipped.selected_upstream.clone());
        record_unsupported_model_skip(
            upstream_chain,
            route_attempts,
            UnsupportedModelSkipParams {
                target: &target,
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::codex_integration::CodexPatchMode;

    fn test_request_flavor(
        is_remote_compaction_v1_request: bool,
        remote_compaction_requires_affinity: bool,
    ) -> RequestFlavor {
        RequestFlavor {
            client_content_type: None,
            is_stream: false,
            is_user_turn: false,
            is_remote_compaction_v1_request,
            is_remote_compaction_v2_request: false,
            remote_compaction_requires_affinity,
            is_codex_service: false,
            codex_client_patch_mode: CodexPatchMode::Default,
            codex_bridge_log: None,
        }
    }

    #[test]
    fn route_graph_policy_tracks_remote_compaction_affinity() {
        let policy = ProviderChainAttemptPolicy::route_graph(
            &test_request_flavor(true, true),
            Some(RoutingAffinityPolicyV5::Off),
        );
        assert!(!policy.allow_provider_failover());

        let relaxed_policy = ProviderChainAttemptPolicy::route_graph(
            &test_request_flavor(true, false),
            Some(RoutingAffinityPolicyV5::Off),
        );
        assert!(relaxed_policy.allow_provider_failover());
    }

    #[test]
    fn route_graph_policy_treats_remote_compaction_v2_as_state_bound() {
        let mut request_flavor = test_request_flavor(false, true);
        request_flavor.is_remote_compaction_v2_request = true;
        request_flavor.is_user_turn = true;

        let policy = ProviderChainAttemptPolicy::route_graph(
            &request_flavor,
            Some(RoutingAffinityPolicyV5::Off),
        );

        assert_eq!(policy.continuity_class(), "provider_state_bound");
        assert!(policy.requires_known_affinity());
        assert!(!policy.allow_provider_failover());
        assert_eq!(
            policy.provider_failover_blocked_reason(),
            Some("provider_state_bound")
        );
    }

    #[test]
    fn legacy_policy_limits_cross_station_failover_to_enabled_failover_profiles() {
        let request_flavor = test_request_flavor(false, false);
        let provider_opt = RetryLayerOptions {
            max_attempts: 4,
            base_backoff_ms: 0,
            max_backoff_ms: 0,
            jitter_ms: 0,
            retry_status_ranges: Vec::new(),
            retry_error_classes: Vec::new(),
            strategy: RetryStrategy::Failover,
        };
        let plan = RetryPlan {
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

        let policy =
            ProviderChainAttemptPolicy::legacy(&request_flavor, &plan, &provider_opt, true);
        assert!(policy.cross_station_failover_enabled);
        assert_eq!(policy.provider_attempt_limit, 4);
        assert!(policy.allow_provider_failover());

        let blocked_policy = ProviderChainAttemptPolicy::legacy(
            &request_flavor,
            &RetryPlan {
                allow_cross_station_before_first_output: false,
                ..plan
            },
            &provider_opt,
            true,
        );
        assert!(!blocked_policy.cross_station_failover_enabled);
        assert_eq!(blocked_policy.provider_attempt_limit, 1);
    }
}
