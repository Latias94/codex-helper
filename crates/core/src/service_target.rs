use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use uuid::Uuid;

use crate::config::ServiceKind;
use crate::credentials::{CredentialAggregateReadiness, CredentialName};
use crate::dashboard_core::OperatorReadModel;

pub const SERVICE_INSTALL_GENERATION_ENV_VAR: &str = "CODEX_HELPER_SERVICE_INSTALL_GENERATION";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("service install generation must be a canonical non-nil UUID")]
pub struct ServiceInstallGenerationError;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ServiceInstallGeneration(String);

impl ServiceInstallGeneration {
    pub fn generate() -> Self {
        Self(Uuid::new_v4().hyphenated().to_string())
    }

    pub fn parse(value: impl AsRef<str>) -> Result<Self, ServiceInstallGenerationError> {
        let value = value.as_ref();
        let parsed = Uuid::parse_str(value).map_err(|_| ServiceInstallGenerationError)?;
        let canonical = parsed.hyphenated().to_string();
        if parsed.is_nil() || value != canonical {
            return Err(ServiceInstallGenerationError);
        }
        Ok(Self(canonical))
    }

    pub fn from_process_env() -> Result<Option<Self>, ServiceInstallGenerationError> {
        match std::env::var(SERVICE_INSTALL_GENERATION_ENV_VAR) {
            Ok(value) => Self::parse(value).map(Some),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(ServiceInstallGenerationError),
        }
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for ServiceInstallGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ServiceInstallGeneration")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for ServiceInstallGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for ServiceInstallGeneration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ServiceInstallGeneration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalCredentialRefreshAction {
    Upsert,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalCredentialRefreshRequest {
    pub service: ServiceKind,
    pub install_generation: ServiceInstallGeneration,
    pub credential_name: CredentialName,
    pub action: LocalCredentialRefreshAction,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocalCredentialRefreshStatus {
    Published,
    Unchanged,
    NotReferenced,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalCredentialRefreshResponse {
    pub service: ServiceKind,
    pub install_generation: ServiceInstallGeneration,
    pub status: LocalCredentialRefreshStatus,
    pub runtime_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceRuntimeIdentity {
    pub service: ServiceKind,
    pub helper_home: PathBuf,
    pub client_home: PathBuf,
    pub install_generation: ServiceInstallGeneration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalServiceRuntimeReadRequest {
    pub service: ServiceKind,
    pub install_generation: ServiceInstallGeneration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalServiceRuntimeReadResponse {
    pub identity: ServiceRuntimeIdentity,
    pub credential_readiness: CredentialAggregateReadiness,
    pub operator: OperatorReadModel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_generation_requires_canonical_uuid() {
        let generation = ServiceInstallGeneration::generate();
        assert_eq!(
            ServiceInstallGeneration::parse(generation.as_str()).expect("canonical generation"),
            generation
        );
        assert!(ServiceInstallGeneration::parse(Uuid::nil().hyphenated().to_string()).is_err());
        assert!(ServiceInstallGeneration::parse(generation.as_str().to_ascii_uppercase()).is_err());
        assert!(ServiceInstallGeneration::parse("not-a-generation").is_err());
    }

    #[test]
    fn refresh_dto_contains_identity_but_no_credential_value() {
        let request = LocalCredentialRefreshRequest {
            service: ServiceKind::Codex,
            install_generation: ServiceInstallGeneration::generate(),
            credential_name: CredentialName::parse("relay.primary").expect("credential name"),
            action: LocalCredentialRefreshAction::Upsert,
        };

        let encoded = serde_json::to_string(&request).expect("serialize request");
        let decoded = serde_json::from_str::<LocalCredentialRefreshRequest>(&encoded)
            .expect("deserialize request");

        assert_eq!(decoded, request);
        assert!(!encoded.contains("credential_value"));
        assert!(!encoded.contains("fingerprint"));
    }

    #[test]
    fn service_runtime_read_contract_contains_only_non_secret_target_identity() {
        let request = LocalServiceRuntimeReadRequest {
            service: ServiceKind::Codex,
            install_generation: ServiceInstallGeneration::generate(),
        };

        let encoded = serde_json::to_string(&request).expect("serialize request");

        assert!(encoded.contains("install_generation"));
        assert!(!encoded.contains("credential_value"));
        assert!(!encoded.contains("fingerprint"));
    }
}
