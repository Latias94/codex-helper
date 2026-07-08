use serde::{Deserialize, Serialize};

use crate::runtime_identity::ProviderEndpointKey;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSignalConfidence {
    Low,
    #[default]
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSignalKind {
    Quota,
    RateLimit,
    Capacity,
    Transport,
    Balance,
    ServiceStatus,
    Capability,
    LocalConcurrency,
    #[serde(other)]
    Unknown,
}

impl ProviderSignalKind {
    pub fn code(&self) -> &'static str {
        match self {
            ProviderSignalKind::Quota => "quota",
            ProviderSignalKind::RateLimit => "rate_limit",
            ProviderSignalKind::Capacity => "capacity",
            ProviderSignalKind::Transport => "transport",
            ProviderSignalKind::Balance => "balance",
            ProviderSignalKind::ServiceStatus => "service_status",
            ProviderSignalKind::Capability => "capability",
            ProviderSignalKind::LocalConcurrency => "local_concurrency",
            ProviderSignalKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSignalSource {
    UpstreamResponse,
    ResponseHeaders,
    BalanceSnapshot,
    ServiceStatus,
    CapabilityProbe,
    LocalScheduler,
    RouteAttempt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSignalTarget {
    ProviderEndpoint {
        provider_endpoint_key: ProviderEndpointKey,
    },
    Provider {
        service: String,
        provider_id: String,
    },
    Service {
        service: String,
    },
}

impl ProviderSignalTarget {
    pub fn provider_endpoint_key(&self) -> Option<&ProviderEndpointKey> {
        match self {
            Self::ProviderEndpoint {
                provider_endpoint_key,
            } => Some(provider_endpoint_key),
            Self::Provider { .. } | Self::Service { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProviderSignalTrace {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cf_ray: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderSignal {
    pub kind: ProviderSignalKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub source: ProviderSignalSource,
    pub target: ProviderSignalTarget,
    pub confidence: ProviderSignalConfidence,
    pub observed_at_ms: u64,
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub route_facing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_after_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    #[serde(default, skip_serializing_if = "ProviderSignalTrace::is_empty")]
    pub trace: ProviderSignalTrace,
}

impl ProviderSignal {
    pub fn high_confidence_route_facing(
        kind: ProviderSignalKind,
        source: ProviderSignalSource,
        target: ProviderSignalTarget,
        observed_at_ms: u64,
    ) -> Self {
        Self {
            code: Some(kind.code().to_string()),
            kind,
            source,
            target,
            confidence: ProviderSignalConfidence::High,
            observed_at_ms,
            route_facing: true,
            retry_after_secs: None,
            reset_after_secs: None,
            reason: None,
            error_class: None,
            trace: ProviderSignalTrace::default(),
        }
    }

    pub fn cooldown_horizon_secs(&self) -> Option<u64> {
        self.reset_after_secs.or(self.retry_after_secs)
    }

    pub fn is_high_confidence_route_facing(&self) -> bool {
        self.route_facing && self.confidence >= ProviderSignalConfidence::High
    }

    pub fn stable_code(&self) -> &str {
        self.code.as_deref().unwrap_or_else(|| self.kind.code())
    }
}

impl ProviderSignalTrace {
    pub fn is_empty(&self) -> bool {
        self.trace_id.is_none() && self.cf_ray.is_none() && self.upstream_request_id.is_none()
    }
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_endpoint_target_exposes_stable_key() {
        let key = ProviderEndpointKey::new("codex", "monthly", "default");
        let target = ProviderSignalTarget::ProviderEndpoint {
            provider_endpoint_key: key.clone(),
        };

        assert_eq!(target.provider_endpoint_key(), Some(&key));
    }

    #[test]
    fn cooldown_horizon_prefers_reset_then_retry_after() {
        let mut signal = ProviderSignal::high_confidence_route_facing(
            ProviderSignalKind::Quota,
            ProviderSignalSource::UpstreamResponse,
            ProviderSignalTarget::ProviderEndpoint {
                provider_endpoint_key: ProviderEndpointKey::new("codex", "monthly", "default"),
            },
            100,
        );
        signal.retry_after_secs = Some(30);
        signal.reset_after_secs = Some(60);

        assert_eq!(signal.cooldown_horizon_secs(), Some(60));
        assert!(signal.is_high_confidence_route_facing());
        assert_eq!(signal.stable_code(), "quota");
    }

    #[test]
    fn provider_signal_serializes_additive_code() {
        let signal = ProviderSignal::high_confidence_route_facing(
            ProviderSignalKind::RateLimit,
            ProviderSignalSource::UpstreamResponse,
            ProviderSignalTarget::ProviderEndpoint {
                provider_endpoint_key: ProviderEndpointKey::new("codex", "monthly", "default"),
            },
            100,
        );
        let value = serde_json::to_value(&signal).expect("serialize signal");

        assert_eq!(value["kind"].as_str(), Some("rate_limit"));
        assert_eq!(value["code"].as_str(), Some("rate_limit"));
    }

    #[test]
    fn unknown_provider_signal_kind_deserializes_as_unknown_code() {
        let signal: ProviderSignal = serde_json::from_value(serde_json::json!({
            "kind": "future_signal",
            "source": "upstream_response",
            "target": {
                "service": { "service": "codex" }
            },
            "confidence": "medium",
            "observed_at_ms": 100
        }))
        .expect("deserialize unknown signal kind");

        assert_eq!(signal.kind, ProviderSignalKind::Unknown);
        assert_eq!(signal.stable_code(), "unknown");
    }
}
