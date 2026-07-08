use serde::{Deserialize, Serialize};

use crate::provider_signals::{
    ProviderSignal, ProviderSignalConfidence, ProviderSignalKind, ProviderSignalTarget,
};
use crate::runtime_identity::ProviderEndpointKey;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyActionKind {
    Cooldown,
    #[serde(other)]
    Unknown,
}

impl PolicyActionKind {
    pub fn code(&self) -> &'static str {
        match self {
            PolicyActionKind::Cooldown => "cooldown",
            PolicyActionKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyActionOwner {
    CodexHelper,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PolicyActionRecoveryState {
    #[default]
    Active,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyAction {
    pub id: String,
    pub kind: PolicyActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub owner: PolicyActionOwner,
    pub provider_endpoint_key: ProviderEndpointKey,
    pub source_signal: ProviderSignal,
    pub reason: String,
    pub confidence: ProviderSignalConfidence,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(default)]
    pub recovery_state: PolicyActionRecoveryState,
    #[serde(default)]
    pub generation: u64,
}

impl PolicyAction {
    pub fn cooldown_from_signal(
        signal: ProviderSignal,
        created_at_ms: u64,
        default_cooldown_secs: u64,
        generation: u64,
    ) -> Option<Self> {
        if !signal.is_high_confidence_route_facing() {
            return None;
        }
        if !matches!(
            signal.kind,
            ProviderSignalKind::Quota
                | ProviderSignalKind::RateLimit
                | ProviderSignalKind::Capacity
                | ProviderSignalKind::Transport
                | ProviderSignalKind::Balance
        ) {
            return None;
        }
        let provider_endpoint_key = match &signal.target {
            ProviderSignalTarget::ProviderEndpoint {
                provider_endpoint_key,
            } => provider_endpoint_key.clone(),
            ProviderSignalTarget::Provider { .. } | ProviderSignalTarget::Service { .. } => {
                return None;
            }
        };
        let cooldown_secs = signal.cooldown_horizon_secs().or_else(|| {
            (matches!(
                signal.kind,
                ProviderSignalKind::Capacity | ProviderSignalKind::Transport
            ) && default_cooldown_secs > 0)
                .then_some(default_cooldown_secs)
        })?;
        if cooldown_secs == 0 {
            return None;
        }

        let reason = signal
            .reason
            .clone()
            .or_else(|| signal.error_class.clone())
            .unwrap_or_else(|| format!("{:?}", signal.kind).to_ascii_lowercase());
        Some(Self {
            id: format!(
                "codex-helper:{}:{}",
                provider_endpoint_key.stable_key(),
                created_at_ms
            ),
            kind: PolicyActionKind::Cooldown,
            code: Some(PolicyActionKind::Cooldown.code().to_string()),
            owner: PolicyActionOwner::CodexHelper,
            provider_endpoint_key,
            source_signal: signal.clone(),
            reason,
            confidence: signal.confidence,
            created_at_ms,
            expires_at_ms: created_at_ms.saturating_add(cooldown_secs.saturating_mul(1000)),
            recovery_state: PolicyActionRecoveryState::Active,
            generation,
        })
    }

    pub fn is_active_at(&self, now_ms: u64) -> bool {
        self.recovery_state == PolicyActionRecoveryState::Active && now_ms < self.expires_at_ms
    }

    pub fn remaining_secs_at(&self, now_ms: u64) -> Option<u64> {
        self.is_active_at(now_ms)
            .then(|| self.expires_at_ms.saturating_sub(now_ms).div_ceil(1000))
    }

    pub fn stable_code(&self) -> &str {
        self.code.as_deref().unwrap_or_else(|| self.kind.code())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyActionProjection {
    pub provider_endpoint_key: ProviderEndpointKey,
    pub active_cooldown: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_remaining_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
}

impl PolicyActionProjection {
    pub fn from_action(action: &PolicyAction, now_ms: u64) -> Option<Self> {
        let cooldown_remaining_secs = action.remaining_secs_at(now_ms)?;
        Some(Self {
            provider_endpoint_key: action.provider_endpoint_key.clone(),
            active_cooldown: matches!(action.kind, PolicyActionKind::Cooldown),
            code: action
                .code
                .clone()
                .or_else(|| Some(action.kind.code().to_string())),
            cooldown_remaining_secs: Some(cooldown_remaining_secs),
            reason: Some(action.reason.clone()),
            action_id: Some(action.id.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_signals::{ProviderSignalSource, ProviderSignalTarget};

    fn quota_signal(reset_after_secs: Option<u64>) -> ProviderSignal {
        let mut signal = ProviderSignal::high_confidence_route_facing(
            ProviderSignalKind::Quota,
            ProviderSignalSource::UpstreamResponse,
            ProviderSignalTarget::ProviderEndpoint {
                provider_endpoint_key: ProviderEndpointKey::new("codex", "monthly", "default"),
            },
            100,
        );
        signal.reset_after_secs = reset_after_secs;
        signal.reason = Some("usage_limit_reached".to_string());
        signal
    }

    #[test]
    fn high_confidence_quota_signal_creates_owned_cooldown() {
        let action = PolicyAction::cooldown_from_signal(quota_signal(Some(30)), 1_000, 0, 7)
            .expect("cooldown action");

        assert_eq!(action.owner, PolicyActionOwner::CodexHelper);
        assert_eq!(action.code.as_deref(), Some("cooldown"));
        assert_eq!(action.expires_at_ms, 31_000);
        assert_eq!(action.generation, 7);
        assert!(action.is_active_at(30_999));
        assert!(!action.is_active_at(31_000));
    }

    #[test]
    fn quota_without_horizon_is_recorded_only() {
        assert!(PolicyAction::cooldown_from_signal(quota_signal(None), 1_000, 0, 1).is_none());
    }

    #[test]
    fn policy_action_projection_serializes_code() {
        let action = PolicyAction::cooldown_from_signal(quota_signal(Some(30)), 1_000, 0, 7)
            .expect("cooldown action");
        let projection = PolicyActionProjection::from_action(&action, 2_000).expect("projection");
        let value = serde_json::to_value(&projection).expect("serialize projection");

        assert_eq!(value["code"].as_str(), Some("cooldown"));
        assert_eq!(value["active_cooldown"].as_bool(), Some(true));
    }

    #[test]
    fn unknown_policy_action_kind_deserializes_as_unknown_code() {
        let kind: PolicyActionKind =
            serde_json::from_str("\"future_action\"").expect("deserialize action kind");

        assert_eq!(kind, PolicyActionKind::Unknown);
        assert_eq!(kind.code(), "unknown");
    }
}
