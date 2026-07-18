use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::http::HeaderValue;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::runtime_identity::{ProviderEndpointKey, RuntimeUpstreamIdentity};

use super::{CredentialErrorCode, CredentialName, CredentialSourceKind, SecretValue};

const NATIVE_SOFT_REFRESH: Duration = Duration::from_secs(60);
pub(super) const NATIVE_HARD_EXPIRY: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct CredentialHandle([u8; 32]);

impl CredentialHandle {
    pub(super) fn for_descriptor(parts: &[&[u8]]) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"codex-helper/runtime-credential-handle/v1\0");
        for part in parts {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part);
        }
        Self(digest.finalize().into())
    }

    fn opaque(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NamedCredentialLookup {
    ServiceCredential,
    EnvironmentOnly,
}

impl NamedCredentialLookup {
    pub(super) fn descriptor_kind(self) -> &'static [u8] {
        match self {
            Self::ServiceCredential => b"named-service-credential",
            Self::EnvironmentOnly => b"environment-only",
        }
    }

    fn digest_label(self) -> &'static [u8] {
        match self {
            Self::ServiceCredential => b"service-credential",
            Self::EnvironmentOnly => b"environment-only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NamedCredentialReference {
    pub(crate) service_name: String,
    pub(crate) name: String,
    pub(crate) lookup: NamedCredentialLookup,
}

impl fmt::Debug for CredentialHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CredentialHandle")
            .field(&self.opaque())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeCredentialKind {
    Bearer,
    ApiKey,
}

impl RuntimeCredentialKind {
    pub(super) fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Bearer => b"bearer",
            Self::ApiKey => b"api-key",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Bearer => "Bearer token",
            Self::ApiKey => "X-API-Key",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CapturedCredentialError {
    kind: RuntimeCredentialKind,
    code: CredentialErrorCode,
    source_kind: &'static str,
    reference: String,
}

impl CapturedCredentialError {
    pub(crate) fn kind_label(&self) -> &'static str {
        self.kind.label()
    }

    pub(crate) fn code(&self) -> CredentialErrorCode {
        self.code
    }

    pub(crate) fn source_kind(&self) -> &'static str {
        self.source_kind
    }

    pub(crate) fn reference(&self) -> &str {
        self.reference.as_str()
    }
}

impl fmt::Debug for CapturedCredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapturedCredentialError")
            .field("kind", &self.kind)
            .field("code", &self.code)
            .field("source_kind", &self.source_kind)
            .field("reference", &self.reference)
            .finish()
    }
}

#[derive(Clone)]
enum CapturedCredentialPart {
    Unconfigured,
    Available(SecretValue),
    Unavailable(CapturedCredentialError),
}

