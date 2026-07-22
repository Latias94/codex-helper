use std::collections::{BTreeMap, BTreeSet, HashSet};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Response, StatusCode, Uri};

use crate::config::{RetryStrategy, RouteAffinityPolicy};
use crate::endpoint_health::CooldownBackoff;
use crate::logging::{
    BodyPreview, HeaderEntry, HttpDebugLog, RouteAttemptLog, ServiceTierLog,
    log_control_trace_event,
};
use crate::routing_ir::{
    RoutePlanAttemptState, RoutePlanExecutor, RoutePlanRuntimeState, RoutePlanSkipReason,
    SelectedRouteCandidate, SkippedRouteCandidate,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::runtime_store::ProviderPolicySnapshot;
use crate::state::{RuntimeHealthHalfOpenProbeLease, SessionBinding, SessionIdentitySource};

use super::ProxyService;
use super::attempt_execution::{
    ExecuteSelectedUpstreamParams, SelectedUpstreamExecutionOutcome, execute_selected_upstream,
};
use super::concurrency_limits::{ConcurrencyAcquireError, ConcurrencyPermit};
use super::request_body::{ReasoningOrchestrationIntent, RequestDialect};
use super::request_continuity::{RequestContinuityContract, RouteContinuityDecisionInput};
use super::request_preparation::RequestFlavor;
#[cfg(test)]
use super::request_preparation::SharedRouteStateImpact;
use super::response_semantics::ResponseSemanticContract;
use super::retry::{RetryLayerOptions, RetryPlan};
use super::route_affinity::{
    SessionRouteReservationDecision, apply_session_route_reservation_to_runtime,
    claim_session_route_reservation, lock_session_route_reservation_selection,
};
use super::route_attempts::{UnsupportedModelSkipParams, record_unsupported_model_skip};
use super::route_target_selection::{
    acquire_candidate_concurrency_permit, log_route_continuity_blocked,
    restrict_route_state_to_affinity_continuity_domain,
    route_graph_request_requires_existing_affinity, route_graph_runtime_for_request,
    runtime_for_acquired_candidate_revalidation, runtime_for_capacity_wait_selection,
    runtime_for_transient_half_open_selection, select_route_graph_candidate,
};
use super::route_unavailability::route_unavailable_report;
use super::runtime_config::CapturedRoutePlan;
use crate::routing_ir::CapturedRouteCandidate;

const COMPACT_ROUTE_UNAVAILABLE_WAIT_MAX_SECS: u64 = 10;
const DEGRADED_SELECTION_BALANCE_REPROBE_LIMIT: usize = 16;

#[derive(Clone, Copy)]
struct ProviderChainAttemptPolicy {
    continuity: RequestContinuityContract,
}

fn request_continuity_contract(
    request_flavor: &RequestFlavor,
    affinity_policy: Option<RouteAffinityPolicy>,
) -> RequestContinuityContract {
    RequestContinuityContract::from_route(RouteContinuityDecisionInput {
        is_remote_compaction_request: request_flavor.is_remote_compaction_request(),
        remote_compaction_requires_affinity: request_flavor.remote_compaction_requires_affinity,
        affinity_policy,
    })
}

impl ProviderChainAttemptPolicy {
    fn route_graph(
        request_flavor: &RequestFlavor,
        affinity_policy: Option<RouteAffinityPolicy>,
    ) -> Self {
        let continuity = request_continuity_contract(request_flavor, affinity_policy);
        Self { continuity }
    }

    fn allow_provider_failover(self) -> bool {
        self.continuity.allow_provider_failover()
    }

    fn allow_provider_failover_with_route_state(self, route_state: &RoutePlanAttemptState) -> bool {
        self.continuity
            .allow_provider_failover_with_explicit_domain(
                route_state.allows_explicit_continuity_domain_failover(),
            )
    }

    #[cfg(test)]
    fn requires_known_affinity(self) -> bool {
        self.continuity.requires_known_affinity()
    }

    fn continuity_class(self) -> &'static str {
        self.continuity.continuity_class()
    }

    fn provider_failover_blocked_reason(self) -> Option<&'static str> {
        self.continuity.provider_failover_blocked_reason()
    }
}

pub(super) struct ExecuteProviderChainParams<'a> {
    pub(super) proxy: &'a ProxyService,
    pub(super) route_plan: &'a CapturedRoutePlan,
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
    pub(super) request_dialect: RequestDialect,
    pub(super) translate_openai_models: bool,
    pub(super) request_model: Option<&'a str>,
    pub(super) session_binding: Option<&'a SessionBinding>,
    pub(super) effective_effort: Option<&'a str>,
    pub(super) deferred_reasoning_intent: Option<ReasoningOrchestrationIntent>,
    pub(super) effective_service_tier: Option<&'a str>,
    pub(super) base_service_tier: &'a ServiceTierLog,
    pub(super) session_id: Option<&'a str>,
    pub(super) session_identity_source: Option<SessionIdentitySource>,
    pub(super) cwd: Option<&'a str>,
    pub(super) request_flavor: &'a RequestFlavor,
    pub(super) request_body_previews: bool,
    pub(super) response_semantic_contract: Option<ResponseSemanticContract>,
    pub(super) debug_max: usize,
    pub(super) warn_max: usize,
    pub(super) client_body_debug: Option<&'a BodyPreview>,
    pub(super) client_body_warn: Option<&'a BodyPreview>,
    pub(super) plan: &'a RetryPlan,
    pub(super) cooldown_backoff: CooldownBackoff,
}

