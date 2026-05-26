use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProviderEndpointKey {
    pub service_name: String,
    pub provider_id: String,
    pub endpoint_id: String,
}

impl ProviderEndpointKey {
    pub fn new(
        service_name: impl Into<String>,
        provider_id: impl Into<String>,
        endpoint_id: impl Into<String>,
    ) -> Self {
        Self {
            service_name: service_name.into(),
            provider_id: provider_id.into(),
            endpoint_id: endpoint_id.into(),
        }
    }

    pub fn stable_key(&self) -> String {
        format!(
            "{}/{}/{}",
            self.service_name, self.provider_id, self.endpoint_id
        )
    }
}

impl fmt::Display for ProviderEndpointKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.stable_key().as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContinuityDomainKey {
    ProviderEndpoint {
        provider_endpoint: ProviderEndpointKey,
    },
    Explicit {
        service_name: String,
        domain: String,
    },
}

impl ContinuityDomainKey {
    pub fn provider_endpoint(provider_endpoint: ProviderEndpointKey) -> Self {
        Self::ProviderEndpoint { provider_endpoint }
    }

    pub fn explicit(service_name: impl Into<String>, domain: impl Into<String>) -> Option<Self> {
        let domain = domain.into();
        let domain = domain.trim();
        if domain.is_empty() {
            return None;
        }
        Some(Self::Explicit {
            service_name: service_name.into(),
            domain: domain.to_string(),
        })
    }

    pub fn stable_key(&self) -> String {
        match self {
            Self::ProviderEndpoint { provider_endpoint } => {
                format!("provider_endpoint:{}", provider_endpoint.stable_key())
            }
            Self::Explicit {
                service_name,
                domain,
            } => {
                format!("explicit:{service_name}/{domain}")
            }
        }
    }

    pub fn is_explicit(&self) -> bool {
        matches!(self, Self::Explicit { .. })
    }
}

impl fmt::Display for ContinuityDomainKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.stable_key().as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LegacyUpstreamKey {
    pub service_name: String,
    pub station_name: String,
    pub upstream_index: usize,
}

impl LegacyUpstreamKey {
    pub fn new(
        service_name: impl Into<String>,
        station_name: impl Into<String>,
        upstream_index: usize,
    ) -> Self {
        Self {
            service_name: service_name.into(),
            station_name: station_name.into(),
            upstream_index,
        }
    }

    pub fn stable_key(&self) -> String {
        format!(
            "{}/{}/{}",
            self.service_name, self.station_name, self.upstream_index
        )
    }
}

impl fmt::Display for LegacyUpstreamKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.stable_key().as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeUpstreamIdentity {
    pub provider_endpoint: ProviderEndpointKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<LegacyUpstreamKey>,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_domain: Option<String>,
}