impl fmt::Debug for CapturedCredentialPart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unconfigured => formatter.write_str("Unconfigured"),
            Self::Available(_) => formatter.write_str("Available(<redacted>)"),
            Self::Unavailable(error) => formatter.debug_tuple("Unavailable").field(error).finish(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct CapturedUpstreamCredential {
    auth_token: CapturedCredentialPart,
    api_key: CapturedCredentialPart,
    named_credentials: Arc<BTreeMap<(NamedCredentialLookup, String), SecretValue>>,
    configured_contract: bool,
    allow_anonymous: bool,
    handles: Arc<[CredentialHandle]>,
    named_catalog_revision: Arc<str>,
    generation: CredentialGenerationMarker,
}

impl CapturedUpstreamCredential {
    pub(crate) fn is_available(&self) -> bool {
        !matches!(self.auth_token, CapturedCredentialPart::Unavailable(_))
            && !matches!(self.api_key, CapturedCredentialPart::Unavailable(_))
    }

    pub(crate) fn configured_contract(&self) -> bool {
        self.configured_contract
    }

    pub(crate) fn allow_anonymous(&self) -> bool {
        self.allow_anonymous
    }

    pub(crate) fn first_error(&self) -> Option<&CapturedCredentialError> {
        match &self.auth_token {
            CapturedCredentialPart::Unavailable(error) => Some(error),
            CapturedCredentialPart::Unconfigured | CapturedCredentialPart::Available(_) => {
                match &self.api_key {
                    CapturedCredentialPart::Unavailable(error) => Some(error),
                    CapturedCredentialPart::Unconfigured | CapturedCredentialPart::Available(_) => {
                        None
                    }
                }
            }
        }
    }

    pub(crate) fn bearer_header(&self) -> Option<HeaderValue> {
        match &self.auth_token {
            CapturedCredentialPart::Available(value) => Some(value.sensitive_bearer_header_value()),
            CapturedCredentialPart::Unconfigured | CapturedCredentialPart::Unavailable(_) => None,
        }
    }

    pub(crate) fn api_key_header(&self) -> Option<HeaderValue> {
        match &self.api_key {
            CapturedCredentialPart::Available(value) => Some(value.sensitive_header_value()),
            CapturedCredentialPart::Unconfigured | CapturedCredentialPart::Unavailable(_) => None,
        }
    }

    pub(crate) fn preferred_usage_token(&self) -> Option<SecretValue> {
        if !self.is_available() {
            return None;
        }
        match &self.auth_token {
            CapturedCredentialPart::Available(value) => Some(value.clone()),
            CapturedCredentialPart::Unconfigured | CapturedCredentialPart::Unavailable(_) => {
                match &self.api_key {
                    CapturedCredentialPart::Available(value) => Some(value.clone()),
                    CapturedCredentialPart::Unconfigured
                    | CapturedCredentialPart::Unavailable(_) => None,
                }
            }
        }
    }

    pub(crate) fn named_credential(
        &self,
        lookup: NamedCredentialLookup,
        name: &str,
    ) -> Option<SecretValue> {
        self.named_credentials
            .iter()
            .find(|((candidate_lookup, candidate_name), _)| {
                *candidate_lookup == lookup && candidate_name == name
            })
            .map(|(_, value)| value)
            .cloned()
    }

    pub(crate) fn named_catalog_revision(&self) -> &str {
        self.named_catalog_revision.as_ref()
    }

    pub(crate) fn refresh_handles(&self) -> Arc<[CredentialHandle]> {
        Arc::clone(&self.handles)
    }

    pub(crate) fn generation_marker(&self) -> CredentialGenerationMarker {
        self.generation.clone()
    }
}

impl fmt::Debug for CapturedUpstreamCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapturedUpstreamCredential")
            .field("auth_token", &self.auth_token)
            .field("api_key", &self.api_key)
            .field("named_credential_count", &self.named_credentials.len())
            .field("configured_contract", &self.configured_contract)
            .field("allow_anonymous", &self.allow_anonymous)
            .field("source_count", &self.handles.len())
            .field("generation", &self.generation)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CredentialGenerationMarker {
    revision: u64,
    digest: String,
}

impl CredentialGenerationMarker {
    pub(crate) fn matches(&self, generation: &CredentialGeneration) -> bool {
        self.revision == generation.revision && self.digest == generation.digest
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CredentialSourceSpec {
    Static {
        source_kind: &'static str,
        reference: String,
    },
    Native {
        name: CredentialName,
    },
}

impl CredentialSourceSpec {
    pub(super) fn source_kind(&self) -> &'static str {
        match self {
            Self::Static { source_kind, .. } => source_kind,
            Self::Native { .. } => CredentialSourceKind::Native.as_str(),
        }
    }

    pub(super) fn reference(&self) -> &str {
        match self {
            Self::Static { reference, .. } => reference.as_str(),
            Self::Native { name } => name.as_str(),
        }
    }

    pub(super) fn is_native(&self) -> bool {
        matches!(self, Self::Native { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EndpointCredentialBinding {
    pub(super) auth_token: Option<CredentialHandle>,
    pub(super) api_key: Option<CredentialHandle>,
    pub(super) configured_contract: bool,
    pub(super) allow_anonymous: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CredentialCatalog {
    pub(super) sources: BTreeMap<CredentialHandle, CredentialSourceSpec>,
    pub(super) endpoints: BTreeMap<ProviderEndpointKey, EndpointCredentialBinding>,
    pub(super) named: BTreeMap<NamedCredentialReference, CredentialHandle>,
    pub(super) named_catalog_revision: Arc<str>,
}

#[derive(Clone)]
pub(super) struct CredentialLoadFailure {
    pub(super) code: CredentialErrorCode,
    pub(super) source_kind: &'static str,
    pub(super) reference: String,
}

impl fmt::Debug for CredentialLoadFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialLoadFailure")
            .field("code", &self.code)
            .field("source_kind", &self.source_kind)
            .field("reference", &self.reference)
            .finish()
    }
}

pub(super) type CredentialLoadResult = std::result::Result<SecretValue, CredentialLoadFailure>;

#[derive(Clone)]
pub(super) enum CredentialSourceState {
    Ready {
        value: SecretValue,
        loaded_at: Instant,
    },
    Stale {
        value: SecretValue,
        loaded_at: Instant,
        attempted_at: Instant,
        failure: CredentialLoadFailure,
    },
    Unavailable {
        attempted_at: Instant,
        failure: CredentialLoadFailure,
    },
}

impl CredentialSourceState {
    pub(super) fn value_at(&self, now: Instant) -> Option<&SecretValue> {
        match self {
            Self::Ready { value, .. } => Some(value),
            Self::Stale {
                value, loaded_at, ..
            } if now.saturating_duration_since(*loaded_at) < NATIVE_HARD_EXPIRY => Some(value),
            Self::Stale { .. } => None,
            Self::Unavailable { .. } => None,
        }
    }

    fn digest_code(&self) -> &'static str {
        match self {
            Self::Ready { .. } => "ready",
            Self::Stale { .. } => "stale",
            Self::Unavailable { failure, .. } => failure.code.as_str(),
        }
    }

    pub(super) fn next_deadline(&self) -> Instant {
        match self {
            Self::Ready { loaded_at, .. } => *loaded_at + NATIVE_SOFT_REFRESH,
            Self::Stale {
                loaded_at,
                attempted_at,
                ..
            } => std::cmp::min(
                *loaded_at + NATIVE_HARD_EXPIRY,
                *attempted_at + NATIVE_SOFT_REFRESH,
            ),
            Self::Unavailable { attempted_at, .. } => *attempted_at + NATIVE_SOFT_REFRESH,
        }
    }
}

impl fmt::Debug for CredentialSourceState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready { loaded_at, .. } => formatter
                .debug_struct("Ready")
                .field("value", &"<redacted>")
                .field("loaded_at", loaded_at)
                .finish(),
            Self::Stale {
                loaded_at,
                attempted_at,
                failure,
                ..
            } => formatter
                .debug_struct("Stale")
                .field("value", &"<redacted>")
                .field("loaded_at", loaded_at)
                .field("attempted_at", attempted_at)
                .field("failure", failure)
                .finish(),
            Self::Unavailable {
                attempted_at,
                failure,
            } => formatter
                .debug_struct("Unavailable")
                .field("attempted_at", attempted_at)
                .field("failure", failure)
                .finish(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct CredentialGeneration {
    pub(super) revision: u64,
    pub(super) digest: String,
    pub(super) catalog: Arc<CredentialCatalog>,
    pub(super) sources: Arc<BTreeMap<CredentialHandle, CredentialSourceState>>,
    pub(super) scopes: Arc<BTreeMap<ProviderEndpointKey, Option<String>>>,
}

impl CredentialGeneration {
    pub(crate) fn empty() -> Arc<Self> {
        Arc::new(Self {
            revision: 0,
            digest: "credential-generation-v1:empty".to_string(),
            catalog: Arc::new(CredentialCatalog::default()),
            sources: Arc::new(BTreeMap::new()),
            scopes: Arc::new(BTreeMap::new()),
        })
    }

    pub(super) fn capture(
        &self,
        provider_endpoint: &ProviderEndpointKey,
    ) -> CapturedUpstreamCredential {
        let Some(binding) = self.catalog.endpoints.get(provider_endpoint) else {
            return CapturedUpstreamCredential {
                auth_token: CapturedCredentialPart::Unconfigured,
                api_key: CapturedCredentialPart::Unconfigured,
                named_credentials: self.capture_named_credentials(&provider_endpoint.service_name),
                configured_contract: false,
                allow_anonymous: false,
                handles: Arc::from([]),
                named_catalog_revision: Arc::clone(&self.catalog.named_catalog_revision),
                generation: self.marker(),
            };
        };
        let mut handles = BTreeSet::new();
        let auth_token = self.capture_part(
            binding.auth_token.as_ref(),
            RuntimeCredentialKind::Bearer,
            &mut handles,
        );
        let api_key = self.capture_part(
            binding.api_key.as_ref(),
            RuntimeCredentialKind::ApiKey,
            &mut handles,
        );
        CapturedUpstreamCredential {
            auth_token,
            api_key,
            named_credentials: self.capture_named_credentials(&provider_endpoint.service_name),
            configured_contract: binding.configured_contract,
            allow_anonymous: binding.allow_anonymous,
            handles: handles.into_iter().collect::<Vec<_>>().into(),
            named_catalog_revision: Arc::clone(&self.catalog.named_catalog_revision),
            generation: self.marker(),
        }
    }

    fn capture_named_credentials(
        &self,
        service_name: &str,
    ) -> Arc<BTreeMap<(NamedCredentialLookup, String), SecretValue>> {
        let now = Instant::now();
        Arc::new(
            self.catalog
                .named
                .iter()
                .filter(|(reference, _)| reference.service_name == service_name)
                .filter_map(|(reference, handle)| {
                    self.sources
                        .get(handle)
                        .and_then(|state| state.value_at(now))
                        .map(|value| ((reference.lookup, reference.name.clone()), value.clone()))
                })
                .collect(),
        )
    }

    pub(crate) fn capture_bound(
        &self,
        provider_endpoint: &ProviderEndpointKey,
    ) -> Result<CapturedUpstreamCredential> {
        if !self.catalog.endpoints.contains_key(provider_endpoint) {
            anyhow::bail!(
                "credential generation {} has no binding for {}",
                self.revision,
                provider_endpoint
            );
        }
        Ok(self.capture(provider_endpoint))
    }

    pub(crate) fn bind_upstream_identity(
        &self,
        provider_endpoint: ProviderEndpointKey,
        base_url: impl Into<String>,
        continuity_domain: Option<String>,
    ) -> Result<RuntimeUpstreamIdentity> {
        let credential_scope = self.scopes.get(&provider_endpoint).ok_or_else(|| {
            anyhow::anyhow!(
                "credential generation {} has no identity binding for {}",
                self.revision,
                provider_endpoint
            )
        })?;
        Ok(RuntimeUpstreamIdentity::new_with_credential_scope(
            provider_endpoint,
            base_url,
            continuity_domain,
            credential_scope.clone(),
        ))
    }

    fn capture_part(
        &self,
        handle: Option<&CredentialHandle>,
        kind: RuntimeCredentialKind,
        handles: &mut BTreeSet<CredentialHandle>,
    ) -> CapturedCredentialPart {
        let Some(handle) = handle else {
            return CapturedCredentialPart::Unconfigured;
        };
        handles.insert(handle.clone());
        let Some(state) = self.sources.get(handle) else {
            return CapturedCredentialPart::Unavailable(CapturedCredentialError {
                kind,
                code: CredentialErrorCode::Invalid,
                source_kind: "runtime",
                reference: handle.opaque(),
            });
        };
        match state {
            CredentialSourceState::Ready { value, .. } => {
                CapturedCredentialPart::Available(value.clone())
            }
            CredentialSourceState::Stale {
                value, loaded_at, ..
            } if Instant::now().saturating_duration_since(*loaded_at) < NATIVE_HARD_EXPIRY => {
                CapturedCredentialPart::Available(value.clone())
            }
            CredentialSourceState::Stale { failure, .. }
            | CredentialSourceState::Unavailable { failure, .. } => {
                CapturedCredentialPart::Unavailable(CapturedCredentialError {
                    kind,
                    code: failure.code,
                    source_kind: failure.source_kind,
                    reference: failure.reference.clone(),
                })
            }
        }
    }

    pub(crate) fn credential_scope_for_route_digest(
        &self,
        provider_endpoint: &ProviderEndpointKey,
    ) -> Result<Option<&str>> {
        if self.revision == 0 && self.catalog.endpoints.is_empty() {
            return Ok(None);
        }
        self.scopes
            .get(provider_endpoint)
            .map(Option::as_deref)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "credential generation {} has no identity binding for {}",
                    self.revision,
                    provider_endpoint
                )
            })
    }

    #[cfg(test)]
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn digest(&self) -> &str {
        self.digest.as_str()
    }

    pub(crate) fn named_catalog_revision(&self) -> &str {
        self.catalog.named_catalog_revision.as_ref()
    }

    pub(crate) fn marker(&self) -> CredentialGenerationMarker {
        CredentialGenerationMarker {
            revision: self.revision,
            digest: self.digest.clone(),
        }
    }

    pub(crate) fn native_handles_for_name(&self, name: &CredentialName) -> Arc<[CredentialHandle]> {
        self.catalog
            .sources
            .iter()
            .filter_map(|(handle, source)| match source {
                CredentialSourceSpec::Native { name: source_name } if source_name == name => {
                    Some(handle.clone())
                }
                CredentialSourceSpec::Static { .. } | CredentialSourceSpec::Native { .. } => None,
            })
            .collect::<Vec<_>>()
            .into()
    }

    pub(crate) fn contains_handle(&self, handle: &CredentialHandle) -> bool {
        self.catalog.sources.contains_key(handle)
    }

    pub(crate) fn next_native_deadline(&self) -> Option<Instant> {
        self.catalog
            .sources
            .iter()
            .filter(|(_, source)| source.is_native())
            .filter_map(|(handle, _)| self.sources.get(handle))
            .map(CredentialSourceState::next_deadline)
            .min()
    }

    #[cfg(test)]
    pub(crate) fn aged_for_test(&self, age: Duration) -> Arc<Self> {
        let now = Instant::now();
        let sources = self
            .sources
            .iter()
            .map(|(handle, state)| {
                let state = match state {
                    CredentialSourceState::Ready { value, .. } => CredentialSourceState::Ready {
                        value: value.clone(),
                        loaded_at: now.checked_sub(age).unwrap_or(now),
                    },
                    CredentialSourceState::Stale {
                        value,
                        attempted_at,
                        failure,
                        ..
                    } => CredentialSourceState::Stale {
                        value: value.clone(),
                        loaded_at: now.checked_sub(age).unwrap_or(now),
                        attempted_at: *attempted_at,
                        failure: failure.clone(),
                    },
                    CredentialSourceState::Unavailable {
                        attempted_at,
                        failure,
                    } => CredentialSourceState::Unavailable {
                        attempted_at: *attempted_at,
                        failure: failure.clone(),
                    },
                };
                (handle.clone(), state)
            })
            .collect();
        Arc::new(Self {
            revision: self.revision,
            digest: self.digest.clone(),
            catalog: Arc::clone(&self.catalog),
            sources: Arc::new(sources),
            scopes: Arc::clone(&self.scopes),
        })
    }
}

impl fmt::Debug for CredentialGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialGeneration")
            .field("revision", &self.revision)
            .field("digest", &self.digest)
            .field("source_count", &self.sources.len())
            .field("endpoint_count", &self.catalog.endpoints.len())
            .field("named_credential_count", &self.catalog.named.len())
            .finish()
    }
}

