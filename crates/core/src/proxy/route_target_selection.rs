use axum::http::StatusCode;

use crate::logging::{RouteAttemptLog, log_retry_trace};
use crate::routing_ir::{
    RouteCandidate, RoutePlanAttemptSelection, RoutePlanAttemptState, RoutePlanExecutor,
    RoutePlanRuntimeState, RoutePlanTemplate,
};

use super::ProxyService;
use super::attempt_target::AttemptTarget;
use super::concurrency_limits::{ConcurrencyAcquireError, ConcurrencyPermit};
use super::request_continuity::RequestContinuityContract;
use super::route_affinity::apply_session_route_affinity_for_template;
use super::route_unavailability::route_unavailable_report;

pub(super) struct RouteGraphTargetSelection {
    pub(super) target: AttemptTarget,
    pub(super) avoided_candidate_indices: Vec<usize>,
    pub(super) avoided_total: usize,
    pub(super) total_upstreams: usize,
    pub(super) concurrency_permit: Option<ConcurrencyPermit>,
}

pub(super) struct RouteGraphSelectionFailure {
    pub(super) status: StatusCode,
    pub(super) message: String,
    pub(super) route_attempts: Vec<RouteAttemptLog>,
}

pub(super) struct WebSocketRouteGraphSelectionParams<'a, 'b> {
    pub(super) proxy: &'b ProxyService,
    pub(super) executor: &'a RoutePlanExecutor<'a>,
    pub(super) runtime: &'b RoutePlanRuntimeState,
    pub(super) route_state: &'b mut RoutePlanAttemptState,
    pub(super) request_id: u64,
    pub(super) request_model: Option<&'b str>,
    pub(super) request_is_remote_compaction: bool,
    pub(super) continuity_contract: RequestContinuityContract,
}

impl RouteGraphSelectionFailure {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            route_attempts: Vec::new(),
        }
    }
}

