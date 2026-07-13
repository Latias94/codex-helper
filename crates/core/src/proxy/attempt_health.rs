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
        .record_provider_endpoint_attempt_success(
            service_name,
            target.provider_endpoint().clone(),
            now_ms(),
        )
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
        .record_provider_endpoint_attempt_failure(
            service_name,
            target.provider_endpoint().clone(),
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
        .penalize_provider_endpoint_attempt(
            service_name,
            target.provider_endpoint().clone(),
            cooldown_secs,
            cooldown_backoff,
        )
        .await;
}
