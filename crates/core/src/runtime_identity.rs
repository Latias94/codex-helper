use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeUpstreamIdentity {
    pub provider_endpoint: ProviderEndpointKey,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_scope: Option<String>,
}

impl RuntimeUpstreamIdentity {
    pub fn new(provider_endpoint: ProviderEndpointKey, base_url: impl Into<String>) -> Self {
        Self {
            provider_endpoint,
            base_url: base_url.into(),
            continuity_domain: None,
            credential_scope: None,
        }
    }

    pub fn new_with_continuity_domain(
        provider_endpoint: ProviderEndpointKey,
        base_url: impl Into<String>,
        continuity_domain: Option<String>,
    ) -> Self {
        Self {
            provider_endpoint,
            base_url: base_url.into(),
            continuity_domain: continuity_domain
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            credential_scope: None,
        }
    }

    pub(crate) fn new_with_credential_scope(
        provider_endpoint: ProviderEndpointKey,
        base_url: impl Into<String>,
        continuity_domain: Option<String>,
        credential_scope: Option<String>,
    ) -> Self {
        Self {
            provider_endpoint,
            base_url: base_url.into(),
            continuity_domain: continuity_domain
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            credential_scope,
        }
    }

    pub fn policy_route_scope(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"codex-helper:runtime-upstream-policy-scope:v2\0");
        let provider_endpoint = self.provider_endpoint.stable_key();
        for value in [provider_endpoint.as_bytes(), self.base_url.as_bytes()] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value);
        }
        match self.continuity_domain.as_deref() {
            Some(domain) => {
                digest.update([1]);
                digest.update((domain.len() as u64).to_be_bytes());
                digest.update(domain.as_bytes());
            }
            None => digest.update([0]),
        }
        match self.credential_scope.as_deref() {
            Some(scope) => {
                digest.update([1]);
                digest.update((scope.len() as u64).to_be_bytes());
                digest.update(scope.as_bytes());
            }
            None => digest.update([0]),
        }
        format!("sha256:{:x}", digest.finalize())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeUpstreamIdentityDelta {
    pub retained: Vec<RuntimeUpstreamIdentity>,
    pub added: Vec<RuntimeUpstreamIdentity>,
    pub removed: Vec<RuntimeUpstreamIdentity>,
}

