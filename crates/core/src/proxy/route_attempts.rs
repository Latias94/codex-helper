use std::collections::HashSet;

use axum::http::StatusCode;

use crate::logging::RouteAttemptLog;
use crate::policy_actions::PolicyAction;
use crate::provider_signals::ProviderSignal;

use super::classify::CLIENT_ERROR_NON_RETRYABLE_CLASS;
use crate::routing_ir::CapturedRouteCandidate;

pub(super) struct StartRouteAttemptParams<'a> {
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) provider_id: Option<&'a str>,
    pub(super) provider_attempt: u32,
    pub(super) upstream_attempt: u32,
    pub(super) provider_max_attempts: u32,
    pub(super) upstream_max_attempts: u32,
    pub(super) model_note: &'a str,
    pub(super) avoid_set: &'a HashSet<usize>,
    pub(super) avoided_total: usize,
    pub(super) total_upstreams: usize,
}

pub(super) struct UnsupportedModelSkipParams<'a> {
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) requested_model: &'a str,
    pub(super) provider_attempt: u32,
    pub(super) provider_max_attempts: u32,
    pub(super) avoid_set: &'a HashSet<usize>,
    pub(super) avoided_total: usize,
    pub(super) total_upstreams: usize,
}

pub(super) struct StatusRouteAttemptParams<'a> {
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) route_attempt_index: usize,
    pub(super) status_code: u16,
    pub(super) error_class: Option<&'a str>,
    pub(super) model_note: &'a str,
    pub(super) upstream_headers_ms: u64,
    pub(super) duration_ms: u64,
    pub(super) cooldown_secs: Option<u64>,
    pub(super) cooldown_reason: Option<&'a str>,
    pub(super) provider_signals: Vec<ProviderSignal>,
    pub(super) policy_actions: Vec<PolicyAction>,
}

pub(super) struct ErrorRouteAttemptParams<'a> {
    pub(super) target: &'a CapturedRouteCandidate,
    pub(super) route_attempt_index: usize,
    pub(super) kind: RouteAttemptErrorKind,
    pub(super) model_note: &'a str,
    pub(super) duration_ms: Option<u64>,
    pub(super) cooldown_secs: Option<u64>,
    pub(super) cooldown_reason: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CandidateSkip {
    decision: String,
    model: Option<String>,
}

