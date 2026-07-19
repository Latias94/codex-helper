use crate::endpoint_health::CooldownBackoff;
use crate::logging::now_ms;
use crate::state::ProxyState;

use crate::routing_ir::CapturedRouteCandidate;

pub(super) async fn record_attempt_success(
    state: &ProxyState,
    service_name: &str,
    target: &CapturedRouteCandidate,
) {
    state
        .record_runtime_upstream_attempt_success(service_name, target.runtime_identity(), now_ms())
        .await;
}

pub(super) async fn record_attempt_failure(
    state: &ProxyState,
    service_name: &str,
    target: &CapturedRouteCandidate,
    failure_threshold_cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
) {
    state
        .record_runtime_upstream_attempt_failure(
            service_name,
            target.runtime_identity(),
            failure_threshold_cooldown_secs,
            cooldown_backoff,
        )
        .await;
}

pub(super) async fn penalize_attempt_target(
    state: &ProxyState,
    service_name: &str,
    target: &CapturedRouteCandidate,
    cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
) {
    state
        .penalize_runtime_upstream_attempt(
            service_name,
            target.runtime_identity(),
            cooldown_secs,
            cooldown_backoff,
        )
        .await;
}
