use crate::lb::{CooldownBackoff, LoadBalancer};
use crate::logging::now_ms;
use crate::state::ProxyState;

use super::attempt_target::AttemptTarget;

pub(super) async fn record_attempt_success(
    state: &ProxyState,
    service_name: &str,
    lb: Option<&LoadBalancer>,
    target: &AttemptTarget,
    failure_threshold_cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
) {
    match target {
        AttemptTarget::Legacy(selected) => {
            if let Some(lb) = lb {
                lb.record_result_with_backoff(
                    selected.index,
                    true,
                    failure_threshold_cooldown_secs,
                    cooldown_backoff,
                );
            }
        }
        AttemptTarget::ProviderEndpoint(provider_target) => {
            state
                .record_provider_endpoint_attempt_success(
                    service_name,
                    provider_target.provider_endpoint.clone(),
                    now_ms(),
                )
                .await;
        }
    }
}

pub(super) async fn record_attempt_failure(
    state: &ProxyState,
    service_name: &str,
    lb: Option<&LoadBalancer>,
    target: &AttemptTarget,
    failure_threshold_cooldown_secs: u64,
    cooldown_backoff: CooldownBackoff,
) {
    match target {
        AttemptTarget::Legacy(selected) => {
            if let Some(lb) = lb {
                lb.record_result_with_backoff(
                    selected.index,
                    false,
                    failure_threshold_cooldown_secs,
                    cooldown_backoff,
                );
            }
        }
        AttemptTarget::ProviderEndpoint(provider_target) => {
            state
                .record_provider_endpoint_attempt_failure(
                    service_name,
                    provider_target.provider_endpoint.clone(),
                    failure_threshold_cooldown_secs,
                    cooldown_backoff,
                )
                .await;
        }
    }
}

pub(super) async fn penalize_attempt_target(
    state: &ProxyState,
    service_name: &str,
    lb: Option<&LoadBalancer>,
    target: &AttemptTarget,
    cooldown_secs: u64,
    reason: &str,
    cooldown_backoff: CooldownBackoff,
) {
    match target {
        AttemptTarget::Legacy(selected) => {
            if let Some(lb) = lb {
                lb.penalize_with_backoff(selected.index, cooldown_secs, reason, cooldown_backoff);
            }
        }
        AttemptTarget::ProviderEndpoint(provider_target) => {
            state
                .penalize_provider_endpoint_attempt(
                    service_name,
                    provider_target.provider_endpoint.clone(),
                    cooldown_secs,
                    cooldown_backoff,
                )
                .await;
        }
    }
}
