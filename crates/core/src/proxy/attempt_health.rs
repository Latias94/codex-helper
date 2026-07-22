use crate::endpoint_health::{
    CooldownBackoff, RouteCapability, RuntimeHealthDomain, RuntimeHealthHalfOpenTerminal,
};
use crate::logging::now_ms;
use crate::state::{DispatchedRuntimeHealthHalfOpenProbe, ProxyState};

use crate::routing_ir::CapturedRouteCandidate;

pub(super) async fn record_attempt_success(
    state: &ProxyState,
    service_name: &str,
    target: &CapturedRouteCandidate,
    capability: RouteCapability,
) {
    state
        .record_runtime_upstream_attempt_success_for_capability(
            service_name,
            target.runtime_identity(),
            capability,
            now_ms(),
        )
        .await;
}

pub(super) async fn record_attempt_failure(
    state: &ProxyState,
    service_name: &str,
    target: &CapturedRouteCandidate,
    domain: RuntimeHealthDomain,
    failure_threshold_cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
) {
    state
        .record_runtime_upstream_attempt_failure_for_domain(
            service_name,
            target.runtime_identity(),
            domain,
            failure_threshold_cooldown_secs,
            cooldown_backoff,
        )
        .await;
}

pub(super) async fn penalize_attempt_target(
    state: &ProxyState,
    service_name: &str,
    target: &CapturedRouteCandidate,
    domain: RuntimeHealthDomain,
    cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
) {
    state
        .penalize_runtime_upstream_attempt_for_domain(
            service_name,
            target.runtime_identity(),
            domain,
            cooldown_secs,
            cooldown_backoff,
        )
        .await;
}

pub(super) async fn settle_or_record_attempt_success(
    state: &ProxyState,
    service_name: &str,
    target: &CapturedRouteCandidate,
    capability: RouteCapability,
    half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
) {
    if let Some(probe) = half_open_probe {
        let _ = state
            .settle_runtime_half_open_probe(
                probe,
                RuntimeHealthHalfOpenTerminal::Success { now_ms: now_ms() },
            )
            .await;
    } else {
        record_attempt_success(state, service_name, target, capability).await;
    }
}

pub(super) async fn settle_or_record_attempt_failure(
    state: &ProxyState,
    service_name: &str,
    target: &CapturedRouteCandidate,
    domain: RuntimeHealthDomain,
    failure_threshold_cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
    half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
) {
    if let Some(probe) = half_open_probe {
        let _ = state
            .settle_runtime_half_open_probe(
                probe,
                RuntimeHealthHalfOpenTerminal::CountedFailure {
                    domain,
                    failure_threshold_cooldown_secs,
                    cooldown_backoff,
                },
            )
            .await;
    } else {
        record_attempt_failure(
            state,
            service_name,
            target,
            domain,
            failure_threshold_cooldown_secs,
            cooldown_backoff,
        )
        .await;
    }
}

pub(super) async fn settle_or_penalize_attempt_target(
    state: &ProxyState,
    service_name: &str,
    target: &CapturedRouteCandidate,
    domain: RuntimeHealthDomain,
    cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
    half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
) {
    if let Some(probe) = half_open_probe {
        let _ = state
            .settle_runtime_half_open_probe(
                probe,
                RuntimeHealthHalfOpenTerminal::Penalty {
                    domain,
                    cooldown_secs,
                    cooldown_backoff,
                },
            )
            .await;
    } else {
        penalize_attempt_target(
            state,
            service_name,
            target,
            domain,
            cooldown_secs,
            cooldown_backoff,
        )
        .await;
    }
}

pub(super) struct RecordAndPenalizeAttemptParams<'a> {
    pub(super) state: &'a ProxyState,
    pub(super) service_name: &'a str,
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) domain: RuntimeHealthDomain,
    pub(super) failure_threshold_cooldown_secs: u64,
    pub(super) penalty_cooldown_secs: u64,
    pub(super) cooldown_backoff: CooldownBackoff,
    pub(super) half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
}

pub(super) async fn settle_or_record_and_penalize_attempt_target(
    params: RecordAndPenalizeAttemptParams<'_>,
) {
    let RecordAndPenalizeAttemptParams {
        state,
        service_name,
        target,
        domain,
        failure_threshold_cooldown_secs,
        penalty_cooldown_secs,
        cooldown_backoff,
        half_open_probe,
    } = params;
    if let Some(probe) = half_open_probe {
        let _ = state
            .settle_runtime_half_open_probe(
                probe,
                RuntimeHealthHalfOpenTerminal::Penalty {
                    domain,
                    cooldown_secs: penalty_cooldown_secs,
                    cooldown_backoff,
                },
            )
            .await;
    } else {
        record_attempt_failure(
            state,
            service_name,
            target,
            domain,
            failure_threshold_cooldown_secs,
            cooldown_backoff,
        )
        .await;
        penalize_attempt_target(
            state,
            service_name,
            target,
            domain,
            penalty_cooldown_secs,
            cooldown_backoff,
        )
        .await;
    }
}

pub(super) async fn settle_half_open_probe_neutral(
    state: &ProxyState,
    half_open_probe: Option<DispatchedRuntimeHealthHalfOpenProbe>,
) {
    if let Some(probe) = half_open_probe {
        let _ = state
            .settle_runtime_half_open_probe(probe, RuntimeHealthHalfOpenTerminal::Neutral)
            .await;
    }
}
