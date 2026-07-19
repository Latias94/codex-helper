use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::usage::CacheAccountingConvention;

pub const OPENAI_CODEX_SOURCE_REVISION: &str = "3380969a29134630d56feb6218e8e8dcc5e8196d";
pub const OPENAI_CODEX_SOURCE_UPDATED_AT: &str = "2026-07-09T03:40:08Z";
pub const OPENAI_CODEX_CATALOG_REVISION: &str =
    "codex-models:3380969a29134630d56feb6218e8e8dcc5e8196d";
pub const OPENAI_CODEX_COMP_HASH: &str = "3000";
pub const OPENAI_GPT_5_6_PRICING_REVISION: &str =
    "openai-api-pricing:gpt-5.6:standard-priority:2026-07-10";
pub const OPENAI_GPT_5_6_PRICING_SOURCE: &str = "https://developers.openai.com/api/docs/pricing";
pub const OPENAI_GPT_5_6_PRICING_CAPTURED_AT: &str = "2026-07-10";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAdapter {
    OpenAiCodex,
    AwsBedrock,
    OpenAiCompatible,
}

impl ProviderAdapter {
    pub fn for_endpoint(endpoint: &reqwest::Url) -> Self {
        let host = endpoint
            .host_str()
            .unwrap_or_default()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if host == "api.openai.com" {
            return Self::OpenAiCodex;
        }

        let bedrock_host =
            host.starts_with("bedrock-runtime.") || host.starts_with("bedrock-runtime-fips.");
        let aws_domain = host.ends_with(".amazonaws.com")
            || host.ends_with(".amazonaws.com.cn")
            || host.ends_with(".api.aws");
        if bedrock_host && aws_domain {
            Self::AwsBedrock
        } else {
            Self::OpenAiCompatible
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccountFingerprint([u8; 32]);

impl AccountFingerprint {
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    pub(crate) const fn from_keyed_account_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    pub(crate) fn unscoped() -> Self {
        let mut digest = Sha256::new();
        digest.update(b"codex-helper:provider-account:credential-scope:v2\0");
        digest.update([0]);
        Self(digest.finalize().into())
    }

    /// Maps an installation-keyed opaque credential scope into the legacy persisted shape.
    /// Request headers and credential values must never be used as this input.
    pub(crate) fn from_credential_scope(credential_scope: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"codex-helper:provider-account:credential-scope:v2\0");
        digest.update([1]);
        digest.update((credential_scope.len() as u64).to_be_bytes());
        digest.update(credential_scope.as_bytes());
        Self(digest.finalize().into())
    }
}

impl std::fmt::Debug for AccountFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AccountFingerprint([redacted])")
    }
}

impl std::fmt::Display for AccountFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("sha256:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Serialize for AccountFingerprint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ProviderCatalogScope {
    adapter: ProviderAdapter,
    endpoint_origin: String,
    route_scope: String,
    account_fingerprint: AccountFingerprint,
    config_revision: String,
}

impl ProviderCatalogScope {
    pub fn new(
        adapter: ProviderAdapter,
        endpoint: impl AsRef<str>,
        route_scope: impl AsRef<str>,
        account_fingerprint: AccountFingerprint,
        config_revision: impl AsRef<str>,
    ) -> Result<Self, ProviderCatalogError> {
        let endpoint = reqwest::Url::parse(endpoint.as_ref().trim())
            .map_err(|_| ProviderCatalogError::InvalidEndpointOrigin)?;
        if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host_str().is_none() {
            return Err(ProviderCatalogError::InvalidEndpointOrigin);
        }
        if !endpoint.username().is_empty() || endpoint.password().is_some() {
            return Err(ProviderCatalogError::EndpointCredentialsNotAllowed);
        }
        let endpoint_origin = endpoint.origin().ascii_serialization();
        if endpoint_origin == "null" {
            return Err(ProviderCatalogError::InvalidEndpointOrigin);
        }

        let route_scope = route_scope.as_ref().trim();
        if route_scope.is_empty() {
            return Err(ProviderCatalogError::EmptyRouteScope);
        }
        let config_revision = config_revision.as_ref().trim();
        if config_revision.is_empty() {
            return Err(ProviderCatalogError::InvalidConfigRevision);
        }
        Ok(Self {
            adapter,
            endpoint_origin,
            route_scope: route_scope.to_string(),
            account_fingerprint,
            config_revision: config_revision.to_string(),
        })
    }

    pub fn adapter(&self) -> ProviderAdapter {
        self.adapter
    }

    pub fn endpoint_origin(&self) -> &str {
        &self.endpoint_origin
    }

    pub fn route_scope(&self) -> &str {
        &self.route_scope
    }

    pub fn account_fingerprint(&self) -> AccountFingerprint {
        self.account_fingerprint
    }