impl RuntimeUpstreamIdentity {
    pub fn new(
        provider_endpoint: ProviderEndpointKey,
        compatibility: Option<LegacyUpstreamKey>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            provider_endpoint,
            compatibility,
            base_url: base_url.into(),
            continuity_domain: None,
        }
    }

    pub fn new_with_continuity_domain(
        provider_endpoint: ProviderEndpointKey,
        compatibility: Option<LegacyUpstreamKey>,
        base_url: impl Into<String>,
        continuity_domain: Option<String>,
    ) -> Self {
        Self {
            provider_endpoint,
            compatibility,
            base_url: base_url.into(),
            continuity_domain: continuity_domain
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeUpstreamCompatibilityChange {
    pub provider_endpoint: ProviderEndpointKey,
    pub previous_compatibility: Option<LegacyUpstreamKey>,
    pub current_compatibility: Option<LegacyUpstreamKey>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeUpstreamIdentityMigrationPlan {
    pub retained: Vec<RuntimeUpstreamIdentity>,
    pub added: Vec<RuntimeUpstreamIdentity>,
    pub removed: Vec<RuntimeUpstreamIdentity>,
    pub compatibility_changed: Vec<RuntimeUpstreamCompatibilityChange>,
}

pub fn plan_runtime_upstream_identity_migration(
    previous: &[RuntimeUpstreamIdentity],
    current: &[RuntimeUpstreamIdentity],
) -> RuntimeUpstreamIdentityMigrationPlan {
    let previous_by_endpoint = identities_by_provider_endpoint(previous);
    let current_by_endpoint = identities_by_provider_endpoint(current);
    let mut plan = RuntimeUpstreamIdentityMigrationPlan::default();

    for current_identity in current_by_endpoint.values() {
        match previous_by_endpoint.get(&current_identity.provider_endpoint) {
            Some(previous_identity)
                if previous_identity.base_url == current_identity.base_url
                    && previous_identity.continuity_domain
                        == current_identity.continuity_domain =>
            {
                plan.retained.push(current_identity.clone());
                if previous_identity.compatibility != current_identity.compatibility {
                    plan.compatibility_changed
                        .push(RuntimeUpstreamCompatibilityChange {
                            provider_endpoint: current_identity.provider_endpoint.clone(),
                            previous_compatibility: previous_identity.compatibility.clone(),
                            current_compatibility: current_identity.compatibility.clone(),
                        });
                }
            }
            _ => plan.added.push(current_identity.clone()),
        }
    }

    for previous_identity in previous_by_endpoint.values() {
        match current_by_endpoint.get(&previous_identity.provider_endpoint) {
            Some(current_identity)
                if current_identity.base_url == previous_identity.base_url
                    && current_identity.continuity_domain
                        == previous_identity.continuity_domain => {}
            _ => plan.removed.push(previous_identity.clone()),
        }
    }

    plan
}

fn identities_by_provider_endpoint(
    identities: &[RuntimeUpstreamIdentity],
) -> BTreeMap<ProviderEndpointKey, RuntimeUpstreamIdentity> {
    identities
        .iter()
        .map(|identity| (identity.provider_endpoint.clone(), identity.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_endpoint_key_uses_stable_service_provider_endpoint_shape() {
        let key = ProviderEndpointKey::new("codex", "openai", "default");

        assert_eq!(key.stable_key(), "codex/openai/default");
        assert_eq!(key.to_string(), "codex/openai/default");
    }

    #[test]
    fn legacy_upstream_key_uses_stable_service_station_index_shape() {
        let key = LegacyUpstreamKey::new("codex", "routing", 2);

        assert_eq!(key.stable_key(), "codex/routing/2");
        assert_eq!(key.to_string(), "codex/routing/2");
    }

    #[test]
    fn continuity_domain_key_distinguishes_default_endpoint_from_explicit_domain() {
        let endpoint = ProviderEndpointKey::new("codex", "relay-a", "default");
        let default_domain = ContinuityDomainKey::provider_endpoint(endpoint.clone());
        let explicit_domain =
            ContinuityDomainKey::explicit("codex", "shared-relay").expect("explicit domain");

        assert_eq!(
            default_domain.stable_key(),
            "provider_endpoint:codex/relay-a/default"
        );
        assert_eq!(explicit_domain.stable_key(), "explicit:codex/shared-relay");
        assert!(!default_domain.is_explicit());
        assert!(explicit_domain.is_explicit());
        assert_ne!(
            default_domain,
            ContinuityDomainKey::provider_endpoint(ProviderEndpointKey::new(
                "codex", "relay-b", "default"
            ))
        );
    }

    #[test]
    fn runtime_identity_serializes_both_target_and_compatibility_keys() {
        let identity = RuntimeUpstreamIdentity::new_with_continuity_domain(
            ProviderEndpointKey::new("codex", "openai", "default"),
            Some(LegacyUpstreamKey::new("codex", "routing", 0)),
            "https://api.openai.com/v1",
            Some("official-openai:acct-1".to_string()),
        );

        let value = serde_json::to_value(identity).expect("serialize identity");

        assert_eq!(
            value["provider_endpoint"]["provider_id"].as_str(),
            Some("openai")
        );
        assert_eq!(
            value["compatibility"]["station_name"].as_str(),
            Some("routing")
        );
        assert_eq!(value["compatibility"]["upstream_index"].as_u64(), Some(0));
        assert_eq!(
            value["base_url"].as_str(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            value["continuity_domain"].as_str(),
            Some("official-openai:acct-1")
        );
    }

    #[test]
    fn migration_plan_retains_provider_endpoint_state_across_legacy_index_changes() {
        let previous = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            Some(LegacyUpstreamKey::new("codex", "routing", 1)),
            "https://api.example/v1",
        )];
        let current = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            Some(LegacyUpstreamKey::new("codex", "routing", 0)),
            "https://api.example/v1",
        )];

        let plan = plan_runtime_upstream_identity_migration(&previous, &current);

        assert_eq!(plan.retained, current);
        assert!(plan.added.is_empty());
        assert!(plan.removed.is_empty());
        assert_eq!(plan.compatibility_changed.len(), 1);
        assert_eq!(
            plan.compatibility_changed[0].provider_endpoint.stable_key(),
            "codex/input/default"
        );
        assert_eq!(
            plan.compatibility_changed[0]
                .previous_compatibility
                .as_ref()
                .map(LegacyUpstreamKey::stable_key)
                .as_deref(),
            Some("codex/routing/1")
        );
        assert_eq!(
            plan.compatibility_changed[0]
                .current_compatibility
                .as_ref()
                .map(LegacyUpstreamKey::stable_key)
                .as_deref(),
            Some("codex/routing/0")
        );
    }

    #[test]
    fn migration_plan_replaces_provider_endpoint_state_when_base_url_changes() {
        let previous = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            Some(LegacyUpstreamKey::new("codex", "routing", 0)),
            "https://old.example/v1",
        )];
        let current = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            Some(LegacyUpstreamKey::new("codex", "routing", 0)),
            "https://new.example/v1",
        )];

        let plan = plan_runtime_upstream_identity_migration(&previous, &current);

        assert!(plan.retained.is_empty());
        assert_eq!(plan.added, current);
        assert_eq!(plan.removed, previous);
        assert!(plan.compatibility_changed.is_empty());
    }

    #[test]
    fn migration_plan_replaces_provider_endpoint_state_when_continuity_domain_changes() {
        let previous = vec![RuntimeUpstreamIdentity::new_with_continuity_domain(
            ProviderEndpointKey::new("codex", "input", "default"),
            Some(LegacyUpstreamKey::new("codex", "routing", 0)),
            "https://api.example/v1",
            Some("old-domain".to_string()),
        )];
        let current = vec![RuntimeUpstreamIdentity::new_with_continuity_domain(
            ProviderEndpointKey::new("codex", "input", "default"),
            Some(LegacyUpstreamKey::new("codex", "routing", 0)),
            "https://api.example/v1",
            Some("new-domain".to_string()),
        )];

        let plan = plan_runtime_upstream_identity_migration(&previous, &current);

        assert!(plan.retained.is_empty());
        assert_eq!(plan.added, current);
        assert_eq!(plan.removed, previous);
        assert!(plan.compatibility_changed.is_empty());
    }

    #[test]
    fn migration_plan_classifies_added_and_removed_provider_endpoints() {
        let previous = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "old", "default"),
            Some(LegacyUpstreamKey::new("codex", "routing", 0)),
            "https://old.example/v1",
        )];
        let current = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "new", "default"),
            Some(LegacyUpstreamKey::new("codex", "routing", 0)),
            "https://new.example/v1",
        )];

        let plan = plan_runtime_upstream_identity_migration(&previous, &current);

        assert!(plan.retained.is_empty());
        assert_eq!(plan.added, current);
        assert_eq!(plan.removed, previous);
        assert!(plan.compatibility_changed.is_empty());
    }
}