pub fn diff_runtime_upstream_identities(
    previous: &[RuntimeUpstreamIdentity],
    current: &[RuntimeUpstreamIdentity],
) -> RuntimeUpstreamIdentityDelta {
    let previous_by_endpoint = identities_by_provider_endpoint(previous);
    let current_by_endpoint = identities_by_provider_endpoint(current);
    let mut delta = RuntimeUpstreamIdentityDelta::default();

    for current_identity in current_by_endpoint.values() {
        match previous_by_endpoint.get(&current_identity.provider_endpoint) {
            Some(previous_identity) if previous_identity == current_identity => {
                delta.retained.push(current_identity.clone());
            }
            _ => delta.added.push(current_identity.clone()),
        }
    }

    for previous_identity in previous_by_endpoint.values() {
        match current_by_endpoint.get(&previous_identity.provider_endpoint) {
            Some(current_identity) if current_identity == previous_identity => {}
            _ => delta.removed.push(previous_identity.clone()),
        }
    }

    delta
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
    fn runtime_identity_serializes_canonical_endpoint_and_continuity_facts() {
        let identity = RuntimeUpstreamIdentity::new_with_continuity_domain(
            ProviderEndpointKey::new("codex", "openai", "default"),
            "https://api.openai.com/v1",
            Some("official-openai:acct-1".to_string()),
        );

        let value = serde_json::to_value(identity).expect("serialize identity");

        assert_eq!(
            value["provider_endpoint"]["provider_id"].as_str(),
            Some("openai")
        );
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
    fn policy_route_scope_changes_with_origin_and_continuity_identity() {
        let endpoint = ProviderEndpointKey::new("codex", "openai", "default");
        let original = RuntimeUpstreamIdentity::new_with_continuity_domain(
            endpoint.clone(),
            "https://api.openai.com/v1",
            Some("account-a".to_string()),
        );
        let changed_origin = RuntimeUpstreamIdentity::new_with_continuity_domain(
            endpoint.clone(),
            "https://relay.example/v1",
            Some("account-a".to_string()),
        );
        let changed_continuity = RuntimeUpstreamIdentity::new_with_continuity_domain(
            endpoint,
            "https://api.openai.com/v1",
            Some("account-b".to_string()),
        );

        assert_eq!(original.policy_route_scope(), original.policy_route_scope());
        assert_ne!(
            original.policy_route_scope(),
            changed_origin.policy_route_scope()
        );
        assert_ne!(
            original.policy_route_scope(),
            changed_continuity.policy_route_scope()
        );
        assert!(original.policy_route_scope().starts_with("sha256:"));
        assert!(!original.policy_route_scope().contains("api.openai.com"));
    }

    #[test]
    fn identity_delta_retains_unchanged_provider_endpoint_state() {
        let previous = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            "https://api.example/v1",
        )];
        let current = previous.clone();

        let delta = diff_runtime_upstream_identities(&previous, &current);

        assert_eq!(delta.retained, current);
        assert!(delta.added.is_empty());
        assert!(delta.removed.is_empty());
    }

    #[test]
    fn identity_delta_replaces_provider_endpoint_state_when_base_url_changes() {
        let previous = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            "https://old.example/v1",
        )];
        let current = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "input", "default"),
            "https://new.example/v1",
        )];

        let delta = diff_runtime_upstream_identities(&previous, &current);

        assert!(delta.retained.is_empty());
        assert_eq!(delta.added, current);
        assert_eq!(delta.removed, previous);
    }

    #[test]
    fn identity_delta_replaces_provider_endpoint_state_when_continuity_domain_changes() {
        let previous = vec![RuntimeUpstreamIdentity::new_with_continuity_domain(
            ProviderEndpointKey::new("codex", "input", "default"),
            "https://api.example/v1",
            Some("old-domain".to_string()),
        )];
        let current = vec![RuntimeUpstreamIdentity::new_with_continuity_domain(
            ProviderEndpointKey::new("codex", "input", "default"),
            "https://api.example/v1",
            Some("new-domain".to_string()),
        )];

        let delta = diff_runtime_upstream_identities(&previous, &current);

        assert!(delta.retained.is_empty());
        assert_eq!(delta.added, current);
        assert_eq!(delta.removed, previous);
    }

    #[test]
    fn identity_delta_replaces_provider_endpoint_state_when_credential_scope_changes() {
        let endpoint = ProviderEndpointKey::new("codex", "input", "default");
        let previous = vec![RuntimeUpstreamIdentity::new_with_credential_scope(
            endpoint.clone(),
            "https://api.example/v1",
            None,
            Some("hmac-sha256-v1:scope-a".to_string()),
        )];
        let current = vec![RuntimeUpstreamIdentity::new_with_credential_scope(
            endpoint,
            "https://api.example/v1",
            None,
            Some("hmac-sha256-v1:scope-b".to_string()),
        )];

        let delta = diff_runtime_upstream_identities(&previous, &current);

        assert!(delta.retained.is_empty());
        assert_eq!(delta.added, current);
        assert_eq!(delta.removed, previous);
        let serialized = serde_json::to_string(&(&delta.added, &delta.removed))
            .expect("serialize safe identity collections");
        assert!(!serialized.contains("account-a-secret"));
        assert!(!serialized.contains("account-b-secret"));
    }

    #[test]
    fn identity_delta_classifies_added_and_removed_provider_endpoints() {
        let previous = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "old", "default"),
            "https://old.example/v1",
        )];
        let current = vec![RuntimeUpstreamIdentity::new(
            ProviderEndpointKey::new("codex", "new", "default"),
            "https://new.example/v1",
        )];

        let delta = diff_runtime_upstream_identities(&previous, &current);

        assert!(delta.retained.is_empty());
        assert_eq!(delta.added, current);
        assert_eq!(delta.removed, previous);
    }
}
