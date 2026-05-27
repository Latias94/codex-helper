use crate::config::RoutingAffinityPolicyV5;

use super::request_body::{
    codex_compact_request_requires_affinity, codex_responses_body_has_compaction_trigger,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestTransport {
    Http,
    ResponsesWebSocket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestContinuityClass {
    StatelessOrSessionPreferred,
    ProviderStateBound,
}

impl RequestContinuityClass {
    pub(super) fn trace_label(self) -> &'static str {
        match self {
            Self::StatelessOrSessionPreferred => "stateless_or_session_preferred",
            Self::ProviderStateBound => "provider_state_bound",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestContinuityReason {
    Ordinary,
    RemoteCompactionV1,
    RemoteCompactionV2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RequestContinuityClassification {
    pub(super) transport: RequestTransport,
    pub(super) class: RequestContinuityClass,
    pub(super) reason: RequestContinuityReason,
    pub(super) is_remote_compaction_v1_request: bool,
    pub(super) is_remote_compaction_v2_request: bool,
    pub(super) remote_compaction_requires_affinity: bool,
}

pub(super) struct RequestContinuityClassificationInput<'a> {
    pub(super) transport: RequestTransport,
    pub(super) is_codex_service: bool,
    pub(super) is_user_turn: bool,
    pub(super) is_remote_compaction_v1_request: bool,
    pub(super) raw_body: &'a [u8],
}

pub(super) fn classify_request_continuity(
    input: RequestContinuityClassificationInput<'_>,
) -> RequestContinuityClassification {
    let is_remote_compaction_v2_request = input.is_codex_service
        && input.is_user_turn
        && codex_responses_body_has_compaction_trigger(input.raw_body);
    let remote_compaction_requires_affinity = (input.is_remote_compaction_v1_request
        && codex_compact_request_requires_affinity(input.raw_body))
        || is_remote_compaction_v2_request;
    let reason = if is_remote_compaction_v2_request {
        RequestContinuityReason::RemoteCompactionV2
    } else if input.is_remote_compaction_v1_request {
        RequestContinuityReason::RemoteCompactionV1
    } else {
        RequestContinuityReason::Ordinary
    };
    let class = if remote_compaction_requires_affinity {
        RequestContinuityClass::ProviderStateBound
    } else {
        RequestContinuityClass::StatelessOrSessionPreferred
    };

    RequestContinuityClassification {
        transport: input.transport,
        class,
        reason,
        is_remote_compaction_v1_request: input.is_remote_compaction_v1_request,
        is_remote_compaction_v2_request,
        remote_compaction_requires_affinity,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RequestContinuityDecision {
    StatelessOrSessionPreferred,
    ProviderStateBound { requires_known_affinity: bool },
}

impl RequestContinuityDecision {
    pub(super) fn trace_label(self) -> &'static str {
        match self {
            Self::StatelessOrSessionPreferred => "stateless_or_session_preferred",
            Self::ProviderStateBound { .. } => "provider_state_bound",
        }
    }
}

pub(super) struct RouteContinuityDecisionInput {
    pub(super) is_remote_compaction_request: bool,
    pub(super) remote_compaction_requires_affinity: bool,
    pub(super) affinity_policy: Option<RoutingAffinityPolicyV5>,
}

pub(super) fn route_continuity_decision(
    input: RouteContinuityDecisionInput,
) -> RequestContinuityDecision {
    let hard_affinity = matches!(input.affinity_policy, Some(RoutingAffinityPolicyV5::Hard));
    if input.is_remote_compaction_request
        && (input.remote_compaction_requires_affinity || hard_affinity)
    {
        RequestContinuityDecision::ProviderStateBound {
            requires_known_affinity: input.remote_compaction_requires_affinity && hard_affinity,
        }
    } else {
        RequestContinuityDecision::StatelessOrSessionPreferred
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_remote_compaction_v2_trigger_is_provider_state_bound() {
        let classification = classify_request_continuity(RequestContinuityClassificationInput {
            transport: RequestTransport::Http,
            is_codex_service: true,
            is_user_turn: true,
            is_remote_compaction_v1_request: false,
            raw_body: br#"{"input":[{"type":"message"},{"type":"compaction_trigger"}]}"#,
        });

        assert_eq!(classification.transport, RequestTransport::Http);
        assert_eq!(
            classification.class,
            RequestContinuityClass::ProviderStateBound
        );
        assert_eq!(
            classification.reason,
            RequestContinuityReason::RemoteCompactionV2
        );
        assert!(!classification.is_remote_compaction_v1_request);
        assert!(classification.is_remote_compaction_v2_request);
        assert!(
            classification.is_remote_compaction_v1_request
                || classification.is_remote_compaction_v2_request
        );
        assert!(classification.remote_compaction_requires_affinity);
    }

    #[test]
    fn websocket_response_create_compaction_trigger_is_provider_state_bound() {
        let classification = classify_request_continuity(RequestContinuityClassificationInput {
            transport: RequestTransport::ResponsesWebSocket,
            is_codex_service: true,
            is_user_turn: true,
            is_remote_compaction_v1_request: false,
            raw_body: br#"{"type":"response.create","input":[{"role":"user","content":"x"},{"type":"compaction_trigger"}]}"#,
        });

        assert_eq!(
            classification.transport,
            RequestTransport::ResponsesWebSocket
        );
        assert_eq!(
            classification.class,
            RequestContinuityClass::ProviderStateBound
        );
        assert_eq!(
            classification.reason,
            RequestContinuityReason::RemoteCompactionV2
        );
        assert!(classification.is_remote_compaction_v2_request);
        assert!(classification.remote_compaction_requires_affinity);
    }

    #[test]
    fn ordinary_websocket_response_create_is_session_preferred() {
        let classification = classify_request_continuity(RequestContinuityClassificationInput {
            transport: RequestTransport::ResponsesWebSocket,
            is_codex_service: true,
            is_user_turn: true,
            is_remote_compaction_v1_request: false,
            raw_body: br#"{"type":"response.create","input":"hello"}"#,
        });

        assert_eq!(
            classification.class,
            RequestContinuityClass::StatelessOrSessionPreferred
        );
        assert_eq!(classification.reason, RequestContinuityReason::Ordinary);
        assert!(
            !classification.is_remote_compaction_v1_request
                && !classification.is_remote_compaction_v2_request
        );
        assert!(!classification.remote_compaction_requires_affinity);
    }

    #[test]
    fn route_continuity_keeps_existing_hard_affinity_semantics_for_compact() {
        let decision = route_continuity_decision(RouteContinuityDecisionInput {
            is_remote_compaction_request: true,
            remote_compaction_requires_affinity: false,
            affinity_policy: Some(RoutingAffinityPolicyV5::Hard),
        });

        assert_eq!(
            decision,
            RequestContinuityDecision::ProviderStateBound {
                requires_known_affinity: false
            }
        );
    }

    #[test]
    fn route_continuity_treats_fallback_sticky_compact_as_tryable_state_bound() {
        let decision = route_continuity_decision(RouteContinuityDecisionInput {
            is_remote_compaction_request: true,
            remote_compaction_requires_affinity: true,
            affinity_policy: Some(RoutingAffinityPolicyV5::FallbackSticky),
        });

        assert_eq!(
            decision,
            RequestContinuityDecision::ProviderStateBound {
                requires_known_affinity: false
            }
        );
    }
}