pub(super) fn preserve_last_known_good(
    previous: &CredentialGeneration,
    next_catalog: &CredentialCatalog,
    handle: &CredentialHandle,
    now: Instant,
    failure: &CredentialLoadFailure,
) -> Option<CredentialSourceState> {
    let next_source = next_catalog.sources.get(handle)?;
    if !next_source.is_native() || previous.catalog.sources.get(handle) != Some(next_source) {
        return None;
    }
    match previous.sources.get(handle)? {
        CredentialSourceState::Ready { value, loaded_at }
        | CredentialSourceState::Stale {
            value, loaded_at, ..
        } if now.saturating_duration_since(*loaded_at) < NATIVE_HARD_EXPIRY => {
            Some(CredentialSourceState::Stale {
                value: value.clone(),
                loaded_at: *loaded_at,
                attempted_at: now,
                failure: failure.clone(),
            })
        }
        CredentialSourceState::Ready { .. }
        | CredentialSourceState::Stale { .. }
        | CredentialSourceState::Unavailable { .. } => None,
    }
}

pub(super) fn generation_digest(
    catalog: &CredentialCatalog,
    states: &BTreeMap<CredentialHandle, CredentialSourceState>,
    scopes: &BTreeMap<ProviderEndpointKey, Option<String>>,
    named_scopes: &BTreeMap<NamedCredentialReference, Option<String>>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"codex-helper/credential-generation/v1\0");
    digest.update(catalog.named_catalog_revision.as_bytes());
    for (handle, source) in &catalog.sources {
        digest.update(handle.0);
        digest.update(source.source_kind().as_bytes());
        digest.update(source.reference().as_bytes());
        if let Some(state) = states.get(handle) {
            digest.update(state.digest_code().as_bytes());
        }
    }
    for (endpoint, scope) in scopes {
        digest.update(endpoint.stable_key().as_bytes());
        match scope {
            Some(scope) => {
                digest.update([1]);
                digest.update(scope.as_bytes());
            }
            None => digest.update([0]),
        }
    }
    for (reference, scope) in named_scopes {
        digest.update(reference.service_name.as_bytes());
        digest.update([0]);
        digest.update(reference.name.as_bytes());
        digest.update([0]);
        digest.update(reference.lookup.digest_label());
        match scope {
            Some(scope) => {
                digest.update([1]);
                digest.update(scope.as_bytes());
            }
            None => digest.update([0]),
        }
    }
    format!("sha256:{:x}", digest.finalize())
}