    pub fn config_revision(&self) -> &str {
        &self.config_revision
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCatalogAuthority {
    CodexModelsRepository,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCatalogFreshness {
    BundledSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderCatalogProvenance {
    authority: ProviderCatalogAuthority,
    freshness: ProviderCatalogFreshness,
    source_revision: String,
    source_updated_at: String,
}

impl ProviderCatalogProvenance {
    pub fn authority(&self) -> ProviderCatalogAuthority {
        self.authority
    }

    pub fn freshness(&self) -> ProviderCatalogFreshness {
        self.freshness
    }

    pub fn source_revision(&self) -> &str {
        &self.source_revision
    }

    pub fn source_updated_at(&self) -> &str {
        &self.source_updated_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPricingTier {
    Standard,
    Priority,
    Flex,
    Batch,
    Regional,
    Unknown,
}

impl ProviderPricingTier {
    pub fn from_actual_service_tier(actual: Option<&str>) -> Self {
        match actual
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("default" | "standard") => Self::Standard,
            Some("priority") => Self::Priority,
            _ => Self::Unknown,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Priority => "priority",
            Self::Flex => "flex",
            Self::Batch => "batch",
            Self::Regional => "regional",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPricingAuthority {
    OpenAiApiPricing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderPricingProvenance {
    authority: ProviderPricingAuthority,
    source: String,
    captured_at: String,
}

impl ProviderPricingProvenance {
    pub fn authority(&self) -> ProviderPricingAuthority {
        self.authority
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn captured_at(&self) -> &str {
        &self.captured_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ProviderPricingRevision(String);

impl ProviderPricingRevision {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderTokenPriceFacts {
    input_usd: Option<String>,
    cache_read_usd: Option<String>,
    cache_write_usd: Option<String>,
    output_usd: Option<String>,
}

impl ProviderTokenPriceFacts {
    pub fn input_usd(&self) -> Option<&str> {
        self.input_usd.as_deref()
    }

    pub fn cache_read_usd(&self) -> Option<&str> {
        self.cache_read_usd.as_deref()
    }

    pub fn cache_write_usd(&self) -> Option<&str> {
        self.cache_write_usd.as_deref()
    }

    pub fn output_usd(&self) -> Option<&str> {
        self.output_usd.as_deref()
    }

    fn complete(input: &str, cache_read: &str, cache_write: &str, output: &str) -> Self {
        Self {
            input_usd: Some(input.to_string()),
            cache_read_usd: Some(cache_read.to_string()),
            cache_write_usd: Some(cache_write.to_string()),
            output_usd: Some(output.to_string()),
        }
    }

    fn is_complete(&self) -> bool {
        self.input_usd.is_some()
            && self.cache_read_usd.is_some()
            && self.cache_write_usd.is_some()
            && self.output_usd.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProviderModelPricing {
    standard: Option<ProviderTokenPriceFacts>,
    priority: Option<ProviderTokenPriceFacts>,
    cache_accounting_convention: CacheAccountingConvention,
}

impl ProviderModelPricing {
    fn price(&self, tier: ProviderPricingTier) -> Option<&ProviderTokenPriceFacts> {
        match tier {
            ProviderPricingTier::Standard => self.standard.as_ref(),
            ProviderPricingTier::Priority => self.priority.as_ref(),
            ProviderPricingTier::Flex
            | ProviderPricingTier::Batch
            | ProviderPricingTier::Regional
            | ProviderPricingTier::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProviderPricingCatalog {
    revision: ProviderPricingRevision,
    provenance: ProviderPricingProvenance,
    models: BTreeMap<String, ProviderModelPricing>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ProviderPriceKey {
    scope: ProviderCatalogScope,
    catalog_revision: ProviderCatalogRevision,
    pricing_revision: ProviderPricingRevision,
    model: String,
    tier: ProviderPricingTier,
}

impl ProviderPriceKey {
    pub fn scope(&self) -> &ProviderCatalogScope {
        &self.scope
    }

    pub fn catalog_revision(&self) -> &ProviderCatalogRevision {
        &self.catalog_revision
    }

    pub fn pricing_revision(&self) -> &ProviderPricingRevision {
        &self.pricing_revision
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn tier(&self) -> ProviderPricingTier {
        self.tier
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderPriceQuote {
    key: ProviderPriceKey,
    pricing_revision: ProviderPricingRevision,
    pricing_provenance: ProviderPricingProvenance,
    prices_per_million: ProviderTokenPriceFacts,
    cache_accounting_convention: CacheAccountingConvention,
}

impl ProviderPriceQuote {
    pub fn key(&self) -> &ProviderPriceKey {
        &self.key
    }

    pub fn pricing_revision(&self) -> &ProviderPricingRevision {
        &self.pricing_revision
    }

    pub fn pricing_provenance(&self) -> &ProviderPricingProvenance {
        &self.pricing_provenance
    }

    pub fn prices_per_million(&self) -> &ProviderTokenPriceFacts {
        &self.prices_per_million
    }

    pub fn cache_accounting_convention(&self) -> CacheAccountingConvention {
        self.cache_accounting_convention
    }

    pub fn source_label(&self) -> String {
        format!(
            "provider-catalog:{}:{}:{}:{}:{}:{}",
            self.key.scope.endpoint_origin(),
            self.key.scope.route_scope(),
            self.key.catalog_revision.as_str(),
            self.key.pricing_revision.as_str(),
            self.key.model,
            self.key.tier.as_str(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogReasoningEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}

impl CatalogReasoningEffort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogToolMode {
    CodeModeOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogMultiAgentVersion {
    V1,
    V2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogClientVersion {
    major: u16,
    minor: u16,
    patch: u16,
}

impl CatalogClientVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl std::fmt::Display for CatalogClientVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Serialize for CatalogClientVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderModelCapabilities {
    slug: String,
    display_name: String,
    description: String,
    listing_priority: i32,
    context_window: i64,
    max_context_window: i64,
    default_reasoning_effort: CatalogReasoningEffort,
    supported_reasoning_efforts: Vec<CatalogReasoningEffort>,
    supports_priority_service_tier: bool,
    uses_responses_lite: bool,
    prefers_websockets: bool,
    tool_mode: CatalogToolMode,
    multi_agent_version: CatalogMultiAgentVersion,
    supports_parallel_tool_calls: bool,
    supports_image_detail_original: bool,
    minimum_codex_client_version: CatalogClientVersion,
    comp_hash: String,
}

impl ProviderModelCapabilities {
    pub fn slug(&self) -> &str {
        &self.slug
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn listing_priority(&self) -> i32 {
        self.listing_priority
    }

    pub fn context_window(&self) -> i64 {
        self.context_window
    }

    pub fn max_context_window(&self) -> i64 {
        self.max_context_window
    }

    pub fn default_reasoning_effort(&self) -> CatalogReasoningEffort {
        self.default_reasoning_effort
    }

    pub fn supported_reasoning_efforts(&self) -> &[CatalogReasoningEffort] {
        &self.supported_reasoning_efforts
    }

    pub fn supports_priority_service_tier(&self) -> bool {
        self.supports_priority_service_tier
    }

    pub fn uses_responses_lite(&self) -> bool {
        self.uses_responses_lite
    }

    pub fn prefers_websockets(&self) -> bool {
        self.prefers_websockets
    }

    pub fn tool_mode(&self) -> CatalogToolMode {
        self.tool_mode
    }

    pub fn multi_agent_version(&self) -> CatalogMultiAgentVersion {
        self.multi_agent_version
    }

    pub fn supports_parallel_tool_calls(&self) -> bool {
        self.supports_parallel_tool_calls
    }

    pub fn supports_image_detail_original(&self) -> bool {
        self.supports_image_detail_original
    }

    pub fn minimum_codex_client_version(&self) -> CatalogClientVersion {
        self.minimum_codex_client_version
    }

    pub fn comp_hash(&self) -> &str {
        &self.comp_hash
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ProviderCatalogRevision(String);

impl ProviderCatalogRevision {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelRequestContract {
    scope: ProviderCatalogScope,
    catalog_revision: ProviderCatalogRevision,
    model: String,
    ultra_maps_to_max: bool,
}

impl ProviderModelRequestContract {
    pub fn scope(&self) -> &ProviderCatalogScope {
        &self.scope
    }

    pub fn catalog_revision(&self) -> &ProviderCatalogRevision {
        &self.catalog_revision
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn ultra_maps_to_max(&self) -> bool {
        self.ultra_maps_to_max
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ProviderCatalogFacts {
    revision: ProviderCatalogRevision,
    provenance: ProviderCatalogProvenance,
    models: BTreeMap<String, ProviderModelCapabilities>,
    pricing: ProviderPricingCatalog,
}

/// Scope-independent provider facts captured as part of one runtime snapshot.
#[derive(Debug, Clone)]
pub struct ProviderCatalogSnapshot {
    openai_codex: Arc<ProviderCatalogFacts>,
}

impl ProviderCatalogSnapshot {
    pub fn bundled() -> Self {
        Self {
            openai_codex: bundled_openai_codex_facts(),
        }
    }

    pub fn catalog_revision(&self) -> &ProviderCatalogRevision {
        &self.openai_codex.revision
    }

    pub fn pricing_revision(&self) -> &ProviderPricingRevision {
        &self.openai_codex.pricing.revision
    }

    pub fn capture_epoch(
        &self,
        scope: ProviderCatalogScope,
    ) -> Result<ProviderCatalogEpoch, ProviderCatalogError> {
        if scope.adapter != ProviderAdapter::OpenAiCodex {
            return Err(ProviderCatalogError::AdapterMismatch {
                expected: ProviderAdapter::OpenAiCodex,
                actual: scope.adapter,
            });
        }
        Ok(ProviderCatalogEpoch {
            scope,
            facts: Arc::clone(&self.openai_codex),
        })
    }
}

impl Default for ProviderCatalogSnapshot {
    fn default() -> Self {
        Self::bundled()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCatalogEpoch {
    scope: ProviderCatalogScope,
    facts: Arc<ProviderCatalogFacts>,
}

impl Serialize for ProviderCatalogEpoch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ProviderCatalogEpoch", 5)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("revision", &self.facts.revision)?;
        state.serialize_field("provenance", &self.facts.provenance)?;
        state.serialize_field("models", &self.facts.models)?;
        state.serialize_field("pricing", &self.facts.pricing)?;
        state.end()
    }
}

impl ProviderCatalogEpoch {
    pub fn bundled_openai_codex(scope: ProviderCatalogScope) -> Result<Self, ProviderCatalogError> {
        ProviderCatalogSnapshot::bundled().capture_epoch(scope)
    }

    pub fn scope(&self) -> &ProviderCatalogScope {
        &self.scope
    }

    pub fn revision(&self) -> &ProviderCatalogRevision {
        &self.facts.revision
    }

    pub fn provenance(&self) -> &ProviderCatalogProvenance {
        &self.facts.provenance
    }

    pub fn model(&self, slug: &str) -> Option<&ProviderModelCapabilities> {
        self.facts.models.get(&slug.trim().to_ascii_lowercase())
    }

    pub fn models(&self) -> impl ExactSizeIterator<Item = &ProviderModelCapabilities> {
        self.facts.models.values()
    }

    pub fn capture_model_request_contract(
        &self,
        effective_model: &str,
    ) -> Option<ProviderModelRequestContract> {
        let model = effective_model.trim().to_ascii_lowercase();
        let capabilities = self.facts.models.get(&model)?;
        let supports_max = capabilities
            .supported_reasoning_efforts
            .contains(&CatalogReasoningEffort::Max);
        let supports_ultra = capabilities
            .supported_reasoning_efforts
            .contains(&CatalogReasoningEffort::Ultra);

        Some(ProviderModelRequestContract {
            scope: self.scope.clone(),
            catalog_revision: self.facts.revision.clone(),
            model,
            ultra_maps_to_max: supports_max && supports_ultra,
        })
    }

    pub fn pricing_revision(&self) -> &ProviderPricingRevision {
        &self.facts.pricing.revision
    }

    pub fn pricing_provenance(&self) -> &ProviderPricingProvenance {
        &self.facts.pricing.provenance
    }

    pub fn capture_price_key(&self, model: &str, tier: ProviderPricingTier) -> ProviderPriceKey {
        ProviderPriceKey {
            scope: self.scope.clone(),
            catalog_revision: self.facts.revision.clone(),
            pricing_revision: self.facts.pricing.revision.clone(),
            model: model.trim().to_ascii_lowercase(),
            tier,
        }
    }

    pub fn price_quote(&self, key: &ProviderPriceKey) -> Option<ProviderPriceQuote> {
        if key.scope != self.scope
            || key.catalog_revision != self.facts.revision
            || key.pricing_revision != self.facts.pricing.revision
        {
            return None;
        }
        if !self.facts.models.contains_key(&key.model) {
            return None;
        }
        let model = self.facts.pricing.models.get(&key.model)?;
        let prices = model.price(key.tier)?.clone();
        if !prices.is_complete() {
            return None;
        }

        Some(ProviderPriceQuote {
            key: key.clone(),
            pricing_revision: self.facts.pricing.revision.clone(),
            pricing_provenance: self.facts.pricing.provenance.clone(),
            prices_per_million: prices,
            cache_accounting_convention: model.cache_accounting_convention,
        })
    }
}

fn bundled_openai_codex_facts() -> Arc<ProviderCatalogFacts> {
    static FACTS: OnceLock<Arc<ProviderCatalogFacts>> = OnceLock::new();
    Arc::clone(FACTS.get_or_init(|| {
        let models = [
            gpt_5_6_model(Gpt56Variant::Sol),
            gpt_5_6_model(Gpt56Variant::Terra),
            gpt_5_6_model(Gpt56Variant::Luna),
        ]
        .into_iter()
        .map(|model| (model.slug.to_ascii_lowercase(), model))
        .collect();
        Arc::new(ProviderCatalogFacts {
            revision: ProviderCatalogRevision(OPENAI_CODEX_CATALOG_REVISION.to_string()),
            provenance: ProviderCatalogProvenance {
                authority: ProviderCatalogAuthority::CodexModelsRepository,
                freshness: ProviderCatalogFreshness::BundledSnapshot,
                source_revision: OPENAI_CODEX_SOURCE_REVISION.to_string(),
                source_updated_at: OPENAI_CODEX_SOURCE_UPDATED_AT.to_string(),
            },
            models,
            pricing: openai_gpt_5_6_pricing_catalog(),
        })
    }))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderCatalogError {
    #[error("provider catalog endpoint origin must be an HTTP(S) URL with a host")]
    InvalidEndpointOrigin,
    #[error("provider catalog endpoint origin must not contain credentials")]
    EndpointCredentialsNotAllowed,
    #[error("provider catalog route scope must not be empty")]
    EmptyRouteScope,
    #[error("provider catalog config revision must not be empty")]
    InvalidConfigRevision,
    #[error("provider catalog adapter mismatch: expected {expected:?}, got {actual:?}")]
    AdapterMismatch {
        expected: ProviderAdapter,
        actual: ProviderAdapter,
    },
}

#[derive(Debug, Clone, Copy)]
enum Gpt56Variant {
    Sol,
    Terra,
    Luna,
}

fn gpt_5_6_model(variant: Gpt56Variant) -> ProviderModelCapabilities {
    let (slug, display_name, description, listing_priority, default_effort, multi_agent) =
        match variant {
            Gpt56Variant::Sol => (
                "gpt-5.6-sol",
                "GPT-5.6-Sol",
                "Latest frontier agentic coding model.",
                1,
                CatalogReasoningEffort::Low,
                CatalogMultiAgentVersion::V2,
            ),
            Gpt56Variant::Terra => (
                "gpt-5.6-terra",
                "GPT-5.6-Terra",
                "Balanced agentic coding model for everyday work.",
                2,
                CatalogReasoningEffort::Medium,
                CatalogMultiAgentVersion::V2,
            ),
            Gpt56Variant::Luna => (
                "gpt-5.6-luna",
                "GPT-5.6-Luna",
                "Fast and affordable agentic coding model.",
                3,
                CatalogReasoningEffort::Medium,
                CatalogMultiAgentVersion::V1,
            ),
        };
    let mut supported_reasoning_efforts = vec![
        CatalogReasoningEffort::Low,
        CatalogReasoningEffort::Medium,
        CatalogReasoningEffort::High,
        CatalogReasoningEffort::Xhigh,
        CatalogReasoningEffort::Max,
    ];
    if !matches!(variant, Gpt56Variant::Luna) {
        supported_reasoning_efforts.push(CatalogReasoningEffort::Ultra);
    }

    ProviderModelCapabilities {
        slug: slug.to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
        listing_priority,
        context_window: 372_000,
        max_context_window: 372_000,
        default_reasoning_effort: default_effort,
        supported_reasoning_efforts,
        supports_priority_service_tier: true,
        uses_responses_lite: true,
        prefers_websockets: true,
        tool_mode: CatalogToolMode::CodeModeOnly,
        multi_agent_version: multi_agent,
        supports_parallel_tool_calls: true,
        supports_image_detail_original: true,
        minimum_codex_client_version: CatalogClientVersion::new(0, 144, 0),
        comp_hash: OPENAI_CODEX_COMP_HASH.to_string(),
    }
}

fn openai_gpt_5_6_pricing_catalog() -> ProviderPricingCatalog {
    let models = [
        (
            "gpt-5.6-sol",
            ["5", "0.5", "6.25", "30"],
            ["10", "1", "12.5", "60"],
        ),
        (
            "gpt-5.6-terra",
            ["2.5", "0.25", "3.125", "15"],
            ["5", "0.5", "6.25", "30"],
        ),
        (
            "gpt-5.6-luna",
            ["1", "0.1", "1.25", "6"],
            ["2", "0.2", "2.5", "12"],
        ),
    ]
    .into_iter()
    .map(|(model, standard, priority)| {
        let [input, cache_read, cache_write, output] = standard;
        let standard = ProviderTokenPriceFacts::complete(input, cache_read, cache_write, output);
        let [input, cache_read, cache_write, output] = priority;
        let priority = ProviderTokenPriceFacts::complete(input, cache_read, cache_write, output);
        (
            model.to_string(),
            ProviderModelPricing {
                standard: Some(standard),
                priority: Some(priority),
                cache_accounting_convention: CacheAccountingConvention::INCLUDED_IN_INPUT,
            },
        )
    })
    .collect();

    ProviderPricingCatalog {
        revision: ProviderPricingRevision(OPENAI_GPT_5_6_PRICING_REVISION.to_string()),
        provenance: ProviderPricingProvenance {
            authority: ProviderPricingAuthority::OpenAiApiPricing,
            source: OPENAI_GPT_5_6_PRICING_SOURCE.to_string(),
            captured_at: OPENAI_GPT_5_6_PRICING_CAPTURED_AT.to_string(),
        },
        models,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(seed: u8) -> AccountFingerprint {
        AccountFingerprint::from_digest([seed; 32])
    }

    fn scope(adapter: ProviderAdapter) -> ProviderCatalogScope {
        ProviderCatalogScope::new(
            adapter,
            "https://api.openai.com/v1/responses",
            "route:codex",
            fingerprint(7),
            "sha256:test-config",
        )
        .expect("valid provider catalog scope")
    }

    #[test]
    fn scope_normalizes_origin_and_keeps_all_identity_dimensions() {
        let scope = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "HTTPS://API.OPENAI.COM:443/v1/models?ignored=true",
            " route:sol ",
            fingerprint(9),
            "sha256:config-seven",
        )
        .expect("valid scope");

        assert_eq!(scope.adapter(), ProviderAdapter::OpenAiCodex);
        assert_eq!(scope.endpoint_origin(), "https://api.openai.com");
        assert_eq!(scope.route_scope(), "route:sol");
        assert_eq!(scope.account_fingerprint(), fingerprint(9));
        assert_eq!(scope.config_revision(), "sha256:config-seven");
        assert_eq!(
            format!("{:?}", scope.account_fingerprint()),
            "AccountFingerprint([redacted])"
        );
        let serialized = serde_json::to_value(&scope).expect("serialize scope");
        assert_eq!(
            serialized["account_fingerprint"].as_str(),
            Some("sha256:0909090909090909090909090909090909090909090909090909090909090909")
        );
    }

    #[test]
    fn provider_adapter_is_derived_from_the_normalized_endpoint_host() {
        let openai = reqwest::Url::parse("https://api.openai.com/v1/responses").expect("OpenAI");
        let bedrock = reqwest::Url::parse(
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/test/invoke",
        )
        .expect("Bedrock");
        let compatible =
            reqwest::Url::parse("https://relay.example/v1/responses").expect("compatible");

        assert_eq!(
            ProviderAdapter::for_endpoint(&openai),
            ProviderAdapter::OpenAiCodex
        );
        assert_eq!(
            ProviderAdapter::for_endpoint(&bedrock),
            ProviderAdapter::AwsBedrock
        );
        assert_eq!(
            ProviderAdapter::for_endpoint(&compatible),
            ProviderAdapter::OpenAiCompatible
        );
    }

    #[test]
    fn opaque_credential_scopes_produce_stable_account_fingerprints() {
        let first_fingerprint = AccountFingerprint::from_credential_scope(
            "hmac-sha256-v1:opaque-installation-scoped-account-one",
        );
        let same_fingerprint = AccountFingerprint::from_credential_scope(
            "hmac-sha256-v1:opaque-installation-scoped-account-one",
        );
        let other_fingerprint = AccountFingerprint::from_credential_scope(
            "hmac-sha256-v1:opaque-installation-scoped-account-two",
        );

        assert_eq!(first_fingerprint, same_fingerprint);
        assert_ne!(first_fingerprint, other_fingerprint);
        let rendered = first_fingerprint.to_string();
        assert!(rendered.starts_with("sha256:"));
        assert!(!rendered.contains("opaque-installation-scoped-account-one"));
        assert_eq!(
            format!("{first_fingerprint:?}"),
            "AccountFingerprint([redacted])"
        );
    }

    #[test]
    fn anonymous_account_fingerprint_is_stable_and_distinct_from_credential_scope() {
        let anonymous = AccountFingerprint::unscoped();

        assert_eq!(anonymous, AccountFingerprint::unscoped());
        assert_ne!(
            anonymous,
            AccountFingerprint::from_credential_scope(
                "hmac-sha256-v1:opaque-installation-scoped-account"
            )
        );
    }

    #[test]
    fn config_revision_is_part_of_provider_catalog_scope_identity() {
        let revision_one = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://api.openai.com/v1",
            "route:codex",
            fingerprint(3),
            "sha256:config-one",
        )
        .expect("revision one");
        let revision_two = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://api.openai.com/v1",
            "route:codex",
            fingerprint(3),
            "sha256:config-two",
        )
        .expect("revision two");

        assert_eq!(revision_one.config_revision(), "sha256:config-one");
        assert_ne!(revision_one, revision_two);
    }

    #[test]
    fn actual_service_tier_resolution_never_falls_back_to_standard() {
        for actual in [Some("default"), Some("standard"), Some(" STANDARD ")] {
            assert_eq!(
                ProviderPricingTier::from_actual_service_tier(actual),
                ProviderPricingTier::Standard
            );
        }
        assert_eq!(
            ProviderPricingTier::from_actual_service_tier(Some("priority")),
            ProviderPricingTier::Priority
        );
        for actual in [
            Some("flex"),
            Some("batch"),
            Some("regional"),
            Some("garbage"),
            Some(""),
            None,
        ] {
            assert_eq!(
                ProviderPricingTier::from_actual_service_tier(actual),
                ProviderPricingTier::Unknown,
                "actual tier: {actual:?}"
            );
        }
    }

    #[test]
    fn scope_rejects_credentials_and_missing_route_scope() {
        assert_eq!(
            ProviderCatalogScope::new(
                ProviderAdapter::OpenAiCodex,
                "https://secret@example.com/v1",
                "route",
                fingerprint(1),
                "sha256:test-config",
            ),
            Err(ProviderCatalogError::EndpointCredentialsNotAllowed)
        );
        assert_eq!(
            ProviderCatalogScope::new(
                ProviderAdapter::OpenAiCodex,
                "https://api.openai.com/v1",
                " ",
                fingerprint(1),
                "sha256:test-config",
            ),
            Err(ProviderCatalogError::EmptyRouteScope)
        );
    }

    #[test]
    fn bundled_openai_codex_epoch_preserves_distinct_gpt_5_6_facts() {
        let epoch = ProviderCatalogEpoch::bundled_openai_codex(scope(ProviderAdapter::OpenAiCodex))
            .expect("OpenAI Codex epoch");

        assert_eq!(epoch.revision().as_str(), OPENAI_CODEX_CATALOG_REVISION);
        assert_eq!(epoch.models().len(), 3);
        assert_eq!(
            epoch.provenance().authority(),
            ProviderCatalogAuthority::CodexModelsRepository
        );
        assert_eq!(
            epoch.provenance().freshness(),
            ProviderCatalogFreshness::BundledSnapshot
        );
        assert_eq!(
            epoch.provenance().source_revision(),
            OPENAI_CODEX_SOURCE_REVISION
        );
        assert_eq!(
            epoch.provenance().source_updated_at(),
            OPENAI_CODEX_SOURCE_UPDATED_AT
        );

        let sol = epoch.model("gpt-5.6-sol").expect("Sol");
        let terra = epoch.model("gpt-5.6-terra").expect("Terra");
        let luna = epoch.model("gpt-5.6-luna").expect("Luna");

        assert_eq!(sol.context_window(), 372_000);
        assert_eq!(sol.default_reasoning_effort(), CatalogReasoningEffort::Low);
        assert_eq!(sol.multi_agent_version(), CatalogMultiAgentVersion::V2);
        assert!(
            sol.supported_reasoning_efforts()
                .contains(&CatalogReasoningEffort::Ultra)
        );
        assert_eq!(terra.listing_priority(), 2);
        assert_eq!(
            terra.default_reasoning_effort(),
            CatalogReasoningEffort::Medium
        );
        assert_eq!(terra.multi_agent_version(), CatalogMultiAgentVersion::V2);
        assert_eq!(luna.listing_priority(), 3);
        assert_eq!(luna.multi_agent_version(), CatalogMultiAgentVersion::V1);
        assert!(
            !luna
                .supported_reasoning_efforts()
                .contains(&CatalogReasoningEffort::Ultra)
        );

        for model in [sol, terra, luna] {
            assert_eq!(model.max_context_window(), 372_000);
            assert!(model.supports_priority_service_tier());
            assert!(model.uses_responses_lite());
            assert!(model.prefers_websockets());
            assert_eq!(model.tool_mode(), CatalogToolMode::CodeModeOnly);
            assert!(model.supports_parallel_tool_calls());
            assert!(model.supports_image_detail_original());
            assert_eq!(model.minimum_codex_client_version().to_string(), "0.144.0");
            assert_eq!(model.comp_hash(), OPENAI_CODEX_COMP_HASH);
        }

        let serialized = serde_json::to_value(&epoch).expect("serialize epoch");
        assert_eq!(
            serialized["models"]["gpt-5.6-sol"]["minimum_codex_client_version"].as_str(),
            Some("0.144.0")
        );
    }

    #[test]
    fn scoped_model_request_contract_distinguishes_ultra_support_and_identity() {
        let provider_scope = scope(ProviderAdapter::OpenAiCodex);
        let epoch = ProviderCatalogEpoch::bundled_openai_codex(provider_scope.clone())
            .expect("OpenAI Codex epoch");

        for model in ["gpt-5.6-sol", "gpt-5.6-terra"] {
            let contract = epoch
                .capture_model_request_contract(model)
                .expect("supported model contract");
            assert_eq!(contract.scope(), &provider_scope);
            assert_eq!(contract.catalog_revision(), epoch.revision());
            assert_eq!(contract.model(), model);
            assert!(contract.ultra_maps_to_max());
        }

        let luna = epoch
            .capture_model_request_contract("gpt-5.6-luna")
            .expect("Luna model contract");
        assert!(!luna.ultra_maps_to_max());
        assert!(
            epoch
                .capture_model_request_contract("gpt-5.6-unknown")
                .is_none()
        );
    }

    #[test]
    fn openai_codex_epoch_rejects_bedrock_scope_and_slug_aliasing() {
        let error = ProviderCatalogEpoch::bundled_openai_codex(scope(ProviderAdapter::AwsBedrock))
            .expect_err("Bedrock scope must not borrow OpenAI Codex facts");
        assert_eq!(
            error,
            ProviderCatalogError::AdapterMismatch {
                expected: ProviderAdapter::OpenAiCodex,
                actual: ProviderAdapter::AwsBedrock,
            }
        );

        let epoch = ProviderCatalogEpoch::bundled_openai_codex(scope(ProviderAdapter::OpenAiCodex))
            .expect("OpenAI Codex epoch");
        assert!(epoch.model("openai.gpt-5.6-sol").is_none());
    }

    #[test]
    fn catalog_epoch_scope_keeps_each_identity_dimension_independent() {
        let base_scope = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://api.openai.com/v1",
            "route:codex",
            fingerprint(1),
            "sha256:config-one",
        )
        .expect("base scope");
        let changed_origin = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://relay.example/v1",
            "route:codex",
            fingerprint(1),
            "sha256:config-one",
        )
        .expect("changed origin");
        let changed_route = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://api.openai.com/v1",
            "route:other",
            fingerprint(1),
            "sha256:config-one",
        )
        .expect("changed route");
        let changed_account = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://api.openai.com/v1",
            "route:codex",
            fingerprint(2),
            "sha256:config-one",
        )
        .expect("changed account");
        let changed_revision = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://api.openai.com/v1",
            "route:codex",
            fingerprint(1),
            "sha256:config-two",
        )
        .expect("changed revision");

        for changed in [
            changed_origin,
            changed_route,
            changed_account,
            changed_revision,
        ] {
            assert_ne!(base_scope, changed);
            let epoch = ProviderCatalogEpoch::bundled_openai_codex(changed)
                .expect("scoped OpenAI Codex epoch");
            assert_eq!(epoch.revision().as_str(), OPENAI_CODEX_CATALOG_REVISION);
        }
    }

    #[test]
    fn gpt_5_6_prices_are_tiered_and_frozen_with_independent_provenance() {
        let epoch = ProviderCatalogEpoch::bundled_openai_codex(scope(ProviderAdapter::OpenAiCodex))
            .expect("OpenAI Codex epoch");
        let expected = [
            (
                "gpt-5.6-sol",
                ProviderPricingTier::Standard,
                ["5", "0.5", "6.25", "30"],
            ),
            (
                "gpt-5.6-sol",
                ProviderPricingTier::Priority,
                ["10", "1", "12.5", "60"],
            ),
            (
                "gpt-5.6-terra",
                ProviderPricingTier::Standard,
                ["2.5", "0.25", "3.125", "15"],
            ),
            (
                "gpt-5.6-terra",
                ProviderPricingTier::Priority,
                ["5", "0.5", "6.25", "30"],
            ),
            (
                "gpt-5.6-luna",
                ProviderPricingTier::Standard,
                ["1", "0.1", "1.25", "6"],
            ),
            (
                "gpt-5.6-luna",
                ProviderPricingTier::Priority,
                ["2", "0.2", "2.5", "12"],
            ),
        ];

        for (model, tier, [input, cache_read, cache_write, output]) in expected {
            let key = epoch.capture_price_key(model, tier);
            let quote = epoch.price_quote(&key).expect("complete scoped price");
            let prices = quote.prices_per_million();

            assert_eq!(prices.input_usd(), Some(input), "{model} {tier:?}");
            assert_eq!(
                prices.cache_read_usd(),
                Some(cache_read),
                "{model} {tier:?}"
            );
            assert_eq!(
                prices.cache_write_usd(),
                Some(cache_write),
                "{model} {tier:?}"
            );
            assert_eq!(prices.output_usd(), Some(output), "{model} {tier:?}");
            assert_eq!(
                quote.cache_accounting_convention(),
                crate::usage::CacheAccountingConvention::INCLUDED_IN_INPUT
            );
            assert_eq!(quote.key(), &key);
            assert_eq!(
                quote.pricing_revision().as_str(),
                OPENAI_GPT_5_6_PRICING_REVISION
            );
        }

        assert_eq!(
            epoch.pricing_provenance().authority(),
            ProviderPricingAuthority::OpenAiApiPricing
        );
        assert_eq!(
            epoch.pricing_revision().as_str(),
            OPENAI_GPT_5_6_PRICING_REVISION
        );
        assert_eq!(
            epoch.pricing_provenance().source(),
            OPENAI_GPT_5_6_PRICING_SOURCE
        );
        assert_eq!(
            epoch.pricing_provenance().captured_at(),
            OPENAI_GPT_5_6_PRICING_CAPTURED_AT
        );

        let serialized = serde_json::to_value(&epoch).expect("serialize epoch");
        assert_eq!(
            serialized["pricing"]["revision"].as_str(),
            Some(OPENAI_GPT_5_6_PRICING_REVISION)
        );
        assert_eq!(
            serialized["pricing"]["provenance"]["authority"].as_str(),
            Some("open_ai_api_pricing")
        );
        let key = epoch.capture_price_key("gpt-5.6-sol", ProviderPricingTier::Priority);
        let serialized_key = serde_json::to_value(&key).expect("serialize price key");
        assert_eq!(serialized_key["scope"], serialized["scope"]);
        assert_eq!(serialized_key["catalog_revision"], serialized["revision"]);
        assert_eq!(
            serialized_key["pricing_revision"].as_str(),
            Some(OPENAI_GPT_5_6_PRICING_REVISION)
        );
        assert_eq!(serialized_key["model"].as_str(), Some("gpt-5.6-sol"));
        assert_eq!(serialized_key["tier"].as_str(), Some("priority"));
    }

    #[test]
    fn price_key_rejects_other_scope_and_unsupported_tiers() {
        let epoch = ProviderCatalogEpoch::bundled_openai_codex(scope(ProviderAdapter::OpenAiCodex))
            .expect("OpenAI Codex epoch");
        let other_scope = ProviderCatalogScope::new(
            ProviderAdapter::OpenAiCodex,
            "https://relay.example/v1",
            "route:codex",
            fingerprint(7),
            "sha256:test-config",
        )
        .expect("other scope");
        let other_epoch = ProviderCatalogEpoch::bundled_openai_codex(other_scope)
            .expect("other OpenAI Codex epoch");
        let foreign_key =
            other_epoch.capture_price_key("gpt-5.6-sol", ProviderPricingTier::Standard);

        assert!(epoch.price_quote(&foreign_key).is_none());
        let local_key = epoch.capture_price_key("gpt-5.6-sol", ProviderPricingTier::Standard);
        let mut repriced_epoch = epoch.clone();
        Arc::make_mut(&mut repriced_epoch.facts).pricing.revision =
            ProviderPricingRevision("openai-api-pricing:changed".to_string());
        assert_eq!(repriced_epoch.revision(), epoch.revision());
        assert!(repriced_epoch.price_quote(&local_key).is_none());
        for tier in [
            ProviderPricingTier::Flex,
            ProviderPricingTier::Batch,
            ProviderPricingTier::Regional,
            ProviderPricingTier::Unknown,
        ] {
            let key = epoch.capture_price_key("gpt-5.6-sol", tier);
            assert!(epoch.price_quote(&key).is_none(), "tier: {tier:?}");
        }
        let missing_key = epoch.capture_price_key("gpt-5.6-missing", ProviderPricingTier::Standard);
        assert!(epoch.price_quote(&missing_key).is_none());
    }

    #[test]
    fn incomplete_price_facts_do_not_produce_a_quote() {
        let mut epoch =
            ProviderCatalogEpoch::bundled_openai_codex(scope(ProviderAdapter::OpenAiCodex))
                .expect("OpenAI Codex epoch");
        Arc::make_mut(&mut epoch.facts)
            .pricing
            .models
            .get_mut("gpt-5.6-sol")
            .expect("Sol pricing")
            .standard
            .as_mut()
            .expect("standard pricing")
            .cache_write_usd = None;
        let key = epoch.capture_price_key("gpt-5.6-sol", ProviderPricingTier::Standard);

        assert!(epoch.price_quote(&key).is_none());
    }
}
