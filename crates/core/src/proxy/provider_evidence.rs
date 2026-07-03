use crate::logging::now_ms;
use crate::policy_actions::PolicyAction;
use crate::provider_signals::{
    ProviderSignal, ProviderSignalConfidence, ProviderSignalKind, ProviderSignalSource,
    ProviderSignalTarget, ProviderSignalTrace,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::state::ProxyState;

use super::attempt_target::AttemptTarget;
use super::classify::{
    ClassifiedUpstreamResponse, UPSTREAM_OVERLOADED_CLASS, UPSTREAM_RATE_LIMITED_CLASS,
};

const POLICY_ACTION_GENERATION: u64 = 1;

pub(super) struct ResponseEvidenceParams<'a> {
    pub(super) target: &'a AttemptTarget,
    pub(super) classified_response: &'a ClassifiedUpstreamResponse,
    pub(super) status_code: u16,
    pub(super) error_class: Option<&'a str>,
    pub(super) route_facing: bool,
    pub(super) default_cooldown_secs: u64,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ProviderResponseEvidence {
    pub(super) signals: Vec<ProviderSignal>,
    pub(super) actions: Vec<PolicyAction>,
}

impl ProviderResponseEvidence {
    pub(super) async fn apply_to_state(&self, service_name: &str, state: &ProxyState) {
        for action in &self.actions {
            state
                .upsert_owned_policy_action(service_name, action.clone())
                .await;
        }
    }
}

pub(super) fn response_evidence_from_classification(
    params: ResponseEvidenceParams<'_>,
) -> ProviderResponseEvidence {
    let ResponseEvidenceParams {
        target,
        classified_response,
        status_code,
        error_class,
        route_facing,
        default_cooldown_secs,
    } = params;
    let Some(provider_endpoint_key) = target_provider_endpoint_key(target) else {
        return ProviderResponseEvidence::default();
    };

    let class = error_class
        .or(classified_response.class.as_deref())
        .unwrap_or("upstream_response_error");
    let Some(kind) = signal_kind_for_class(class, status_code) else {
        return ProviderResponseEvidence::default();
    };

    let now_ms = now_ms();
    let retry_after_secs = classified_response.retry_after_secs();
    let confidence = signal_confidence(class, classified_response, &kind, status_code);
    let mut signal = ProviderSignal {
        kind,
        source: ProviderSignalSource::UpstreamResponse,
        target: ProviderSignalTarget::ProviderEndpoint {
            provider_endpoint_key,
        },
        confidence,
        observed_at_ms: now_ms,
        route_facing,
        retry_after_secs,
        reset_after_secs: retry_after_secs,
        reason: Some(class.to_string()),
        error_class: Some(class.to_string()),
        trace: ProviderSignalTrace {
            cf_ray: classified_response.cf_ray.clone(),
            ..ProviderSignalTrace::default()
        },
    };
    if signal.trace.is_empty() {
        signal.trace = ProviderSignalTrace::default();
    }

    let actions = PolicyAction::cooldown_from_signal(
        signal.clone(),
        now_ms,
        default_cooldown_secs,
        POLICY_ACTION_GENERATION,
    )
    .into_iter()
    .collect();

    ProviderResponseEvidence {
        signals: vec![signal],
        actions,
    }
}

fn target_provider_endpoint_key(target: &AttemptTarget) -> Option<ProviderEndpointKey> {
    target.provider_endpoint_ref().cloned().or_else(|| {
        target
            .provider_endpoint_key()
            .and_then(parse_provider_endpoint_key)
    })
}

fn parse_provider_endpoint_key(value: String) -> Option<ProviderEndpointKey> {
    let mut parts = value.split('/');
    let service_name = parts.next()?.trim();
    let provider_id = parts.next()?.trim();
    let endpoint_id = parts.next()?.trim();
    if parts.next().is_some()
        || service_name.is_empty()
        || provider_id.is_empty()
        || endpoint_id.is_empty()
    {
        return None;
    }
    Some(ProviderEndpointKey::new(
        service_name,
        provider_id,
        endpoint_id,
    ))
}

fn signal_kind_for_class(class: &str, status_code: u16) -> Option<ProviderSignalKind> {
    match class {
        UPSTREAM_RATE_LIMITED_CLASS => Some(ProviderSignalKind::RateLimit),
        UPSTREAM_OVERLOADED_CLASS => Some(ProviderSignalKind::Capacity),
        "cloudflare_challenge" | "cloudflare_timeout" => Some(ProviderSignalKind::Transport),
        "upstream_transport_error"
        | "upstream_body_read_error"
        | "upstream_stream_error"
        | "upstream_stream_idle_timeout" => Some(ProviderSignalKind::Transport),
        "routing_mismatch_capability" => Some(ProviderSignalKind::Capability),
        _ if status_code == 429 => Some(ProviderSignalKind::RateLimit),
        _ if matches!(status_code, 503 | 529) => Some(ProviderSignalKind::Capacity),
        _ => None,
    }
}

fn signal_confidence(
    class: &str,
    classified_response: &ClassifiedUpstreamResponse,
    kind: &ProviderSignalKind,
    status_code: u16,
) -> ProviderSignalConfidence {
    if matches!(kind, ProviderSignalKind::Transport) && is_transport_error_class(class) {
        return ProviderSignalConfidence::High;
    }
    if classified_response
        .throttle_signal
        .as_ref()
        .is_some_and(|signal| signal.strong)
    {
        return ProviderSignalConfidence::High;
    }
    if classified_response.retry_after_secs().is_some()
        && matches!(
            kind,
            ProviderSignalKind::RateLimit
                | ProviderSignalKind::Capacity
                | ProviderSignalKind::Transport
        )
    {
        return ProviderSignalConfidence::High;
    }
    if matches!(status_code, 429 | 503 | 529) {
        ProviderSignalConfidence::High
    } else {
        ProviderSignalConfidence::Medium
    }
}

fn is_transport_error_class(class: &str) -> bool {
    matches!(
        class,
        "cloudflare_challenge"
            | "cloudflare_timeout"
            | "upstream_transport_error"
            | "upstream_body_read_error"
            | "upstream_stream_error"
            | "upstream_stream_idle_timeout"
    )
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use std::collections::BTreeMap;

    use crate::routing_ir::{RouteCandidate, RouteCandidateConcurrency};

    use super::super::classify::classify_observed_upstream_response;
    use super::*;

    fn provider_target() -> AttemptTarget {
        let candidate = RouteCandidate {
            provider_id: "monthly".to_string(),
            provider_alias: None,
            endpoint_id: "default".to_string(),
            base_url: "https://relay.example/v1".to_string(),
            continuity_domain: None,
            auth: Default::default(),
            tags: BTreeMap::new(),
            supported_models: BTreeMap::new(),
            model_mapping: BTreeMap::new(),
            route_path: vec!["root".to_string(), "monthly".to_string()],
            preference_group: 0,
            stable_index: 0,
            concurrency: RouteCandidateConcurrency::default(),
            compatibility_station_name: None,
            compatibility_upstream_index: None,
        };
        AttemptTarget::from_candidate("codex", &candidate)
    }

    #[test]
    fn rate_limit_classification_creates_owned_policy_action() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let body =
            br#"{"error":{"type":"usage_limit_reached","message":"usage limit","resets_in_seconds":12}}"#;
        let classified = classify_observed_upstream_response(429, &headers, body);

        let evidence = response_evidence_from_classification(ResponseEvidenceParams {
            target: &provider_target(),
            classified_response: &classified,
            status_code: 429,
            error_class: classified.class.as_deref(),
            route_facing: true,
            default_cooldown_secs: 30,
        });

        assert_eq!(evidence.signals.len(), 1);
        assert_eq!(evidence.signals[0].kind, ProviderSignalKind::RateLimit);
        assert_eq!(
            evidence.signals[0].confidence,
            ProviderSignalConfidence::High
        );
        assert_eq!(evidence.signals[0].reset_after_secs, Some(12));
        assert_eq!(evidence.actions.len(), 1);
        assert_eq!(
            evidence.actions[0].expires_at_ms,
            evidence.actions[0].created_at_ms + 12_000
        );
    }

    #[test]
    fn capability_mismatch_is_recorded_only() {
        let classified = ClassifiedUpstreamResponse {
            class: Some("routing_mismatch_capability".to_string()),
            hint: None,
            cf_ray: None,
            throttle_signal: None,
        };

        let evidence = response_evidence_from_classification(ResponseEvidenceParams {
            target: &provider_target(),
            classified_response: &classified,
            status_code: 400,
            error_class: classified.class.as_deref(),
            route_facing: false,
            default_cooldown_secs: 30,
        });

        assert_eq!(evidence.signals.len(), 1);
        assert_eq!(evidence.signals[0].kind, ProviderSignalKind::Capability);
        assert!(evidence.actions.is_empty());
    }

    #[test]
    fn rate_limit_without_reset_horizon_is_recorded_only() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let body = br#"{"error":{"type":"rate_limit_error","message":"too many requests"}}"#;
        let classified = classify_observed_upstream_response(429, &headers, body);

        let evidence = response_evidence_from_classification(ResponseEvidenceParams {
            target: &provider_target(),
            classified_response: &classified,
            status_code: 429,
            error_class: classified.class.as_deref(),
            route_facing: true,
            default_cooldown_secs: 30,
        });

        assert_eq!(evidence.signals.len(), 1);
        assert_eq!(evidence.signals[0].kind, ProviderSignalKind::RateLimit);
        assert_eq!(evidence.signals[0].reset_after_secs, None);
        assert!(evidence.actions.is_empty());
    }
}