pub(super) enum ProviderExecutionOutcome {
    Return(Response<Body>),
    Exhausted(Box<ProviderExecutionState>),
}

pub(super) struct ProviderExecutionState {
    pub(super) route_attempts: Vec<RouteAttemptLog>,
    pub(super) last_err: Option<(StatusCode, String)>,
    pub(super) last_http_debug: Option<HttpDebugLog>,
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
    request_dialect: RequestDialect,
    translate_openai_models: bool,
    request_model: Option<&'a str>,
    session_binding: Option<&'a SessionBinding>,
    effective_effort: Option<&'a str>,
    deferred_reasoning_intent: Option<ReasoningOrchestrationIntent>,
    effective_service_tier: Option<&'a str>,
    base_service_tier: &'a ServiceTierLog,
    session_id: Option<&'a str>,
    session_identity_source: Option<SessionIdentitySource>,
    cwd: Option<&'a str>,
    request_flavor: &'a RequestFlavor,
    request_body_previews: bool,
    response_semantic_contract: Option<ResponseSemanticContract>,
    debug_max: usize,
    warn_max: usize,
    client_body_debug: Option<&'a BodyPreview>,
    client_body_warn: Option<&'a BodyPreview>,
    plan: &'a RetryPlan,
    cooldown_backoff: CooldownBackoff,
}

struct SelectedAttemptExecutionParams<'a> {
    target: &'a CapturedRouteCandidate,
    route_graph_key: Option<&'a str>,
    allow_provider_failover: bool,
    provider_attempt: u32,
    total_upstreams: usize,
    global_attempt: &'a mut u32,
    avoid_set: &'a mut HashSet<usize>,
    avoided_total: &'a mut usize,
    last_err: &'a mut Option<(StatusCode, String)>,
    last_http_debug: &'a mut Option<HttpDebugLog>,
    route_attempts: &'a mut Vec<RouteAttemptLog>,
    concurrency_permit: Option<ConcurrencyPermit>,
    half_open_probe: Option<RuntimeHealthHalfOpenProbeLease>,
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
            request_dialect: params.request_dialect,
            translate_openai_models: params.translate_openai_models,
            request_model: params.request_model,
            session_binding: params.session_binding,
            effective_effort: params.effective_effort,
            deferred_reasoning_intent: params.deferred_reasoning_intent,
            effective_service_tier: params.effective_service_tier,
            base_service_tier: params.base_service_tier,
            session_id: params.session_id,
            session_identity_source: params.session_identity_source,
            cwd: params.cwd,
            request_flavor: params.request_flavor,
            request_body_previews: params.request_body_previews,
            response_semantic_contract: params.response_semantic_contract,
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
        let mut half_open_upstream_opt = self.upstream_opt().clone();
        let mut half_open_provider_opt = self.provider_opt().clone();
        let is_half_open_probe = params.half_open_probe.is_some();
        if is_half_open_probe {
            half_open_upstream_opt.max_attempts = 1;
            half_open_provider_opt.max_attempts = 1;
        }
        let upstream_opt = if is_half_open_probe {
            &half_open_upstream_opt
        } else {
            self.upstream_opt()
        };
        let provider_opt = if is_half_open_probe {
            &half_open_provider_opt
        } else {
            self.provider_opt()
        };

        execute_selected_upstream(ExecuteSelectedUpstreamParams {
            proxy: self.proxy,
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
            request_dialect: self.request_dialect,
            translate_openai_models: self.translate_openai_models,
            request_model: self.request_model,
            session_binding: self.session_binding,
            effective_effort: self.effective_effort,
            deferred_reasoning_intent: self.deferred_reasoning_intent,
            effective_service_tier: self.effective_service_tier,
            base_service_tier: self.base_service_tier,
            session_id: self.session_id,
            session_identity_source: self.session_identity_source,
            cwd: self.cwd,
            request_flavor: self.request_flavor,
            request_body_previews: self.request_body_previews,
            response_semantic_contract: self.response_semantic_contract,
            debug_max: self.debug_max,
            warn_max: self.warn_max,
            client_body_debug: self.client_body_debug,
            client_body_warn: self.client_body_warn,
            plan: self.plan,
            route_graph_key: params.route_graph_key,
            upstream_opt,
            provider_opt,
            allow_provider_failover: params.allow_provider_failover && !is_half_open_probe,
            provider_attempt: params.provider_attempt,
            total_upstreams: params.total_upstreams,
            cooldown_backoff: self.cooldown_backoff,
            global_attempt: params.global_attempt,
            avoid_set: params.avoid_set,
            avoided_total: params.avoided_total,
            last_err: params.last_err,
            last_http_debug: params.last_http_debug,
            route_attempts: params.route_attempts,
            concurrency_permit: params.concurrency_permit,
            half_open_probe: params.half_open_probe,
        })
        .await
    }
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

    log_control_trace_event(serde_json::json!({
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
    }));
}

