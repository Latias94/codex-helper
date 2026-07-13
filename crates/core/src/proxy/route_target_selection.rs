use std::time::Duration;

use crate::config::SchedulingPreset;
use crate::logging::log_control_trace_event;
use crate::routing_ir::{
    RouteCandidate, RoutePlanAttemptSelection, RoutePlanAttemptState, RoutePlanExecutor,
    RoutePlanRuntimeState, RoutePlanTemplate,
};
use crate::runtime_store::ProviderPolicySnapshot;

use super::ProxyService;
use super::concurrency_limits::{
    ConcurrencyAcquireError, ConcurrencyLimit, ConcurrencyPermit, ConcurrencyWaitPolicy,
};
use super::request_continuity::RequestContinuityContract;
use super::route_affinity::apply_session_route_affinity_for_template;

pub(super) async fn route_graph_runtime_for_request(
    proxy: &ProxyService,
    template: &RoutePlanTemplate,
    runtime_revision: u64,
    provider_policy: &ProviderPolicySnapshot,
    session_id: Option<&str>,
) -> RoutePlanRuntimeState {
    let mut runtime = proxy
        .state
        .route_plan_runtime_state_with_provider_policy(proxy.service_name, provider_policy)
        .await;
    apply_concurrency_snapshots_to_runtime(proxy, template, runtime_revision, &mut runtime);
    apply_session_route_affinity_for_template(proxy, session_id, template, &mut runtime).await;
    runtime
}

pub(super) fn apply_concurrency_snapshots_to_runtime(
    proxy: &ProxyService,
    template: &RoutePlanTemplate,
    runtime_revision: u64,
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
        let Some(limit) = ConcurrencyLimit::new(limit, runtime_revision) else {
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

pub(super) async fn acquire_candidate_concurrency_permit(
    proxy: &ProxyService,
    template: &RoutePlanTemplate,
    candidate: &RouteCandidate,
    runtime_revision: u64,
    session_id: Option<&str>,
) -> Result<Option<ConcurrencyPermit>, ConcurrencyAcquireError> {
    let Some(limit_value) = candidate.concurrency.max_concurrent_requests else {
        return Ok(None);
    };
    if limit_value == 0 {
        return Ok(None);
    }
    let provider_endpoint = template.candidate_provider_endpoint_key(candidate);
    let Some(key) = candidate
        .concurrency
        .limit_key(proxy.service_name, &provider_endpoint)
    else {
        return Ok(None);
    };
    let Some(limit) = ConcurrencyLimit::new(limit_value, runtime_revision) else {
        return Ok(None);
    };
    proxy
        .concurrency_limiter
        .acquire(
            key,
            limit,
            session_id.map(ToOwned::to_owned),
            concurrency_wait_policy(template.scheduling_preset, limit_value),
        )
        .await
        .map(Some)
}

pub(super) fn runtime_for_capacity_wait_selection(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
) -> RoutePlanRuntimeState {
    let mut selection_runtime = runtime.clone();
    if template.scheduling_preset == SchedulingPreset::ThroughputFirst {
        return selection_runtime;
    }
    for candidate in &template.candidates {
        clear_candidate_concurrency_saturation(template, candidate, &mut selection_runtime);
    }
    selection_runtime
}

pub(super) fn runtime_for_acquired_candidate_revalidation(
    template: &RoutePlanTemplate,
    runtime: &RoutePlanRuntimeState,
    candidate: &RouteCandidate,
) -> RoutePlanRuntimeState {
    let mut validation_runtime = runtime.clone();
    clear_candidate_concurrency_saturation(template, candidate, &mut validation_runtime);
    validation_runtime
}

fn clear_candidate_concurrency_saturation(
    template: &RoutePlanTemplate,
    candidate: &RouteCandidate,
    runtime: &mut RoutePlanRuntimeState,
) {
    let provider_endpoint = template.candidate_provider_endpoint_key(candidate);
    let mut state = runtime.provider_endpoint(&provider_endpoint);
    state.concurrency_saturated = false;
    runtime.set_provider_endpoint(provider_endpoint, state);
}

fn concurrency_wait_policy(preset: SchedulingPreset, limit: u32) -> ConcurrencyWaitPolicy {
    match preset {
        SchedulingPreset::ContinuityFirst => {
            ConcurrencyWaitPolicy::new(Duration::from_secs(8), limit.saturating_mul(4).max(4))
        }
        SchedulingPreset::Balanced => {
            ConcurrencyWaitPolicy::new(Duration::from_secs(2), limit.saturating_mul(2).max(2))
        }
        SchedulingPreset::ThroughputFirst => ConcurrencyWaitPolicy::new(Duration::ZERO, 0),
    }
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
    log_control_trace_event(payload);
}
