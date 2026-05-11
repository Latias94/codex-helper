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
    pub legacy: LegacyUpstreamKey,
    pub base_url: String,
}

impl RuntimeUpstreamIdentity {
    pub fn new(
        provider_endpoint: ProviderEndpointKey,
        legacy: LegacyUpstreamKey,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            provider_endpoint,
            legacy,
            base_url: base_url.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeUpstreamCompatibilityChange {
    pub provider_endpoint: ProviderEndpointKey,
    pub previous_legacy: LegacyUpstreamKey,
    pub current_legacy: LegacyUpstreamKey,
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
            Some(previous_identity) if previous_identity.base_url == current_identity.base_url => {
                plan.retained.push(current_identity.clone());
                if previous_identity.legacy != current_identity.legacy {
                    plan.compatibility_changed
                        .push(RuntimeUpstreamCompatibilityChange {
                            provider_endpoint: current_identity.provider_endpoint.clone(),
                            previous_legacy: previous_identity.legacy.clone(),
                            current_legacy: current_identity.legacy.clone(),
                        });
                }
            }
            _ => plan.added.push(current_identity.clone()),
        }
    }

    for previous_identity in previous_by_endpoint.values() {
        match current_by_endpoint.get(&previous_identity.provider_endpoint) {
            Some(current_identity) if current_identity.base_url == previous_identity.base_url => {}
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
    fn runtime_identity_serializes_both_target_and_compatibility_keys() {
        let identity = RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "openai", "default"),
            LegacyUpstreamKey::new("codex", "routing", 0),
            "https://api.openai.com/v1",
        );

        let value = serde_json::to_value(identity).expect("serialize identity");

        assert_eq!(
            value["provider_endpoint"]["provider_id"].as_str(),
            Some("openai")
        );
        assert_eq!(value["legacy"]["station_name"].as_str(), Some("routing"));
        assert_eq!(value["legacy"]["upstream_index"].as_u64(), Some(0));
        assert_eq!(
            value["base_url"].as_str(),
            Some("https://api.openai.com/v1")
        );
    }

    #[test]
    fn migration_plan_retains_provider_endpoint_state_across_legacy_index_changes() {
        let previous = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            LegacyUpstreamKey::new("codex", "routing", 1),
            "https://api.example/v1",
        )];
        let current = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            LegacyUpstreamKey::new("codex", "routing", 0),
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
            plan.compatibility_changed[0].previous_legacy.stable_key(),
            "codex/routing/1"
        );
        assert_eq!(
            plan.compatibility_changed[0].current_legacy.stable_key(),
            "codex/routing/0"
        );
    }

    #[test]
    fn migration_plan_replaces_provider_endpoint_state_when_base_url_changes() {
        let previous = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            LegacyUpstreamKey::new("codex", "routing", 0),
            "https://old.example/v1",
        )];
        let current = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            LegacyUpstreamKey::new("codex", "routing", 0),
            "https://new.example/v1",
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
            LegacyUpstreamKey::new("codex", "routing", 0),
            "https://old.example/v1",
        )];
        let current = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "new", "default"),
            LegacyUpstreamKey::new("codex", "routing", 0),
            "https://new.example/v1",
        )];

        let plan = plan_runtime_upstream_identity_migration(&previous, &current);

        assert!(plan.retained.is_empty());
        assert_eq!(plan.added, current);
        assert_eq!(plan.removed, previous);
        assert!(plan.compatibility_changed.is_empty());
    }
}