pub(super) async fn execute_provider_chain_with_route_executor(
    params: ExecuteProviderChainParams<'_>,
) -> ProviderExecutionOutcome {
    #[cfg(test)]
    ROUTE_EXECUTOR_REQUEST_PATH_TEST_INVOCATIONS.fetch_add(1, Ordering::SeqCst);

    let ctx = ProviderExecutionContext::from_params(&params);
    let route_plan = params.route_plan;
    let template = route_plan.template();
    let routing_control_graph_key = route_plan.routing_control_graph_key();
    let executor = RoutePlanExecutor::new(template);
    let route_graph_key = template.route_graph_key();
    let total_upstreams = template.candidates.len();
    let shared_route_updates_allowed = ctx
        .request_flavor
        .shared_route_state_impact
        .allows_shared_updates();
    let route_state_session_id = ctx.request_flavor.route_state_session_id(ctx.session_id);
    let mut runtime = match route_graph_runtime_for_request(
        ctx.proxy,
        template,
        routing_control_graph_key,
        route_plan.runtime_revision(),
        route_plan.provider_policy(),
        ctx.request_flavor.transient_health_capability(),
        route_state_session_id,
    )
    .await
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(
                service = ctx.proxy.service_name,
                request_id = ctx.request_id,
                error = %error,
                "captured runtime credential binding is invalid"
            );
            return ProviderExecutionOutcome::Exhausted(Box::new(ProviderExecutionState {
                route_attempts: Vec::new(),
                last_err: Some((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "configured upstream credentials are unavailable".to_string(),
                )),
                last_http_debug: None,
            }));
        }
    };
    let mut route_state = RoutePlanAttemptState::default();
    let provider_chain_policy =
        ProviderChainAttemptPolicy::route_graph(ctx.request_flavor, Some(template.affinity_policy));
    let mut route_attempts: Vec<RouteAttemptLog> = Vec::new();
    let mut global_attempt: u32 = 0;
    let mut last_err: Option<(StatusCode, String)> = None;
    let mut last_http_debug: Option<HttpDebugLog> = None;

    if route_graph_request_requires_existing_affinity(
        provider_chain_policy.continuity,
        &runtime,
        template,
    ) {
        last_err = Some((
            StatusCode::SERVICE_UNAVAILABLE,
            "state-bound compact request requires existing route affinity".to_string(),
        ));
        log_route_continuity_blocked(
            ctx.proxy.service_name,
            ctx.request_id,
            provider_chain_policy.continuity,
            None,
        );
        return ProviderExecutionOutcome::Exhausted(Box::new(ProviderExecutionState {
            route_attempts,
            last_err,
            last_http_debug,
        }));
    }
    restrict_route_state_to_affinity_continuity_domain(
        provider_chain_policy.continuity,
        &mut route_state,
        template,
        &runtime,
    );

    let route_graph_loop = RouteGraphAttemptLoop {
        params: ExecuteRouteGraphExecutorParams {
            ctx,
            route_graph_key: (template.affinity_policy != RouteAffinityPolicy::Off
                && shared_route_updates_allowed)
                .then_some(route_graph_key.as_str()),
            route_state_session_id,
            routing_control_graph_key,
            provider_attempt: 0,
            total_upstreams,
            executor: &executor,
            runtime_revision: route_plan.runtime_revision(),
            provider_policy: route_plan.provider_policy(),
            runtime: &mut runtime,
            route_state: &mut route_state,
            global_attempt: &mut global_attempt,
            last_err: &mut last_err,
            last_http_debug: &mut last_http_debug,
            route_attempts: &mut route_attempts,
            policy: provider_chain_policy,
        },
        compact_route_unavailable_waited: false,
    };
    if let Some(response) = route_graph_loop.run().await {
        return ProviderExecutionOutcome::Return(response);
    }

    ProviderExecutionOutcome::Exhausted(Box::new(ProviderExecutionState {
        route_attempts,
        last_err,
        last_http_debug,
    }))
}

struct ExecuteRouteGraphExecutorParams<'a, 'route> {
    ctx: ProviderExecutionContext<'a>,
    route_graph_key: Option<&'a str>,
    route_state_session_id: Option<&'a str>,
    routing_control_graph_key: &'a str,
    provider_attempt: u32,
    total_upstreams: usize,
    executor: &'a RoutePlanExecutor<'route>,
    runtime_revision: u64,
    provider_policy: &'a ProviderPolicySnapshot,
    runtime: &'a mut RoutePlanRuntimeState,
    route_state: &'a mut RoutePlanAttemptState,
    global_attempt: &'a mut u32,
    last_err: &'a mut Option<(StatusCode, String)>,
    last_http_debug: &'a mut Option<HttpDebugLog>,
    route_attempts: &'a mut Vec<RouteAttemptLog>,
    policy: ProviderChainAttemptPolicy,
}

