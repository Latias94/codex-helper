use serde::{Deserialize, Serialize};

use crate::policy_actions::PolicyActionProjection;
use crate::state::RuntimeConfigState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlProfileOption {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default)]
    pub fast_mode: bool,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderEndpointOption {
    pub provider_name: String,
    pub name: String,
    #[serde(default)]
    pub provider_endpoint_key: String,
    pub base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuity_domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_continuity_domain: Option<String>,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub configured_enabled: bool,
    #[serde(default)]
    pub effective_enabled: bool,
    #[serde(default)]
    pub routable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_enabled_override: Option<bool>,
    #[serde(default)]
    pub runtime_state: RuntimeConfigState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_state_override: Option<RuntimeConfigState>,
    #[serde(default, skip_serializing_if = "ProviderCapacity::is_empty")]
    pub capacity: ProviderCapacity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_actions: Vec<PolicyActionProjection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderOption {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default)]
    pub configured_enabled: bool,
    #[serde(default)]
    pub effective_enabled: bool,
    #[serde(default)]
    pub routable_endpoints: usize,
    #[serde(default)]
    pub endpoints: Vec<ProviderEndpointOption>,
    #[serde(default, skip_serializing_if = "ProviderCapacity::is_empty")]
    pub capacity: ProviderCapacity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProviderCapacity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_max_concurrent_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_limit_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_max_concurrent_requests: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_limit_group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_key: Option<String>,
    #[serde(default)]
    pub saturated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherited_from_provider: Option<bool>,
}

impl ProviderCapacity {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub fn runtime_label(&self) -> Option<String> {
        self.capacity_parts(" ", true)
            .map(|parts| format!("capacity {parts}"))
    }

    pub fn compact_runtime_label(&self) -> String {
        self.capacity_parts(",", false)
            .map(|parts| format!("capacity={parts}"))
            .unwrap_or_else(|| "capacity=-".to_string())
    }

    fn capacity_parts(&self, separator: &str, include_configured: bool) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        match (self.active, self.limit) {
            (Some(active), Some(limit)) => parts.push(format!("active={active}/{limit}")),
            (None, Some(limit)) => parts.push(format!("limit={limit}")),
            _ => {}
        }
        if include_configured && let Some(configured) = self.configured_max_concurrent_requests {
            parts.push(format!("configured={configured}"));
        }
        if let Some(group) = self
            .effective_limit_group
            .as_deref()
            .map(str::trim)
            .filter(|group| !group.is_empty())
        {
            parts.push(format!("group={group}"));
        }
        if self.inherited_from_provider == Some(true) {
            parts.push("inherited".to_string());
        }
        if self.saturated {
            parts.push("saturated".to_string());
        }
        (!parts.is_empty()).then(|| parts.join(separator))
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderCapacity;

    #[test]
    fn provider_capacity_runtime_label_reports_configured_inherited_and_saturated_state() {
        let capacity = ProviderCapacity {
            configured_max_concurrent_requests: Some(3),
            configured_limit_group: Some("relay".to_string()),
            effective_max_concurrent_requests: Some(3),
            effective_limit_group: Some("relay".to_string()),
            active: Some(3),
            limit: Some(3),
            limit_key: Some("codex/relay".to_string()),
            saturated: true,
            inherited_from_provider: Some(true),
        };

        assert_eq!(
            capacity.runtime_label().as_deref(),
            Some("capacity active=3/3 configured=3 group=relay inherited saturated")
        );
    }

    #[test]
    fn provider_capacity_compact_runtime_label_keeps_route_preview_shape() {
        let capacity = ProviderCapacity {
            effective_max_concurrent_requests: Some(2),
            effective_limit_group: Some("shared".to_string()),
            active: Some(1),
            limit: Some(2),
            limit_key: Some("codex/shared".to_string()),
            inherited_from_provider: Some(true),
            ..ProviderCapacity::default()
        };

        assert_eq!(
            capacity.compact_runtime_label(),
            "capacity=active=1/2,group=shared,inherited"
        );
        assert_eq!(
            ProviderCapacity::default().compact_runtime_label(),
            "capacity=-"
        );
        assert_eq!(ProviderCapacity::default().runtime_label(), None);
    }
}
