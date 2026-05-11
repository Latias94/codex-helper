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
}