struct RouteGraphAttemptLoop<'a, 'route> {
    params: ExecuteRouteGraphExecutorParams<'a, 'route>,
    compact_route_unavailable_waited: bool,
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
            route_state_session_id,
            routing_control_graph_key,
            provider_attempt,
            total_upstreams,
            executor,
            runtime_revision,
            provider_policy,
            runtime,
            route_state,
            global_attempt,
            last_err,
            last_http_debug,
            route_attempts,
            policy,
        } = params;
        let shared_route_updates_allowed = ctx
            .request_flavor
            .shared_route_state_impact
            .allows_shared_updates();

        loop {
            let revalidation_affinity_policy = if !shared_route_updates_allowed {
                RouteAffinityPolicy::Off
            } else if executor.template().affinity_policy == RouteAffinityPolicy::Hard
                && (!policy.continuity.is_provider_state_bound()
                    || route_state.allows_explicit_continuity_domain_failover())
            {
                RouteAffinityPolicy::FallbackSticky
            } else {
                executor.template().affinity_policy
            };
            let reservation_selection_guard = if revalidation_affinity_policy
                != RouteAffinityPolicy::Off
            {
                lock_session_route_reservation_selection(ctx.proxy, route_state_session_id).await
            } else {
                None
            };
            let reservation_decision = if reservation_selection_guard.is_some() {
                apply_session_route_reservation_to_runtime(
                    ctx.proxy,
                    ctx.request_id,
                    route_state_session_id,
                    executor.template(),
                    route_graph_key,
                    runtime,
                )
                .await
            } else {
                SessionRouteReservationDecision::None
            };
            match reservation_decision {
                SessionRouteReservationDecision::Busy => {
                    *last_err = Some((
                        StatusCode::TOO_MANY_REQUESTS,
                        "another active request is establishing this session's route affinity"
                            .to_string(),
                    ));
                    break;
                }
                SessionRouteReservationDecision::Failed => {
                    *last_err = Some((
                        StatusCode::SERVICE_UNAVAILABLE,
                        "session route reservation state is unavailable".to_string(),
                    ));
                    break;
                }
                SessionRouteReservationDecision::None
                | SessionRouteReservationDecision::Available(_) => {}
            }
            let selection_runtime =
                runtime_for_capacity_wait_selection(executor.template(), runtime);
            let mut selection = select_route_graph_candidate(
                executor,
                route_state,
                &selection_runtime,
                ctx.request_model,
                ctx.request_flavor.is_remote_compaction_request(),
                policy.continuity,
            );
            record_executor_unsupported_model_skips(
                executor,
                route_attempts,
                &selection.skipped,
                provider_attempt,
                ctx.plan.route.max_attempts,
            );

            let compact_short_cooldown_wait_secs = if selection.selected.is_none()
                && ctx.request_flavor.is_remote_compaction_request()
                && !compact_route_unavailable_waited
                && route_graph_key.is_some()
            {
                route_unavailable_report(
                    ctx.proxy.service_name,
                    ctx.request_id,
                    executor,
                    &*runtime,
                    route_state,
                    ctx.request_model,
                )
                .and_then(|report| {
                    report.short_cooldown_wait_secs(COMPACT_ROUTE_UNAVAILABLE_WAIT_MAX_SECS)
                })
            } else {
                None
            };

            let mut half_open_probe = None;
            if selection.selected.is_none()
                && compact_short_cooldown_wait_secs.is_none()
                && shared_route_updates_allowed
                && !ctx.request_flavor.is_remote_compaction_v2_request
                && let Some(capability) = ctx.request_flavor.transient_health_capability()
                && let Ok(identities) = executor.template().candidate_identities()
            {
                let eligible_provider_endpoints = ctx
                    .proxy
                    .state
                    .half_open_probe_eligible_provider_endpoints(
                        ctx.proxy.service_name,
                        identities.as_slice(),
                        capability,
                    )
                    .await;
                if !eligible_provider_endpoints.is_empty() {
                    let half_open_runtime = runtime_for_transient_half_open_selection(
                        executor.template(),
                        &selection_runtime,
                        &eligible_provider_endpoints,
                    );
                    let mut half_open_route_state = route_state.clone();
                    let half_open_selection = select_route_graph_candidate(
                        executor,
                        &mut half_open_route_state,
                        &half_open_runtime,
                        ctx.request_model,
                        ctx.request_flavor.is_remote_compaction_request(),
                        policy.continuity,
                    );
                    if let Some(selected) = half_open_selection.selected.as_ref()
                        && let Ok(identity) =
                            executor.template().candidate_identity(selected.candidate)
                        && let Some(probe) = ctx
                            .proxy
                            .state
                            .try_acquire_runtime_half_open_probe(
                                ctx.proxy.service_name,
                                &identity,
                                capability,
                            )
                            .await
                    {
                        log_control_trace_event(serde_json::json!({
                            "event": "route_transient_half_open_acquired",
                            "service": ctx.proxy.service_name,
                            "request_id": ctx.request_id,
                            "provider_endpoint_key": selected.provider_endpoint.stable_key(),
                        }));
                        selection = half_open_selection;
                        half_open_probe = Some(probe);
                    }
                }
            }

            let avoided_candidate_indices = selection.avoided_candidate_indices.clone();
            let mut avoided_total = selection.avoided_total;
            let Some(selected) = selection.selected else {
                drop(reservation_selection_guard);
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
                        && let Some(wait_secs) = compact_short_cooldown_wait_secs
                    {
                        compact_route_unavailable_waited = true;
                        log_control_trace_event(serde_json::json!({
                            "event": "compact_route_unavailable_wait",
                            "service": ctx.proxy.service_name,
                            "request_id": ctx.request_id,
                            "wait_secs": wait_secs,
                            "reason": "short_cooldown",
                        }));
                        tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
                        let refreshed_runtime = route_graph_runtime_for_request(
                            ctx.proxy,
                            executor.template(),
                            routing_control_graph_key,
                            runtime_revision,
                            provider_policy,
                            ctx.request_flavor.transient_health_capability(),
                            route_state_session_id,
                        )
                        .await;
                        match refreshed_runtime {
                            Ok(refreshed_runtime) => *runtime = refreshed_runtime,
                            Err(error) => {
                                tracing::error!(
                                    service = ctx.proxy.service_name,
                                    request_id = ctx.request_id,
                                    error = %error,
                                    "captured runtime credential binding became invalid"
                                );
                                *last_err = Some((
                                    StatusCode::SERVICE_UNAVAILABLE,
                                    "configured upstream credentials are unavailable".to_string(),
                                ));
                                break;
                            }
                        }
                        continue;
                    }
                    if shared_route_updates_allowed {
                        enqueue_usage_probes_for_provider_endpoints(
                            ctx.proxy,
                            executor.template(),
                            report.provider_endpoints_to_probe.iter(),
                        )
                        .await;
                    }
                    route_attempts.extend(report.route_attempts.clone());
                    *last_err = Some(report.failure_status_message());
                }
                break;
            };
            let selected_candidate = selected.candidate;
            let target = match executor.template().capture_candidate(selected_candidate) {
                Ok(target) => target,
                Err(error) => {
                    log_control_trace_event(serde_json::json!({
                        "event": "route_candidate_credential_binding_failed",
                        "service": ctx.proxy.service_name,
                        "request_id": ctx.request_id,
                        "provider_endpoint_key": selected.provider_endpoint.stable_key(),
                        "error": error.to_string(),
                    }));
                    *last_err = Some((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "captured runtime route has no matching credential binding".to_string(),
                    ));
                    break;
                }
            };
            let claimed = if reservation_selection_guard.is_some() {
                match claim_session_route_reservation(
                    ctx.proxy,
                    ctx.request_id,
                    route_state_session_id,
                    ctx.session_identity_source,
                    route_graph_key,
                    &target,
                )
                .await
                {
                    SessionRouteReservationDecision::Available(claimed) => Some(claimed),
                    SessionRouteReservationDecision::None => None,
                    SessionRouteReservationDecision::Busy => {
                        *last_err = Some((
                            StatusCode::TOO_MANY_REQUESTS,
                            "another active request is establishing this session's route affinity"
                                .to_string(),
                        ));
                        break;
                    }
                    SessionRouteReservationDecision::Failed => {
                        *last_err = Some((
                            StatusCode::SERVICE_UNAVAILABLE,
                            "session route reservation state is unavailable".to_string(),
                        ));
                        break;
                    }
                }
            } else {
                None
            };
            if let Some(claimed) = claimed
                && claimed.provider_endpoint != selected.provider_endpoint
                && let Some(claimed_candidate) = executor
                    .template()
                    .continuity_topology()
                    .find_candidate_by_provider_endpoint(&claimed.provider_endpoint)
                && (revalidation_affinity_policy != RouteAffinityPolicy::PreferredGroup
                    || claimed_candidate.preference_group == selected_candidate.preference_group)
                && executor.candidate_is_valid_after_runtime_update(
                    route_state,
                    &selection_runtime,
                    claimed_candidate,
                    ctx.request_model,
                    revalidation_affinity_policy,
                )
            {
                drop(reservation_selection_guard);
                runtime.set_affinity_provider_endpoint_with_observed_at(
                    Some(claimed.provider_endpoint.clone()),
                    Some(claimed.last_selected_at_ms),
                    Some(claimed.last_changed_at_ms),
                );
                log_control_trace_event(serde_json::json!({
                    "event": "route_candidate_reconciled_to_session_claim",
                    "service": ctx.proxy.service_name,
                    "request_id": ctx.request_id,
                    "provider_endpoint_key": claimed.provider_endpoint.stable_key(),
                }));
                continue;
            }
            drop(reservation_selection_guard);
            log_route_graph_selection_explain(RouteGraphSelectionExplain {
                service_name: ctx.proxy.service_name,
                request_id: ctx.request_id,
                executor,
                runtime: &*runtime,
                route_state,
                request_model: ctx.request_model,
                selected: &selected,
                policy,
            });
            let balance_probe_targets = degraded_selection_balance_probe_targets(
                executor,
                &*runtime,
                ctx.request_model,
                &selected,
            );
            if !balance_probe_targets.is_empty() && shared_route_updates_allowed {
                log_degraded_selection_balance_reprobe(
                    ctx.proxy.service_name,
                    ctx.request_id,
                    &selected,
                    &balance_probe_targets,
                );
                enqueue_usage_probes_for_provider_endpoints(
                    ctx.proxy,
                    executor.template(),
                    balance_probe_targets.iter(),
                )
                .await;
            }
            let mut avoid_set = hash_set_from_indices(&avoided_candidate_indices);
            let concurrency_permit = match acquire_candidate_concurrency_permit(
                ctx.proxy,
                executor.template(),
                selected_candidate,
                runtime_revision,
                route_state_session_id,
            )
            .await
            {
                Ok(permit) => permit,
                Err(ConcurrencyAcquireError::SessionAlreadyQueued { session_id }) => {
                    let provider_endpoint = executor
                        .template()
                        .candidate_provider_endpoint_key(selected_candidate);
                    let message = format!(
                        "session `{session_id}` already has a pending request for this provider capacity group"
                    );
                    *last_err = Some((StatusCode::TOO_MANY_REQUESTS, message.clone()));
                    log_control_trace_event(serde_json::json!({
                        "event": "route_candidate_session_queue_rejected",
                        "service": ctx.proxy.service_name,
                        "request_id": ctx.request_id,
                        "provider_endpoint_key": provider_endpoint.stable_key(),
                        "reason": "session_already_queued",
                    }));
                    break;
                }
                Err(error) => {
                    let provider_endpoint = executor
                        .template()
                        .candidate_provider_endpoint_key(selected_candidate);
                    route_state.avoid_provider_endpoint(provider_endpoint.clone());
                    log_control_trace_event(serde_json::json!({
                        "event": "route_candidate_concurrency_admission_failed",
                        "service": ctx.proxy.service_name,
                        "request_id": ctx.request_id,
                        "provider_endpoint_key": provider_endpoint.stable_key(),
                        "error": format!("{error:?}"),
                    }));
                    continue;
                }
            };

            if concurrency_permit.is_some() || half_open_probe.is_some() {
                let selected_provider_endpoint = executor
                    .template()
                    .candidate_provider_endpoint_key(selected_candidate);
                let refreshed_runtime = route_graph_runtime_for_request(
                    ctx.proxy,
                    executor.template(),
                    routing_control_graph_key,
                    runtime_revision,
                    provider_policy,
                    ctx.request_flavor.transient_health_capability(),
                    route_state_session_id,
                )
                .await;
                match refreshed_runtime {
                    Ok(refreshed_runtime) => *runtime = refreshed_runtime,
                    Err(error) => {
                        tracing::error!(
                            service = ctx.proxy.service_name,
                            request_id = ctx.request_id,
                            error = %error,
                            "captured runtime credential binding became invalid"
                        );
                        *last_err = Some((
                            StatusCode::SERVICE_UNAVAILABLE,
                            "configured upstream credentials are unavailable".to_string(),
                        ));
                        break;
                    }
                }
                if let Some(probe) = half_open_probe.as_ref()
                    && !ctx
                        .proxy
                        .state
                        .validate_runtime_half_open_probe(probe)
                        .await
                {
                    log_control_trace_event(serde_json::json!({
                        "event": "route_transient_half_open_revalidation_failed",
                        "service": ctx.proxy.service_name,
                        "request_id": ctx.request_id,
                        "provider_endpoint_key": selected_provider_endpoint.stable_key(),
                    }));
                    drop(concurrency_permit);
                    continue;
                }
                let mut validation_runtime = runtime_for_acquired_candidate_revalidation(
                    executor.template(),
                    runtime,
                    selected_candidate,
                );
                if half_open_probe.is_some() {
                    validation_runtime = runtime_for_transient_half_open_selection(
                        executor.template(),
                        &validation_runtime,
                        &HashSet::from([selected_provider_endpoint.clone()]),
                    );
                }
                if !executor.candidate_is_valid_after_runtime_update(
                    route_state,
                    &validation_runtime,
                    selected_candidate,
                    ctx.request_model,
                    revalidation_affinity_policy,
                ) {
                    log_control_trace_event(serde_json::json!({
                        "event": "route_candidate_changed_after_concurrency_wait",
                        "service": ctx.proxy.service_name,
                        "request_id": ctx.request_id,
                        "provider_endpoint_key": selected_provider_endpoint.stable_key(),
                    }));
                    drop(concurrency_permit);
                    continue;
                }
            }

            match ctx
                .execute_selected_attempt(SelectedAttemptExecutionParams {
                    target: &target,
                    route_graph_key,
                    allow_provider_failover: policy
                        .allow_provider_failover_with_route_state(route_state),
                    provider_attempt,
                    total_upstreams,
                    global_attempt,
                    avoid_set: &mut avoid_set,
                    avoided_total: &mut avoided_total,
                    last_err,
                    last_http_debug,
                    route_attempts,
                    concurrency_permit,
                    half_open_probe,
                })
                .await
            {
                SelectedUpstreamExecutionOutcome::ContinueProviderChain => {}
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
            if policy
                .continuity
                .should_restrict_to_affinity_continuity_domain()
            {
                route_state.restrict_to_continuity_domain(target.continuity_domain().clone());
            }
            debug_assert_eq!(route_state.avoided_total(), avoided_total);
        }

        None
    }
}