pub(super) async fn route_graph_runtime_for_request(
    proxy: &ProxyService,
    template: &RoutePlanTemplate,
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

pub(super) fn apply_concurrency_snapshots_to_runtime(
    proxy: &ProxyService,
    template: &RoutePlanTemplate,
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

pub(super) fn try_acquire_candidate_concurrency_permit(
    proxy: &ProxyService,
    template: &RoutePlanTemplate,
    candidate: &RouteCandidate,
) -> Result<Option<ConcurrencyPermit>, ConcurrencyAcquireError> {
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

pub(super) fn route_graph_request_requires_existing_affinity(
    contract: RequestContinuityContract,
    runtime: &RoutePlanRuntimeState,
    template: &RoutePlanTemplate,
) -> bool {
    contract.requires_existing_route_affinity(
        runtime.affinity_provider_endpoint().is_some(),
        template
            .continuity_topology()
            .configured_provider_endpoint_count(),
    )
}

pub(super) fn select_route_graph_candidate<'a>(
    executor: &'a RoutePlanExecutor<'a>,
    route_state: &mut RoutePlanAttemptState,
    runtime: &RoutePlanRuntimeState,
    request_model: Option<&str>,
    request_is_remote_compaction: bool,
    continuity_contract: RequestContinuityContract,
) -> RoutePlanAttemptSelection<'a> {
    let explicit_domain_failover_allowed = continuity_contract
        .allow_provider_failover_with_explicit_domain(
            route_state.allows_explicit_continuity_domain_failover(),
        );
    if request_is_remote_compaction && runtime.affinity_provider_endpoint().is_some() {
        let affinity_selection = executor.select_affinity_candidate_with_runtime_state(
            route_state,
            runtime,
            request_model,
        );
        if affinity_selection.selected.is_some()
            || (continuity_contract.should_restrict_to_affinity_continuity_domain()
                && !explicit_domain_failover_allowed)
        {
            affinity_selection
        } else if continuity_contract.should_restrict_to_affinity_continuity_domain()
            && explicit_domain_failover_allowed
        {
            executor.select_supported_candidate_with_soft_affinity_runtime_state(
                route_state,
                runtime,
                request_model,
            )
        } else if continuity_contract.is_provider_state_bound() {
            executor.select_supported_candidate_with_runtime_state(
                route_state,
                runtime,
                request_model,
            )
        } else {
            executor.select_supported_candidate_with_soft_affinity_runtime_state(
                route_state,
                runtime,
                request_model,
            )
        }
    } else if continuity_contract.is_provider_state_bound() {
        executor.select_supported_candidate_with_runtime_state(route_state, runtime, request_model)
    } else {
        executor.select_supported_candidate_with_soft_affinity_runtime_state(
            route_state,
            runtime,
            request_model,
        )
    }
}

pub(super) fn restrict_route_state_to_affinity_continuity_domain(
    contract: RequestContinuityContract,
    route_state: &mut RoutePlanAttemptState,
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
) {
    if !contract.should_restrict_to_affinity_continuity_domain() {
        return;
    }
    let Some(affinity_provider_endpoint) = runtime.affinity_provider_endpoint() else {
        return;
    };
    let topology = template.continuity_topology();
    let Some(candidate) = topology.find_candidate_by_provider_endpoint(affinity_provider_endpoint)
    else {
        return;
    };
    route_state.restrict_to_continuity_domain(topology.candidate_domain(candidate));
}

pub(super) fn log_route_continuity_blocked(
    service_name: &str,
    request_id: u64,
    contract: RequestContinuityContract,
    transport: Option<&str>,
) {
    let reason = contract.missing_affinity_trace_reason();
    let mut payload = serde_json::json!({
        "event": "route_continuity_blocked",
        "service": service_name,
        "request_id": request_id,
        "reason": reason,
        "continuity_class": contract.continuity_class(),
        "affinity_source": "none",
        "provider_failover_allowed": contract.allow_provider_failover(),
        "provider_failover_blocked_reason": reason,
        "balance_signal_authoritative": false,
    });
    if let Some(transport) = transport
        && let Some(object) = payload.as_object_mut()
    {
        object.insert(
            "transport".to_string(),
            serde_json::Value::String(transport.to_string()),
        );
    }
    log_retry_trace(payload);
}

pub(super) async fn select_websocket_route_graph_target(
    params: WebSocketRouteGraphSelectionParams<'_, '_>,
) -> Result<RouteGraphTargetSelection, RouteGraphSelectionFailure> {
    let WebSocketRouteGraphSelectionParams {
        proxy,
        executor,
        runtime,
        route_state,
        request_id,
        request_model,
        request_is_remote_compaction,
        continuity_contract,
    } = params;

    loop {
        let selection = select_route_graph_candidate(
            executor,
            route_state,
            runtime,
            request_model,
            request_is_remote_compaction,
            continuity_contract,
        );
        if let Some(selected) = selection.selected {
            let candidate = selected.candidate;
            let concurrency_permit = match try_acquire_candidate_concurrency_permit(
                proxy,
                executor.template(),
                candidate,
            ) {
                Ok(permit) => permit,
                Err(ConcurrencyAcquireError::Saturated { active, limit }) => {
                    let provider_endpoint = executor
                        .template()
                        .candidate_provider_endpoint_key(candidate);
                    route_state.avoid_provider_endpoint(provider_endpoint.clone());
                    log_retry_trace(serde_json::json!({
                        "event": "route_candidate_concurrency_saturated",
                        "service": proxy.service_name,
                        "request_id": request_id,
                        "provider_endpoint_key": provider_endpoint.stable_key(),
                        "active": active,
                        "limit": limit,
                    }));
                    continue;
                }
            };

            return Ok(RouteGraphTargetSelection {
                target: AttemptTarget::from_candidate(proxy.service_name, candidate),
                avoided_candidate_indices: selection.avoided_candidate_indices,
                avoided_total: selection.avoided_total,
                total_upstreams: selection.total_upstreams,
                concurrency_permit,
            });
        }

        if let Some(report) = route_unavailable_report(
            proxy.service_name,
            request_id,
            executor,
            runtime,
            route_state,
            request_model,
        ) {
            let (status, message) = report.failure_status_message();
            return Err(RouteGraphSelectionFailure {
                status,
                message,
                route_attempts: report.route_attempts,
            });
        }
        return Err(RouteGraphSelectionFailure::new(
            StatusCode::BAD_GATEWAY,
            format!("no route candidate supports model {request_model:?}"),
        ));
    }
}
