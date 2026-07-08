use serde::{Deserialize, Serialize};

use crate::logging::RouteAttemptLog;
use crate::policy_actions::{
    PolicyAction, PolicyActionKind, PolicyActionOwner, PolicyActionRecoveryState,
};
use crate::pricing::CostBreakdown;
use crate::provider_signals::{
    ProviderSignal, ProviderSignalConfidence, ProviderSignalKind, ProviderSignalSource,
    ProviderSignalTarget,
};
use crate::state::{FinishedRequest, RequestObservability, SessionIdentitySource};
use crate::usage::UsageMetrics;

pub const REQUEST_CHAIN_EXPORT_API_VERSION: u32 = 1;
pub const REQUEST_CHAIN_EXPORT_DEFAULT_LIMIT: usize = 20;
pub const REQUEST_CHAIN_EXPORT_MAX_LIMIT: usize = 100;
pub const REQUEST_CHAIN_ATTEMPT_MAX: usize = 16;
pub const REQUEST_CHAIN_SIGNALS_MAX: usize = 16;
pub const REQUEST_CHAIN_ACTIONS_MAX: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RequestChainSelector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl RequestChainSelector {
    pub fn has_identity(&self) -> bool {
        self.trace_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || self.request_id.is_some()
            || self
                .session_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }

    pub fn normalized(mut self) -> Self {
        self.trace_id = clean_identity(self.trace_id);
        self.session_id = clean_identity(self.session_id);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestChainExport {
    pub api_version: u32,
    pub selector: RequestChainSelector,
    pub limit: usize,
    pub truncated: bool,
    pub requests: Vec<RequestChainRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestChainRequest {
    pub request_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_identity_source: Option<SessionIdentitySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetrics>,
    #[serde(default, skip_serializing_if = "CostBreakdown::is_unknown")]
    pub cost: CostBreakdown,
    pub observability: RequestObservability,
    pub service: String,
    pub method: String,
    pub path: String,
    pub status_code: u16,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttfb_ms: Option<u64>,
    pub streaming: bool,
    pub ended_at_ms: u64,
    pub attempts_truncated: bool,
    pub provider_signals_truncated: bool,
    pub policy_actions_truncated: bool,
    pub route_attempts: Vec<RequestChainRouteAttempt>,
    pub provider_signals: Vec<RequestChainProviderSignal>,
    pub policy_actions: Vec<RequestChainPolicyAction>,
    pub timeline: Vec<RequestChainTimelineEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestChainRouteAttempt {
    pub attempt_index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preference_group: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_path: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_max_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_max_attempts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avoided_total: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_upstreams: Option<usize>,
    pub decision: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_headers_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cooldown_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub skipped: bool,
    pub provider_signals: Vec<RequestChainProviderSignal>,
    pub policy_actions: Vec<RequestChainPolicyAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestChainProviderSignal {
    pub kind: ProviderSignalKind,
    pub code: String,
    pub source: ProviderSignalSource,
    pub target: ProviderSignalTarget,
    pub confidence: ProviderSignalConfidence,
    pub observed_at_ms: u64,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub route_facing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestChainPolicyAction {
    pub id: String,
    pub kind: PolicyActionKind,
    pub code: String,
    pub owner: PolicyActionOwner,
    pub provider_endpoint_key: String,
    pub source_signal: RequestChainProviderSignal,
    pub confidence: ProviderSignalConfidence,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub recovery_state: PolicyActionRecoveryState,
    pub generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestChainTimelineEvent {
    pub order: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
    pub kind: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl RequestChainExport {
    pub fn from_finished_requests(
        selector: RequestChainSelector,
        limit: usize,
        truncated: bool,
        mut requests: Vec<FinishedRequest>,
    ) -> Self {
        requests.sort_by_key(|request| (request.ended_at_ms, request.id));
        Self {
            api_version: REQUEST_CHAIN_EXPORT_API_VERSION,
            selector: selector.normalized(),
            limit: limit.clamp(1, REQUEST_CHAIN_EXPORT_MAX_LIMIT),
            truncated,
            requests: requests
                .iter()
                .map(RequestChainRequest::from_finished_request)
                .collect(),
        }
    }
}

impl RequestChainRequest {
    pub fn from_finished_request(request: &FinishedRequest) -> Self {
        let route_attempts = request
            .retry
            .as_ref()
            .map(|retry| retry.route_attempts_or_derived())
            .unwrap_or_default();
        let attempts_truncated = route_attempts.len() > REQUEST_CHAIN_ATTEMPT_MAX;
        let route_attempts = route_attempts
            .iter()
            .take(REQUEST_CHAIN_ATTEMPT_MAX)
            .map(RequestChainRouteAttempt::from_attempt)
            .collect::<Vec<_>>();

        let provider_signals_truncated = request.provider_signals.len() > REQUEST_CHAIN_SIGNALS_MAX;
        let provider_signals = request
            .provider_signals
            .iter()
            .take(REQUEST_CHAIN_SIGNALS_MAX)
            .map(RequestChainProviderSignal::from_signal)
            .collect::<Vec<_>>();

        let policy_actions_truncated = request.policy_actions.len() > REQUEST_CHAIN_ACTIONS_MAX;
        let policy_actions = request
            .policy_actions
            .iter()
            .take(REQUEST_CHAIN_ACTIONS_MAX)
            .map(RequestChainPolicyAction::from_action)
            .collect::<Vec<_>>();

        let timeline =
            request_chain_timeline(request, &route_attempts, &provider_signals, &policy_actions);

        Self {
            request_id: request.id,
            trace_id: request.trace_id.clone(),
            session_id: request.session_id.clone(),
            session_identity_source: request.session_identity_source,
            client_name: request.client_name.clone(),
            model: request.model.clone(),
            reasoning_effort: request.reasoning_effort.clone(),
            service_tier: request.service_tier.clone(),
            station_name: request.station_name.clone(),
            provider_id: request.provider_id.clone(),
            usage: request.usage.clone(),
            cost: request.cost.clone(),
            observability: request.observability_view(),
            service: request.service.clone(),
            method: request.method.clone(),
            path: request.path.clone(),
            status_code: request.status_code,
            duration_ms: request.duration_ms,
            ttfb_ms: request.ttfb_ms,
            streaming: request.streaming,
            ended_at_ms: request.ended_at_ms,
            attempts_truncated,
            provider_signals_truncated,
            policy_actions_truncated,
            route_attempts,
            provider_signals,
            policy_actions,
            timeline,
        }
    }
}

impl RequestChainRouteAttempt {
    pub fn from_attempt(attempt: &RouteAttemptLog) -> Self {
        let provider_signals = attempt
            .provider_signals
            .iter()
            .take(REQUEST_CHAIN_SIGNALS_MAX)
            .map(RequestChainProviderSignal::from_signal)
            .collect();
        let policy_actions = attempt
            .policy_actions
            .iter()
            .take(REQUEST_CHAIN_ACTIONS_MAX)
            .map(RequestChainPolicyAction::from_action)
            .collect();
        Self {
            attempt_index: attempt.attempt_index,
            provider_id: attempt.provider_id.clone(),
            endpoint_id: attempt.endpoint_id.clone(),
            provider_endpoint_key: attempt.provider_endpoint_key.clone(),
            station_name: attempt.station_name.clone(),
            preference_group: attempt.preference_group,
            route_path: attempt.route_path.clone(),
            provider_attempt: attempt.provider_attempt,
            upstream_attempt: attempt.upstream_attempt,
            provider_max_attempts: attempt.provider_max_attempts,
            upstream_max_attempts: attempt.upstream_max_attempts,
            avoided_total: attempt.avoided_total,
            total_upstreams: attempt.total_upstreams,
            decision: attempt.decision.clone(),
            code: attempt.stable_code().to_string(),
            status_code: attempt.status_code,
            error_class: attempt.error_class.clone(),
            model: attempt.model.clone(),
            upstream_headers_ms: attempt.upstream_headers_ms,
            duration_ms: attempt.duration_ms,
            cooldown_secs: attempt.cooldown_secs,
            skipped: attempt.skipped,
            provider_signals,
            policy_actions,
        }
    }
}

impl RequestChainProviderSignal {
    pub fn from_signal(signal: &ProviderSignal) -> Self {
        Self {
            kind: signal.kind.clone(),
            code: signal.stable_code().to_string(),
            source: signal.source.clone(),
            target: signal.target.clone(),
            confidence: signal.confidence,
            observed_at_ms: signal.observed_at_ms,
            route_facing: signal.route_facing,
            retry_after_secs: signal.retry_after_secs,
            reset_after_secs: signal.reset_after_secs,
            error_class: signal.error_class.clone(),
            trace_id: signal.trace.trace_id.clone(),
        }
    }
}

impl RequestChainPolicyAction {
    pub fn from_action(action: &PolicyAction) -> Self {
        Self {
            id: action.id.clone(),
            kind: action.kind.clone(),
            code: action.stable_code().to_string(),
            owner: action.owner.clone(),
            provider_endpoint_key: action.provider_endpoint_key.stable_key(),
            source_signal: RequestChainProviderSignal::from_signal(&action.source_signal),
            confidence: action.confidence,
            created_at_ms: action.created_at_ms,
            expires_at_ms: action.expires_at_ms,
            recovery_state: action.recovery_state.clone(),
            generation: action.generation,
        }
    }
}

fn request_chain_timeline(
    request: &FinishedRequest,
    route_attempts: &[RequestChainRouteAttempt],
    provider_signals: &[RequestChainProviderSignal],
    policy_actions: &[RequestChainPolicyAction],
) -> Vec<RequestChainTimelineEvent> {
    let mut events = vec![RequestChainTimelineEvent {
        order: 0,
        at_ms: Some(request.ended_at_ms),
        kind: "request".to_string(),
        code: if request.status_code >= 400 {
            "request_failed".to_string()
        } else {
            "request_completed".to_string()
        },
        attempt_index: None,
        provider_id: request.provider_id.clone(),
        endpoint_id: None,
        provider_endpoint_key: None,
        status_code: Some(request.status_code),
        model: request.model.clone(),
    }];

    events.extend(
        route_attempts
            .iter()
            .map(|attempt| RequestChainTimelineEvent {
                order: 1_000 + attempt.attempt_index,
                at_ms: None,
                kind: "route_attempt".to_string(),
                code: attempt.code.clone(),
                attempt_index: Some(attempt.attempt_index),
                provider_id: attempt.provider_id.clone(),
                endpoint_id: attempt.endpoint_id.clone(),
                provider_endpoint_key: attempt.provider_endpoint_key.clone(),
                status_code: attempt.status_code,
                model: attempt.model.clone(),
            }),
    );

    events.extend(provider_signals.iter().enumerate().map(|(index, signal)| {
        let (provider_id, provider_endpoint_key) = signal_target_identity(&signal.target);
        RequestChainTimelineEvent {
            order: 2_000 + u32::try_from(index).unwrap_or(u32::MAX),
            at_ms: Some(signal.observed_at_ms),
            kind: "provider_signal".to_string(),
            code: signal.code.clone(),
            attempt_index: None,
            provider_id,
            endpoint_id: None,
            provider_endpoint_key,
            status_code: None,
            model: None,
        }
    }));

    events.extend(policy_actions.iter().enumerate().map(|(index, action)| {
        RequestChainTimelineEvent {
            order: 3_000 + u32::try_from(index).unwrap_or(u32::MAX),
            at_ms: Some(action.created_at_ms),
            kind: "policy_action".to_string(),
            code: action.code.clone(),
            attempt_index: None,
            provider_id: Some(action.source_signal_target_provider_id()),
            endpoint_id: None,
            provider_endpoint_key: Some(action.provider_endpoint_key.clone()),
            status_code: None,
            model: None,
        }
    }));

    events.sort_by_key(|event| (event.at_ms.unwrap_or(request.ended_at_ms), event.order));
    events
}

impl RequestChainPolicyAction {
    fn source_signal_target_provider_id(&self) -> String {
        match &self.source_signal.target {
            ProviderSignalTarget::ProviderEndpoint {
                provider_endpoint_key,
            } => provider_endpoint_key.provider_id.clone(),
            ProviderSignalTarget::Provider { provider_id, .. } => provider_id.clone(),
            ProviderSignalTarget::Service { service } => service.clone(),
        }
    }
}

fn signal_target_identity(target: &ProviderSignalTarget) -> (Option<String>, Option<String>) {
    match target {
        ProviderSignalTarget::ProviderEndpoint {
            provider_endpoint_key,
        } => (
            Some(provider_endpoint_key.provider_id.clone()),
            Some(provider_endpoint_key.stable_key()),
        ),
        ProviderSignalTarget::Provider { provider_id, .. } => (Some(provider_id.clone()), None),
        ProviderSignalTarget::Service { .. } => (None, None),
    }
}

fn clean_identity(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::{RetryInfo, RouteAttemptLog};
    use crate::policy_actions::PolicyActionRecoveryState;
    use crate::provider_signals::{
        ProviderSignalSource, ProviderSignalTarget, ProviderSignalTrace,
    };
    use crate::runtime_identity::ProviderEndpointKey;

    fn endpoint_key() -> ProviderEndpointKey {
        ProviderEndpointKey::new("codex", "relay", "default")
    }

    fn signal(observed_at_ms: u64) -> ProviderSignal {
        ProviderSignal {
            kind: ProviderSignalKind::RateLimit,
            code: Some("rate_limit".to_string()),
            source: ProviderSignalSource::UpstreamResponse,
            target: ProviderSignalTarget::ProviderEndpoint {
                provider_endpoint_key: endpoint_key(),
            },
            confidence: ProviderSignalConfidence::High,
            observed_at_ms,
            route_facing: true,
            retry_after_secs: Some(30),
            reset_after_secs: None,
            reason: Some("raw upstream says secret-token".to_string()),
            error_class: Some("http_429".to_string()),
            trace: ProviderSignalTrace {
                trace_id: Some("trace-a".to_string()),
                cf_ray: Some("cf-ray-secret".to_string()),
                upstream_request_id: Some("upstream-secret".to_string()),
            },
        }
    }

    fn action(source_signal: ProviderSignal) -> PolicyAction {
        PolicyAction {
            id: "codex-helper:relay:100".to_string(),
            kind: PolicyActionKind::Cooldown,
            code: Some("cooldown".to_string()),
            owner: PolicyActionOwner::CodexHelper,
            provider_endpoint_key: endpoint_key(),
            source_signal,
            reason: "raw policy reason secret-token".to_string(),
            confidence: ProviderSignalConfidence::High,
            created_at_ms: 120,
            expires_at_ms: 30_120,
            recovery_state: PolicyActionRecoveryState::Active,
            generation: 7,
        }
    }

    fn finished_request() -> FinishedRequest {
        let provider_signal = signal(110);
        let policy_action = action(provider_signal.clone());
        let mut request = FinishedRequest {
            id: 42,
            trace_id: Some("trace-a".to_string()),
            session_id: Some("session-a".to_string()),
            session_identity_source: None,
            client_name: Some("codex".to_string()),
            client_addr: Some("127.0.0.1:5555".to_string()),
            cwd: Some("C:/Users/Frankorz/private-project".to_string()),
            model: Some("gpt-5.6".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            station_name: Some("primary".to_string()),
            provider_id: Some("relay".to_string()),
            upstream_base_url: Some("https://relay.example/v1?token=secret".to_string()),
            route_decision: None,
            usage: Some(UsageMetrics {
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
                ..UsageMetrics::default()
            }),
            cost: CostBreakdown::unknown(),
            retry: Some(RetryInfo {
                attempts: 1,
                upstream_chain: vec!["raw upstream chain secret-token".to_string()],
                route_attempts: vec![RouteAttemptLog {
                    attempt_index: 0,
                    provider_id: Some("relay".to_string()),
                    endpoint_id: Some("default".to_string()),
                    provider_endpoint_key: Some(endpoint_key().stable_key()),
                    decision: "failed_status".to_string(),
                    code: Some("failed_status".to_string()),
                    status_code: Some(429),
                    error_class: Some("http_429".to_string()),
                    model: Some("gpt-5.6".to_string()),
                    raw: "raw route attempt secret-token".to_string(),
                    provider_signals: vec![provider_signal.clone()],
                    policy_actions: vec![policy_action.clone()],
                    ..RouteAttemptLog::default()
                }],
            }),
            provider_signals: vec![provider_signal],
            policy_actions: vec![policy_action],
            observability: RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 429,
            duration_ms: 1000,
            ttfb_ms: Some(250),
            streaming: true,
            ended_at_ms: 200,
        };
        request.refresh_observability();
        request
    }

    #[test]
    fn request_chain_export_sanitizes_sensitive_finished_request_fields() {
        let export = RequestChainExport::from_finished_requests(
            RequestChainSelector {
                trace_id: Some(" trace-a ".to_string()),
                request_id: None,
                session_id: None,
            },
            20,
            false,
            vec![finished_request()],
        );
        let text = serde_json::to_string(&export).expect("serialize request chain export");

        assert!(text.contains("\"trace_id\":\"trace-a\""));
        assert!(text.contains("\"route_attempts\""));
        assert!(text.contains("\"code\":\"failed_status\""));
        assert!(!text.contains("client_addr"));
        assert!(!text.contains("cwd"));
        assert!(!text.contains("upstream_base_url"));
        assert!(!text.contains("raw"));
        assert!(!text.contains("secret-token"));
        assert!(!text.contains("cf-ray-secret"));
        assert!(!text.contains("upstream-secret"));
    }

    #[test]
    fn request_chain_export_orders_session_requests_by_completion_time() {
        let mut newer = finished_request();
        newer.id = 2;
        newer.ended_at_ms = 200;
        let mut older = finished_request();
        older.id = 1;
        older.ended_at_ms = 100;

        let export = RequestChainExport::from_finished_requests(
            RequestChainSelector {
                session_id: Some("session-a".to_string()),
                ..RequestChainSelector::default()
            },
            20,
            false,
            vec![newer, older],
        );

        assert_eq!(
            export
                .requests
                .iter()
                .map(|request| request.request_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }
}