struct RouteGraphSelectionExplain<'a> {
    service_name: &'a str,
    request_id: u64,
    executor: &'a RoutePlanExecutor<'a>,
    runtime: &'a RoutePlanRuntimeState,
    route_state: &'a RoutePlanAttemptState,
    request_model: Option<&'a str>,
    selected: &'a SelectedRouteCandidate<'a>,
    policy: ProviderChainAttemptPolicy,
}

fn log_route_graph_selection_explain(args: RouteGraphSelectionExplain<'_>) {
    let RouteGraphSelectionExplain {
        service_name,
        request_id,
        executor,
        runtime,
        route_state,
        request_model,
        selected,
        policy,
    } = args;
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

    log_control_trace_event(serde_json::json!({
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

fn routing_affinity_policy_trace_label(policy: RouteAffinityPolicy) -> &'static str {
    match policy {
        RouteAffinityPolicy::Off => "off",
        RouteAffinityPolicy::PreferredGroup => "preferred_group",
        RouteAffinityPolicy::FallbackSticky => "fallback_sticky",
        RouteAffinityPolicy::Hard => "hard",
    }
}

fn degraded_selection_balance_probe_targets(
    executor: &RoutePlanExecutor<'_>,
    runtime: &RoutePlanRuntimeState,
    request_model: Option<&str>,
    selected: &SelectedRouteCandidate<'_>,
) -> Vec<ProviderEndpointKey> {
    let selected_group = selected.candidate.preference_group;
    if selected_group == 0 {
        return Vec::new();
    }

    let template = executor.template();
    let runtime_reason_map = executor
        .explain_candidate_skip_reasons_with_runtime_state(runtime, request_model)
        .into_iter()
        .map(|skip| (skip.provider_endpoint, skip.reasons))
        .collect::<BTreeMap<_, _>>();

    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();
    for candidate in executor.iter_candidates() {
        if candidate.preference_group >= selected_group {
            continue;
        }

        let provider_endpoint = template.candidate_provider_endpoint_key(candidate);
        if provider_endpoint == selected.provider_endpoint {
            continue;
        }

        let Some(reasons) = runtime_reason_map.get(&provider_endpoint) else {
            continue;
        };
        if !runtime_skip_reasons_warrant_balance_reprobe(reasons) {
            continue;
        }

        if seen.insert(provider_endpoint.clone()) {
            targets.push(provider_endpoint);
            if targets.len() >= DEGRADED_SELECTION_BALANCE_REPROBE_LIMIT {
                break;
            }
        }
    }

    targets
}

fn runtime_skip_reasons_warrant_balance_reprobe(reasons: &[RoutePlanSkipReason]) -> bool {
    let has_balance_recoverable_reason = reasons.iter().any(|reason| {
        matches!(
            reason,
            RoutePlanSkipReason::Cooldown | RoutePlanSkipReason::UsageExhausted
        )
    });
    let has_non_balance_blocker = reasons.iter().any(|reason| {
        matches!(
            reason,
            RoutePlanSkipReason::UnsupportedModel { .. }
                | RoutePlanSkipReason::RuntimeDisabled
                | RoutePlanSkipReason::MissingAuth
                | RoutePlanSkipReason::ConcurrencySaturated { .. }
        )
    });

    has_balance_recoverable_reason && !has_non_balance_blocker
}

fn log_degraded_selection_balance_reprobe(
    service_name: &str,
    request_id: u64,
    selected: &SelectedRouteCandidate<'_>,
    provider_endpoints: &[ProviderEndpointKey],
) {
    log_control_trace_event(serde_json::json!({
        "event": "route_graph_degraded_balance_reprobe_queued",
        "service": service_name,
        "request_id": request_id,
        "selected": {
            "provider_id": selected.candidate.provider_id.as_str(),
            "endpoint_id": selected.candidate.endpoint_id.as_str(),
            "provider_endpoint_key": selected.provider_endpoint.stable_key(),
            "preference_group": selected.candidate.preference_group,
            "route_path": &selected.candidate.route_path,
        },
        "probe_provider_endpoints": provider_endpoints
            .iter()
            .map(ProviderEndpointKey::stable_key)
            .collect::<Vec<_>>(),
    }));
}

async fn enqueue_usage_probes_for_provider_endpoints<'a>(
    proxy: &ProxyService,
    template: &crate::routing_ir::RoutePlanTemplate,
    provider_endpoints: impl IntoIterator<Item = &'a ProviderEndpointKey>,
) {
    let provider_endpoints = provider_endpoints.into_iter().cloned().collect::<Vec<_>>();
    if provider_endpoints.is_empty() {
        return;
    }

    let topology = template.continuity_topology();
    let provider_catalog = proxy.config.capture().await.usage_provider_catalog();
    for provider_endpoint in provider_endpoints {
        let Some(candidate) = topology.find_candidate_by_provider_endpoint(&provider_endpoint)
        else {
            continue;
        };
        let Ok(target) = template.capture_candidate(candidate) else {
            continue;
        };
        super::providers_api::enqueue_provider_balance_probe(
            proxy.client.clone(),
            proxy.state.clone(),
            target,
            Arc::clone(&provider_catalog),
        );
    }
}

fn record_executor_unsupported_model_skips(
    executor: &RoutePlanExecutor<'_>,
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
        let Ok(target) = executor.template().capture_candidate(skipped.candidate) else {
            continue;
        };
        record_unsupported_model_skip(
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

    use std::collections::BTreeMap;

    use crate::config::UpstreamAuth;
    use crate::routing_ir::{
        RouteCandidate, RouteCandidateConcurrency, RoutePlanTemplate, RoutePlanUpstreamRuntimeState,
    };

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
            remote_v2_downgrade_enabled: false,
            remote_compaction_requires_affinity,
            is_codex_service: false,
            shared_route_state_impact: SharedRouteStateImpact::RouteFacing,
            terminal_accounting: crate::runtime_store::RequestAccountingScope::Economic,
            route_capability: if is_remote_compaction_v1_request {
                crate::endpoint_health::RouteCapability::RemoteCompaction
            } else {
                crate::endpoint_health::RouteCapability::Inference
            },
            stream_terminal_policy:
                crate::proxy::request_preparation::StreamTerminalPolicy::ProtocolEvent,
            replay_policy: crate::proxy::request_preparation::RequestReplayPolicy::RouteFacing,
            codex_bridge_log: None,
        }
    }

    fn test_route_candidate(provider_id: &str, preference_group: u32) -> RouteCandidate {
        RouteCandidate {
            provider_id: provider_id.to_string(),
            provider_alias: None,
            endpoint_id: "default".to_string(),
            base_url: format!("https://{provider_id}.example/v1"),
            continuity_domain: None,
            auth: UpstreamAuth::default(),
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            model_rules: std::sync::Arc::default(),
            route_path: vec!["monthly_first".to_string(), provider_id.to_string()],
            preference_group,
            stable_index: preference_group as usize,
            concurrency: RouteCandidateConcurrency::default(),
        }
    }

    fn test_route_template(groups: &[&str]) -> RoutePlanTemplate {
        RoutePlanTemplate {
            service_name: "codex".to_string(),
            entry: "monthly_first".to_string(),
            affinity_policy: RouteAffinityPolicy::PreferredGroup,
            scheduling_preset: crate::config::SchedulingPreset::Balanced,
            fallback_ttl_ms: None,
            reprobe_preferred_after_ms: None,
            nodes: BTreeMap::new(),
            expanded_provider_order: groups.iter().map(|provider| provider.to_string()).collect(),
            candidates: groups
                .iter()
                .enumerate()
                .map(|(idx, provider)| test_route_candidate(provider, idx as u32))
                .collect(),
            credential_generation: crate::credentials::CredentialGeneration::empty(),
        }
    }

    fn selected_route_candidate<'a>(
        template: &'a RoutePlanTemplate,
        index: usize,
    ) -> SelectedRouteCandidate<'a> {
        let candidate = &template.candidates[index];
        SelectedRouteCandidate {
            candidate,
            provider_endpoint: template.candidate_provider_endpoint_key(candidate),
        }
    }

    fn stable_keys(keys: Vec<ProviderEndpointKey>) -> Vec<String> {
        keys.into_iter().map(|key| key.stable_key()).collect()
    }

    #[test]
    fn route_graph_policy_tracks_remote_compaction_affinity() {
        let policy = ProviderChainAttemptPolicy::route_graph(
            &test_request_flavor(true, true),
            Some(RouteAffinityPolicy::Off),
        );
        assert!(policy.allow_provider_failover());

        let relaxed_policy = ProviderChainAttemptPolicy::route_graph(
            &test_request_flavor(true, false),
            Some(RouteAffinityPolicy::Off),
        );
        assert!(relaxed_policy.allow_provider_failover());

        let hard_policy = ProviderChainAttemptPolicy::route_graph(
            &test_request_flavor(true, true),
            Some(RouteAffinityPolicy::Hard),
        );
        assert!(hard_policy.requires_known_affinity());
        assert!(!hard_policy.allow_provider_failover());
    }

    #[test]
    fn route_graph_policy_treats_remote_compaction_v2_as_tryable_state_bound_by_default() {
        let mut request_flavor = test_request_flavor(false, true);
        request_flavor.is_remote_compaction_v2_request = true;
        request_flavor.is_user_turn = true;

        let policy = ProviderChainAttemptPolicy::route_graph(
            &request_flavor,
            Some(RouteAffinityPolicy::FallbackSticky),
        );

        assert_eq!(policy.continuity_class(), "provider_state_bound");
        assert!(!policy.requires_known_affinity());
        assert!(policy.allow_provider_failover());
        assert_eq!(policy.provider_failover_blocked_reason(), None);
    }

    #[test]
    fn half_open_selection_clears_only_transient_breaker_fields() {
        let template = test_route_template(&["relay"]);
        let endpoint = template.candidate_provider_endpoint_key(&template.candidates[0]);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            endpoint.clone(),
            RoutePlanUpstreamRuntimeState {
                runtime_disabled: true,
                failure_count: crate::endpoint_health::FAILURE_THRESHOLD,
                cooldown_active: true,
                cooldown_remaining_secs: Some(30),
                usage_exhausted: true,
                ..RoutePlanUpstreamRuntimeState::default()
            },
        );

        let half_open_runtime = runtime_for_transient_half_open_selection(
            &template,
            &runtime,
            &HashSet::from([endpoint.clone()]),
        );
        let projected = half_open_runtime.provider_endpoint(&endpoint);
        assert_eq!(projected.failure_count, 0);
        assert!(!projected.cooldown_active);
        assert_eq!(projected.cooldown_remaining_secs, None);
        assert!(projected.runtime_disabled);
        assert!(projected.usage_exhausted);

        let executor = RoutePlanExecutor::new(&template);
        let selection = executor.select_supported_candidate_with_runtime_state(
            &mut RoutePlanAttemptState::default(),
            &half_open_runtime,
            None,
        );
        assert!(selection.selected.is_none());
    }

    #[test]
    fn degraded_selection_balance_reprobe_targets_runtime_recoverable_skips() {
        let template = test_route_template(&["input", "input1", "input2", "input3"]);
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            template.candidate_provider_endpoint_key(&template.candidates[0]),
            RoutePlanUpstreamRuntimeState {
                cooldown_active: true,
                cooldown_remaining_secs: Some(30),
                ..Default::default()
            },
        );
        runtime.set_provider_endpoint(
            template.candidate_provider_endpoint_key(&template.candidates[1]),
            RoutePlanUpstreamRuntimeState {
                usage_exhausted: true,
                ..Default::default()
            },
        );
        runtime.set_provider_endpoint(
            template.candidate_provider_endpoint_key(&template.candidates[2]),
            RoutePlanUpstreamRuntimeState {
                credential_readiness: crate::credentials::CredentialReadinessCode::Missing,
                ..Default::default()
            },
        );

        let targets = degraded_selection_balance_probe_targets(
            &executor,
            &runtime,
            None,
            &selected_route_candidate(&template, 3),
        );

        assert_eq!(
            stable_keys(targets),
            vec![
                "codex/input/default".to_string(),
                "codex/input1/default".to_string(),
            ]
        );
    }

    #[test]
    fn degraded_selection_balance_reprobe_ignores_best_group_selection() {
        let template = test_route_template(&["input", "input1"]);
        let executor = RoutePlanExecutor::new(&template);
        let mut runtime = RoutePlanRuntimeState::default();
        runtime.set_provider_endpoint(
            template.candidate_provider_endpoint_key(&template.candidates[1]),
            RoutePlanUpstreamRuntimeState {
                cooldown_active: true,
                cooldown_remaining_secs: Some(30),
                ..Default::default()
            },
        );

        let targets = degraded_selection_balance_probe_targets(
            &executor,
            &runtime,
            None,
            &selected_route_candidate(&template, 0),
        );

        assert!(targets.is_empty());
    }
}