impl CandidateSkip {
    fn unsupported_model(requested_model: &str) -> Self {
        Self {
            decision: "skipped_capability_mismatch".to_string(),
            model: normalize_model(requested_model),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AttemptOutcome {
    decision: String,
    status_code: Option<u16>,
    class: Option<String>,
    model: Option<String>,
    ttfb_ms: Option<u64>,
    duration_ms: Option<u64>,
    cooldown_secs: Option<u64>,
    cooldown_reason: Option<String>,
    provider_signals: Vec<ProviderSignal>,
    policy_actions: Vec<PolicyAction>,
    skipped: bool,
}

impl AttemptOutcome {
    fn from_skip(skip: CandidateSkip) -> Self {
        Self {
            decision: skip.decision,
            status_code: None,
            class: None,
            model: skip.model,
            ttfb_ms: None,
            duration_ms: None,
            cooldown_secs: None,
            cooldown_reason: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            skipped: true,
        }
    }

    fn from_status(params: &StatusRouteAttemptParams<'_>) -> Self {
        let cooldown_secs = normalize_cooldown(params.cooldown_secs);
        let decision = status_decision(params.status_code, params.error_class);
        Self {
            decision: decision.to_string(),
            status_code: Some(params.status_code),
            class: params.error_class.map(ToOwned::to_owned),
            model: normalize_model(params.model_note),
            ttfb_ms: Some(params.upstream_headers_ms),
            duration_ms: Some(params.duration_ms),
            cooldown_secs,
            cooldown_reason: cooldown_secs
                .and_then(|_| params.cooldown_reason.map(ToOwned::to_owned)),
            provider_signals: params.provider_signals.clone(),
            policy_actions: params.policy_actions.clone(),
            skipped: false,
        }
    }

    fn from_error(params: &ErrorRouteAttemptParams<'_>) -> Self {
        let cooldown_secs = normalize_cooldown(params.cooldown_secs);
        Self {
            decision: params.kind.decision().to_string(),
            status_code: None,
            class: Some(params.kind.error_class().to_string()),
            model: normalize_model(params.model_note),
            ttfb_ms: None,
            duration_ms: params.duration_ms,
            cooldown_secs,
            cooldown_reason: cooldown_secs
                .and_then(|_| params.cooldown_reason.map(ToOwned::to_owned)),
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            skipped: false,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum RouteAttemptErrorKind {
    Lifecycle,
    TargetBuild,
    Transport,
    BodyRead,
    BodyTooLarge,
}

impl RouteAttemptErrorKind {
    fn decision(self) -> &'static str {
        match self {
            RouteAttemptErrorKind::Lifecycle => "failed_lifecycle_store",
            RouteAttemptErrorKind::TargetBuild => "failed_target_build",
            RouteAttemptErrorKind::Transport => "failed_transport",
            RouteAttemptErrorKind::BodyRead => "failed_body_read",
            RouteAttemptErrorKind::BodyTooLarge => "failed_body_too_large",
        }
    }

    fn error_class(self) -> &'static str {
        match self {
            RouteAttemptErrorKind::Lifecycle => "lifecycle_store_error",
            RouteAttemptErrorKind::TargetBuild => "target_build_error",
            RouteAttemptErrorKind::Transport => "upstream_transport_error",
            RouteAttemptErrorKind::BodyRead => "upstream_body_read_error",
            RouteAttemptErrorKind::BodyTooLarge => "upstream_response_body_too_large",
        }
    }
}

pub(super) fn start_selected_route_attempt(
    route_attempts: &mut Vec<RouteAttemptLog>,
    params: StartRouteAttemptParams<'_>,
) -> usize {
    let attempt_index = route_attempts.len() as u32;
    let mut attempt = RouteAttemptLog {
        attempt_index,
        provider_id: non_dash(params.provider_id)
            .map(ToOwned::to_owned)
            .or_else(|| Some(params.target.provider_id().to_owned())),
        endpoint_id: Some(params.target.endpoint_id().to_owned()),
        provider_endpoint_key: Some(params.target.provider_endpoint_key()),
        preference_group: Some(params.target.preference_group()),
        route_path: params.target.route_path().to_vec(),
        provider_attempt: Some(params.provider_attempt + 1),
        upstream_attempt: Some(params.upstream_attempt + 1),
        provider_max_attempts: Some(params.provider_max_attempts),
        upstream_max_attempts: Some(params.upstream_max_attempts),
        avoided_candidate_indices: sorted_avoid_set(params.avoid_set),
        avoided_total: Some(params.avoided_total),
        total_upstreams: Some(params.total_upstreams),
        decision: "selected".to_string(),
        model: normalize_model(params.model_note),
        ..Default::default()
    };
    attempt.refresh_code();
    route_attempts.push(attempt);
    route_attempts.len() - 1
}

pub(super) fn record_unsupported_model_skip(
    route_attempts: &mut Vec<RouteAttemptLog>,
    params: UnsupportedModelSkipParams<'_>,
) {
    let outcome =
        AttemptOutcome::from_skip(CandidateSkip::unsupported_model(params.requested_model));
    let mut attempt = RouteAttemptLog {
        attempt_index: route_attempts.len() as u32,
        provider_attempt: Some(params.provider_attempt + 1),
        provider_max_attempts: Some(params.provider_max_attempts),
        avoided_candidate_indices: sorted_avoid_set(params.avoid_set),
        avoided_total: Some(params.avoided_total),
        total_upstreams: Some(params.total_upstreams),
        ..Default::default()
    };
    fill_attempt_identity(&mut attempt, params.target);
    apply_attempt_outcome(&mut attempt, outcome);
    route_attempts.push(attempt);
}

pub(super) fn record_status_route_attempt(
    route_attempts: &mut Vec<RouteAttemptLog>,
    params: StatusRouteAttemptParams<'_>,
) {
    let outcome = AttemptOutcome::from_status(&params);

    if let Some(attempt) = route_attempts.get_mut(params.route_attempt_index) {
        fill_attempt_identity(attempt, params.target);
        apply_attempt_outcome(attempt, outcome);
        return;
    }

    let mut attempt = RouteAttemptLog {
        attempt_index: route_attempts.len() as u32,
        ..Default::default()
    };
    fill_attempt_identity(&mut attempt, params.target);
    apply_attempt_outcome(&mut attempt, outcome);
    route_attempts.push(attempt);
}

pub(super) fn record_error_route_attempt(
    route_attempts: &mut Vec<RouteAttemptLog>,
    params: ErrorRouteAttemptParams<'_>,
) {
    let outcome = AttemptOutcome::from_error(&params);

    if let Some(attempt) = route_attempts.get_mut(params.route_attempt_index) {
        fill_attempt_identity(attempt, params.target);
        apply_attempt_outcome(attempt, outcome);
        return;
    }

    let mut attempt = RouteAttemptLog {
        attempt_index: route_attempts.len() as u32,
        ..Default::default()
    };
    fill_attempt_identity(&mut attempt, params.target);
    apply_attempt_outcome(&mut attempt, outcome);
    route_attempts.push(attempt);
}

fn fill_attempt_identity(attempt: &mut RouteAttemptLog, target: &CapturedRouteCandidate) {
    if attempt.provider_id.is_none() {
        attempt.provider_id = Some(target.provider_id().to_owned());
    }
    if attempt.endpoint_id.is_none() {
        attempt.endpoint_id = Some(target.endpoint_id().to_owned());
    }
    if attempt.provider_endpoint_key.is_none() {
        attempt.provider_endpoint_key = Some(target.provider_endpoint_key());
    }
    if attempt.preference_group.is_none() {
        attempt.preference_group = Some(target.preference_group());
    }
    if attempt.route_path.is_empty() {
        attempt.route_path = target.route_path().to_vec();
    }
}

fn apply_attempt_outcome(attempt: &mut RouteAttemptLog, outcome: AttemptOutcome) {
    attempt.decision = outcome.decision;
    attempt.status_code = outcome.status_code;
    attempt.error_class = outcome.class;
    attempt.model = outcome.model;
    attempt.upstream_headers_ms = outcome.ttfb_ms;
    attempt.duration_ms = outcome.duration_ms;
    attempt.cooldown_secs = outcome.cooldown_secs;
    attempt.cooldown_reason = outcome.cooldown_reason;
    attempt.provider_signals = outcome.provider_signals;
    attempt.policy_actions = outcome.policy_actions;
    attempt.skipped = outcome.skipped;
    attempt.refresh_code();
}

fn sorted_avoid_set(avoid_set: &HashSet<usize>) -> Vec<usize> {
    let mut values = avoid_set.iter().copied().collect::<Vec<_>>();
    values.sort_unstable();
    values
}

fn non_dash(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "-")
}

fn normalize_model(value: &str) -> Option<String> {
    non_dash(Some(value)).map(ToOwned::to_owned)
}

fn normalize_cooldown(value: Option<u64>) -> Option<u64> {
    value.filter(|secs| *secs > 0)
}

fn status_decision(status_code: u16, error_class: Option<&str>) -> &'static str {
    if matches!(
        error_class,
        Some("reasoning_guard_triggered" | "reasoning_guard_blocked")
    ) {
        return "failed_reasoning_guard";
    }
    if (200..300).contains(&status_code) || status_code == StatusCode::SWITCHING_PROTOCOLS.as_u16()
    {
        "completed"
    } else if error_class == Some(CLIENT_ERROR_NON_RETRYABLE_CLASS) {
        "failed_client_request"
    } else {
        "failed_status"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_decision_separates_non_retryable_client_errors() {
        assert_eq!(
            status_decision(400, Some(CLIENT_ERROR_NON_RETRYABLE_CLASS)),
            "failed_client_request"
        );
        assert_eq!(status_decision(400, None), "failed_status");
        assert_eq!(status_decision(200, None), "completed");
        assert_eq!(
            status_decision(StatusCode::SWITCHING_PROTOCOLS.as_u16(), None),
            "completed"
        );
    }
}
