use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use reqwest::header::{
    ETAG, HeaderMap, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, RETRY_AFTER,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::pricing::basellm_all_json_url;
use crate::pricing::{ModelPrice, ModelPriceTier, canonical_provider};
use crate::runtime_store::{
    RuntimeDocument, RuntimeDocumentCommit, RuntimeDocumentKind, RuntimeDocumentWrite,
    RuntimeStore, RuntimeStoreError, RuntimeStoreReader,
};

pub const BASELLM_CATALOG_SCHEMA_VERSION: u32 = 1;
pub const BASELLM_CATALOG_MANIFEST_GENERATION: u64 = 1;
const DEFAULT_RESPONSE_LIMIT: usize = 16 * 1024 * 1024;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_WARNINGS: usize = 128;
const MAX_WARNING_BYTES: usize = 240;
const MAX_METADATA_DISPLAY_NAME_BYTES: usize = 512;
const MAX_METADATA_DESCRIPTION_BYTES: usize = 8 * 1024;
const MAX_MODEL_ALIASES: usize = 128;
const MAX_PRICE_PER_MILLION_USD: i128 = 1_000_000;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BasellmOpenAiModelMetadata {
    pub model_id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub context_window: Option<i64>,
    pub max_context_window: Option<i64>,
    pub input_modalities: Vec<String>,
    pub reasoning: Option<bool>,
    pub tool_call: Option<bool>,
    pub structured_output: Option<bool>,
    pub supports_fast_priority: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BasellmCatalogPriceFields {
    pub input_per_1m_usd: Option<String>,
    pub output_per_1m_usd: Option<String>,
    pub cache_read_input_per_1m_usd: Option<String>,
    pub cache_creation_input_per_1m_usd: Option<String>,
}

impl BasellmCatalogPriceFields {
    pub fn is_empty(&self) -> bool {
        self.input_per_1m_usd.is_none()
            && self.output_per_1m_usd.is_none()
            && self.cache_read_input_per_1m_usd.is_none()
            && self.cache_creation_input_per_1m_usd.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BasellmCatalogContextTier {
    pub threshold_tokens: u64,
    pub prices: BasellmCatalogPriceFields,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BasellmCatalogPrice {
    pub base: BasellmCatalogPriceFields,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_tiers: Vec<BasellmCatalogContextTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BasellmCatalogModel {
    pub model_id: String,
    pub source_provider_id: String,
    pub metadata: BasellmOpenAiModelMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<BasellmCatalogPrice>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BasellmProviderCatalog {
    pub provider_id: String,
    pub display_name: Option<String>,
    pub models: BTreeMap<String, BasellmCatalogModel>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BasellmCatalogContent {
    pub providers: BTreeMap<String, BasellmProviderCatalog>,
}

impl BasellmCatalogContent {
    pub fn model(&self, provider: &str, model: &str) -> Option<&BasellmCatalogModel> {
        self.providers
            .get(&normalize_id(provider))?
            .models
            .get(&normalize_id(model))
    }

    pub fn counts(&self) -> BasellmCatalogCounts {
        let mut counts = BasellmCatalogCounts {
            provider_count: self.providers.len(),
            ..BasellmCatalogCounts::default()
        };
        for provider in self.providers.values() {
            counts.model_count = counts.model_count.saturating_add(provider.models.len());
            for model in provider.models.values() {
                if let Some(price) = &model.price {
                    counts.priced_model_count = counts.priced_model_count.saturating_add(1);
                    counts.tier_count = counts.tier_count.saturating_add(price.context_tiers.len());
                }
            }
        }
        counts
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BasellmCatalogCounts {
    pub provider_count: usize,
    pub model_count: usize,
    pub priced_model_count: usize,
    pub tier_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BasellmCatalogLkg {
    pub schema_version: u32,
    pub manifest_generation: u64,
    pub body_generation: u64,
    pub content_generation: u64,
    pub source_url: String,
    pub fetched_at_unix: i64,
    pub validated_at_unix: i64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_hash: String,
    pub counts: BasellmCatalogCounts,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub catalog: BasellmCatalogContent,
}

impl BasellmCatalogLkg {
    pub fn model(&self, provider: &str, model: &str) -> Option<&BasellmCatalogModel> {
        self.catalog.model(provider, model)
    }

    pub fn model_prices_for_provider(&self, provider: &str) -> Vec<ModelPrice> {
        let provider = normalize_id(provider);
        let Some(provider_catalog) = self.catalog.providers.get(&provider) else {
            return Vec::new();
        };
        provider_catalog
            .models
            .values()
            .filter_map(|model| {
                let price = model.price.as_ref()?;
                let input = price.base.input_per_1m_usd.as_deref()?;
                let output = price.base.output_per_1m_usd.as_deref()?;
                let tiers = price
                    .context_tiers
                    .iter()
                    .map(|tier| {
                        ModelPriceTier::from_per_million_usd(
                            tier.threshold_tokens,
                            tier.prices.input_per_1m_usd.as_deref(),
                            tier.prices.output_per_1m_usd.as_deref(),
                            tier.prices.cache_read_input_per_1m_usd.as_deref(),
                            tier.prices.cache_creation_input_per_1m_usd.as_deref(),
                        )
                    })
                    .collect::<Option<Vec<_>>>()?;
                ModelPrice::from_per_million_usd_for_provider(
                    &provider,
                    model.model_id.clone(),
                    model.metadata.display_name.clone(),
                    input,
                    output,
                    price.base.cache_read_input_per_1m_usd.as_deref(),
                    price.base.cache_creation_input_per_1m_usd.as_deref(),
                    format!("basellm-remote:{}", model.source_provider_id),
                )
                .map(|price| price.with_aliases(model.aliases.clone()))
                .and_then(|price| price.with_tiers(tiers).ok())
                .map(|price| price.with_source_generation(self.content_hash.clone()))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BasellmSyncOutcome {
    Updated,
    NotModified,
    Quarantined,
    Unavailable,
    StaleResponse,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BasellmSyncErrorCategory {
    Timeout,
    Transport,
    Redirect,
    Http,
    RateLimited,
    BodyTooLarge,
    Utf8,
    Json,
    Schema,
    Semantic,
    Sanity,
    EconomicAnomaly,
    Persistence,
    StaleResponse,
    UnsupportedSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BasellmCatalogAttemptState {
    pub schema_version: u32,
    #[serde(default)]
    pub check_generation: u64,
    pub source_url: String,
    pub last_checked_at_unix: i64,
    pub outcome: BasellmSyncOutcome,
    pub last_error_category: Option<BasellmSyncErrorCategory>,
    pub content_hash: Option<String>,
    pub content_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantined_candidate_hash: Option<String>,
    pub read_only_schema_version: Option<u32>,
    pub retry_after_unix: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct BasellmCatalogSyncReport {
    pub outcome: BasellmSyncOutcome,
    pub snapshot: Option<Arc<BasellmCatalogLkg>>,
    pub attempt: BasellmCatalogAttemptState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BasellmCatalogLoad {
    Missing,
    Valid(Arc<BasellmCatalogLkg>),
    Corrupt,
    UnsupportedSchema(u32),
}

/// BaseLLM state decoded from the canonical runtime document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasellmCatalogRuntimeState {
    pub lkg: BasellmCatalogLoad,
    pub attempt: Option<BasellmCatalogAttemptState>,
    pub document_revision: Option<u64>,
}

impl Default for BasellmCatalogRuntimeState {
    fn default() -> Self {
        Self {
            lkg: BasellmCatalogLoad::Missing,
            attempt: None,
            document_revision: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BasellmCatalogRuntimeDocument {
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lkg: Option<BasellmCatalogLkg>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempt: Option<BasellmCatalogAttemptState>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BasellmCatalogParseError {
    #[error("invalid BaseLLM JSON")]
    Json,
    #[error("BaseLLM response is missing openai.models")]
    MissingOpenAiModels,
    #[error("invalid provider or model identifier")]
    InvalidIdentifier,
    #[error("invalid BaseLLM display metadata")]
    InvalidMetadata,
    #[error("duplicate canonical provider/model key")]
    CanonicalCollision,
    #[error("invalid price field")]
    InvalidPrice,
    #[error("invalid context tier")]
    InvalidTier,
    #[error("BaseLLM catalog contains no usable models")]
    EmptyCatalog,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BasellmCatalogImportError {
    #[error("invalid BaseLLM import URL")]
    InvalidUrl,
    #[error("BaseLLM import URL must use HTTP or HTTPS")]
    UnsupportedScheme,
    #[error("BaseLLM import URL must not contain user information")]
    UserInfoNotAllowed,
    #[error("BaseLLM import fetch failed ({0:?})")]
    Fetch(BasellmSyncErrorCategory),
    #[error(transparent)]
    Parse(#[from] BasellmCatalogParseError),
    #[error("BaseLLM import catalog validation failed ({0:?})")]
    Validation(BasellmSyncErrorCategory),
}

#[derive(Debug, Clone)]
pub struct BasellmCatalogSyncOptions {
    source_url: String,
    force: bool,
    require_https: bool,
    response_limit: usize,
    connect_timeout: Duration,
    read_timeout: Duration,
    total_timeout: Duration,
    max_attempts: usize,
    approved_economic_change_hash: Option<String>,
}

impl Default for BasellmCatalogSyncOptions {
    fn default() -> Self {
        Self {
            source_url: basellm_all_json_url().to_string(),
            force: false,
            require_https: true,
            response_limit: DEFAULT_RESPONSE_LIMIT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
            total_timeout: DEFAULT_TOTAL_TIMEOUT,
            max_attempts: 3,
            approved_economic_change_hash: None,
        }
    }
}

impl BasellmCatalogSyncOptions {
    pub fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    pub fn with_source_url(mut self, source_url: impl Into<String>) -> Self {
        self.source_url = source_url.into();
        self
    }

    pub fn with_max_attempts(mut self, max_attempts: usize) -> Self {
        self.max_attempts = max_attempts.clamp(1, 5);
        self
    }

    /// Approves exactly one previously quarantined economic-change candidate.
    pub fn with_approved_quarantine_hash(mut self, content_hash: impl Into<String>) -> Self {
        let content_hash = content_hash.into();
        self.approved_economic_change_hash =
            valid_content_hash(&content_hash).then_some(content_hash);
        self
    }

    #[cfg(test)]
    fn allow_http_for_fixture(mut self) -> Self {
        self.require_https = false;
        self
    }
}

static CATALOG_SNAPSHOT: OnceLock<RwLock<Option<Arc<BasellmCatalogLkg>>>> = OnceLock::new();

fn catalog_snapshot_slot() -> &'static RwLock<Option<Arc<BasellmCatalogLkg>>> {
    CATALOG_SNAPSHOT.get_or_init(|| RwLock::new(None))
}

pub fn basellm_catalog_snapshot() -> Option<Arc<BasellmCatalogLkg>> {
    catalog_snapshot_slot()
        .read()
        .ok()
        .and_then(|snapshot| snapshot.clone())
}

fn install_catalog_snapshot(snapshot: Arc<BasellmCatalogLkg>) {
    replace_catalog_snapshot(Some(snapshot));
}

fn replace_catalog_snapshot(snapshot: Option<Arc<BasellmCatalogLkg>>) {
    if let Ok(mut current) = catalog_snapshot_slot().write() {
        *current = snapshot;
    }
}

/// Installs the validated runtime LKG, or clears the remote layer when none is valid.
pub fn install_basellm_catalog_runtime_state(state: &BasellmCatalogRuntimeState) {
    let snapshot = match &state.lkg {
        BasellmCatalogLoad::Valid(snapshot) => Some(snapshot.clone()),
        _ => None,
    };
    replace_catalog_snapshot(snapshot);
}

pub fn parse_basellm_catalog_json(
    text: &str,
) -> Result<(BasellmCatalogContent, Vec<String>), BasellmCatalogParseError> {
    let root: Value = serde_json::from_str(text).map_err(|_| BasellmCatalogParseError::Json)?;
    let providers = root.as_object().ok_or(BasellmCatalogParseError::Json)?;
    let mut catalog = BasellmCatalogContent::default();
    let mut warnings = Vec::new();
    for (provider_key, provider_value) in providers {
        let Some(models) = provider_value.get("models").and_then(Value::as_object) else {
            continue;
        };
        let source_provider_id = provider_key.trim().to_string();
        let provider_id = canonical_provider(provider_key)
            .filter(|provider| !provider.is_empty())
            .ok_or(BasellmCatalogParseError::InvalidIdentifier)?;
        if !valid_identifier(provider_key) {
            return Err(BasellmCatalogParseError::InvalidIdentifier);
        }
        let display_name = json_string(provider_value.get("name"));
        if display_name
            .as_deref()
            .is_some_and(|value| !valid_metadata_text(value, MAX_METADATA_DISPLAY_NAME_BYTES))
        {
            return Err(BasellmCatalogParseError::InvalidMetadata);
        }
        let provider = catalog
            .providers
            .entry(provider_id.clone())
            .or_insert_with(|| BasellmProviderCatalog {
                provider_id: provider_id.clone(),
                display_name: None,
                models: BTreeMap::new(),
            });
        if display_name
            .as_ref()
            .zip(provider.display_name.as_ref())
            .is_some_and(|(candidate, current)| candidate < current)
            || provider.display_name.is_none()
        {
            provider.display_name = display_name;
        }
        for (model_key, model_value) in models {
            let normalized_model = normalize_id(model_key);
            if !valid_identifier(model_key) || normalized_model.is_empty() {
                return Err(BasellmCatalogParseError::InvalidIdentifier);
            }
            let metadata = parse_basellm_model_metadata(model_key, model_value);
            if metadata
                .display_name
                .as_deref()
                .is_some_and(|value| !valid_metadata_text(value, MAX_METADATA_DISPLAY_NAME_BYTES))
                || metadata.description.as_deref().is_some_and(|value| {
                    !valid_metadata_text(value, MAX_METADATA_DESCRIPTION_BYTES)
                })
            {
                return Err(BasellmCatalogParseError::InvalidMetadata);
            }
            let aliases = parse_model_aliases(model_key, model_value)?;
            let price =
                parse_model_price(&provider_id, &normalized_model, model_value, &mut warnings)?;
            if provider.models.contains_key(&normalized_model) {
                return Err(BasellmCatalogParseError::CanonicalCollision);
            }
            provider.models.insert(
                normalized_model,
                BasellmCatalogModel {
                    model_id: model_key.trim().to_string(),
                    source_provider_id: source_provider_id.clone(),
                    metadata,
                    aliases,
                    price,
                },
            );
        }
        if !valid_provider_aliases(&provider.models) {
            return Err(BasellmCatalogParseError::CanonicalCollision);
        }
    }
    catalog
        .providers
        .retain(|_, provider| !provider.models.is_empty());
    let valid_openai = catalog.providers.get("openai").is_some_and(|provider| {
        !provider.models.is_empty() && provider.models.values().any(|model| model.price.is_some())
    });
    if !valid_openai {
        return Err(BasellmCatalogParseError::MissingOpenAiModels);
    }
    if catalog.providers.is_empty() || catalog.counts().model_count == 0 {
        return Err(BasellmCatalogParseError::EmptyCatalog);
    }
    warnings.truncate(MAX_WARNINGS);
    Ok((catalog, warnings))
}

fn parse_basellm_model_metadata(model_id: &str, model_value: &Value) -> BasellmOpenAiModelMetadata {
    let display_name = json_string(
        model_value
            .get("name")
            .or_else(|| model_value.get("display_name")),
    )
    .filter(|value| !value.eq_ignore_ascii_case(model_id));
    let description = json_string(model_value.get("description"));
    let context_window = model_value
        .get("limit")
        .and_then(|limit| limit.get("input").or_else(|| limit.get("context")))
        .and_then(json_i64);
    let max_context_window = model_value
        .get("limit")
        .and_then(|limit| limit.get("context").or_else(|| limit.get("input")))
        .and_then(json_i64);
    BasellmOpenAiModelMetadata {
        model_id: model_id.to_string(),
        display_name,
        description,
        context_window,
        max_context_window,
        input_modalities: parse_input_modalities(model_value),
        reasoning: model_value.get("reasoning").and_then(Value::as_bool),
        tool_call: model_value.get("tool_call").and_then(Value::as_bool),
        structured_output: model_value
            .get("structured_output")
            .and_then(Value::as_bool),
        supports_fast_priority: basellm_model_supports_fast_priority(model_value),
    }
}

fn parse_input_modalities(model_value: &Value) -> Vec<String> {
    let Some(items) = model_value
        .get("modalities")
        .and_then(|modalities| modalities.get("input"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let mut modalities = Vec::new();
    for modality in items.iter().filter_map(Value::as_str).map(str::trim) {
        let modality = match modality.to_ascii_lowercase().as_str() {
            "text" => "text",
            "image" => "image",
            // PDFs remain attachments rather than a modality unknown to older Codex clients.
            "pdf" => continue,
            _ => continue,
        };
        if !modalities.iter().any(|existing| existing == modality) {
            modalities.push(modality.to_string());
        }
    }
    modalities
}

fn basellm_model_supports_fast_priority(model_value: &Value) -> bool {
    model_value
        .get("experimental")
        .and_then(|experimental| experimental.get("modes"))
        .and_then(|modes| modes.get("fast"))
        .and_then(|fast| fast.get("provider"))
        .and_then(|provider| provider.get("body"))
        .and_then(|body| body.get("service_tier"))
        .and_then(Value::as_str)
        .is_some_and(|service_tier| service_tier.eq_ignore_ascii_case("priority"))
}

fn parse_model_aliases(
    model_id: &str,
    model_value: &Value,
) -> Result<Vec<String>, BasellmCatalogParseError> {
    let Some(value) = model_value.get("aliases").filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let values = match value {
        Value::String(value) => vec![value.as_str()],
        Value::Array(values) => values
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .ok_or(BasellmCatalogParseError::InvalidMetadata)?,
        _ => return Err(BasellmCatalogParseError::InvalidMetadata),
    };
    if values.len() > MAX_MODEL_ALIASES {
        return Err(BasellmCatalogParseError::InvalidMetadata);
    }

    let model_key = normalize_id(model_id);
    let mut aliases = BTreeMap::<String, String>::new();
    for alias in values {
        let alias = alias.trim();
        if !valid_identifier(alias) {
            return Err(BasellmCatalogParseError::InvalidMetadata);
        }
        let alias_key = normalize_id(alias);
        if alias_key == model_key {
            continue;
        }
        aliases
            .entry(alias_key)
            .and_modify(|current| {
                if alias < current.as_str() {
                    *current = alias.to_string();
                }
            })
            .or_insert_with(|| alias.to_string());
    }
    Ok(aliases.into_values().collect())
}

fn valid_provider_aliases(models: &BTreeMap<String, BasellmCatalogModel>) -> bool {
    let mut aliases = BTreeMap::<String, &str>::new();
    for (model_key, model) in models {
        if model.aliases.len() > MAX_MODEL_ALIASES {
            return false;
        }
        for alias in &model.aliases {
            let alias_key = normalize_id(alias);
            if !valid_identifier(alias)
                || alias_key == *model_key
                || models.contains_key(&alias_key)
                || aliases.insert(alias_key, model_key).is_some()
            {
                return false;
            }
        }
    }
    true
}

fn parse_model_price(
    provider: &str,
    model: &str,
    model_value: &Value,
    warnings: &mut Vec<String>,
) -> Result<Option<BasellmCatalogPrice>, BasellmCatalogParseError> {
    let Some(cost) = model_value.get("cost") else {
        return Ok(None);
    };
    if cost.is_null() {
        return Ok(None);
    }
    let Some(cost) = cost.as_object() else {
        return Err(BasellmCatalogParseError::InvalidPrice);
    };
    let base = parse_price_fields(cost)?;
    if base.is_empty() {
        return Ok(None);
    }
    if base.input_per_1m_usd.is_none() || base.output_per_1m_usd.is_none() {
        push_warning(
            warnings,
            format!("skipped incomplete token price for {provider}/{model}"),
        );
        return Ok(None);
    }

    let mut context_tiers = Vec::new();
    if let Some(tiers) = cost.get("tiers").filter(|value| !value.is_null()) {
        let tiers = tiers
            .as_array()
            .ok_or(BasellmCatalogParseError::InvalidTier)?;
        for tier in tiers {
            let tier_kind = tier
                .get("tier")
                .and_then(|tier| tier.get("type"))
                .and_then(Value::as_str)
                .map(str::trim);
            if tier_kind != Some("context") {
                push_warning(
                    warnings,
                    format!("skipped unknown tier for {provider}/{model}"),
                );
                continue;
            }
            let threshold_tokens = tier
                .get("tier")
                .and_then(|tier| tier.get("size"))
                .and_then(json_u64)
                .filter(|value| *value > 0)
                .ok_or(BasellmCatalogParseError::InvalidTier)?;
            let prices = tier
                .as_object()
                .ok_or(BasellmCatalogParseError::InvalidTier)
                .and_then(parse_price_fields)?;
            if prices.is_empty() {
                return Err(BasellmCatalogParseError::InvalidTier);
            }
            context_tiers.push(BasellmCatalogContextTier {
                threshold_tokens,
                prices,
            });
        }
    } else if let Some(legacy) = cost
        .get("context_over_200k")
        .filter(|value| !value.is_null())
    {
        let prices = legacy
            .as_object()
            .ok_or(BasellmCatalogParseError::InvalidTier)
            .and_then(parse_price_fields)?;
        if !prices.is_empty() {
            context_tiers.push(BasellmCatalogContextTier {
                threshold_tokens: 200_000,
                prices,
            });
        }
    }

    context_tiers.sort_by_key(|tier| tier.threshold_tokens);
    if context_tiers
        .windows(2)
        .any(|pair| pair[0].threshold_tokens == pair[1].threshold_tokens)
    {
        return Err(BasellmCatalogParseError::InvalidTier);
    }
    Ok(Some(BasellmCatalogPrice {
        base,
        context_tiers,
    }))
}

fn parse_price_fields(
    object: &serde_json::Map<String, Value>,
) -> Result<BasellmCatalogPriceFields, BasellmCatalogParseError> {
    Ok(BasellmCatalogPriceFields {
        input_per_1m_usd: parse_optional_price(object, "input")?,
        output_per_1m_usd: parse_optional_price(object, "output")?,
        cache_read_input_per_1m_usd: parse_optional_price(object, "cache_read")?,
        cache_creation_input_per_1m_usd: parse_optional_price(object, "cache_write")?,
    })
}

fn parse_optional_price(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, BasellmCatalogParseError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let raw = match value {
        Value::Number(number) => number.to_string(),
        Value::String(value) => value.trim().to_string(),
        _ => return Err(BasellmCatalogParseError::InvalidPrice),
    };
    canonical_price_decimal(&raw)
        .map(Some)
        .ok_or(BasellmCatalogParseError::InvalidPrice)
}

fn canonical_price_decimal(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') || value.len() > 80 {
        return None;
    }
    let value = value.strip_prefix('+').unwrap_or(value);
    let (mantissa, exponent) = match value.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (value, 0),
    };
    let (whole, fractional) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (whole.is_empty() && fractional.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{whole}{fractional}");
    let digits = digits.trim_start_matches('0');
    let mantissa = if digits.is_empty() {
        0
    } else {
        digits.parse::<i128>().ok()?
    };
    let scale = exponent
        .checked_sub(i32::try_from(fractional.len()).ok()?)?
        .checked_add(15)?;
    let femto = if scale >= 0 {
        mantissa.checked_mul(pow10_i128(u32::try_from(scale).ok()?)?)?
    } else {
        let divisor = pow10_i128(scale.unsigned_abs())?;
        let quotient = mantissa / divisor;
        let remainder = mantissa % divisor;
        quotient.checked_add(i128::from(remainder.saturating_mul(2) >= divisor))?
    };
    if femto > MAX_PRICE_PER_MILLION_USD.checked_mul(1_000_000_000_000_000)? {
        return None;
    }
    Some(format_femto(femto))
}

fn pow10_i128(exponent: u32) -> Option<i128> {
    (0..exponent).try_fold(1_i128, |value, _| value.checked_mul(10))
}

fn format_femto(value: i128) -> String {
    let whole = value / 1_000_000_000_000_000;
    let fractional = value % 1_000_000_000_000_000;
    if fractional == 0 {
        return whole.to_string();
    }
    let mut fractional = format!("{fractional:015}");
    while fractional.ends_with('0') {
        fractional.pop();
    }
    format!("{whole}.{fractional}")
}

fn content_hash(content: &BasellmCatalogContent) -> Result<String, serde_json::Error> {
    let bytes = serde_json::to_vec(content)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn build_lkg(
    catalog: BasellmCatalogContent,
    warnings: Vec<String>,
    source_url: &str,
    validators: ResponseValidators,
    existing: Option<&BasellmCatalogLkg>,
    now: i64,
) -> Result<BasellmCatalogLkg, BasellmSyncErrorCategory> {
    let hash = content_hash(&catalog).map_err(|_| BasellmSyncErrorCategory::Schema)?;
    let same_content = existing.is_some_and(|existing| existing.content_hash == hash);
    Ok(BasellmCatalogLkg {
        schema_version: BASELLM_CATALOG_SCHEMA_VERSION,
        manifest_generation: BASELLM_CATALOG_MANIFEST_GENERATION,
        body_generation: existing
            .map(|existing| existing.body_generation.saturating_add(1))
            .unwrap_or(1),
        content_generation: existing
            .map(|existing| {
                if same_content {
                    existing.content_generation
                } else {
                    existing.content_generation.saturating_add(1)
                }
            })
            .unwrap_or(1),
        source_url: sanitize_source_url(source_url),
        fetched_at_unix: now,
        validated_at_unix: now,
        etag: validators.etag,
        last_modified: validators.last_modified,
        content_hash: hash,
        counts: catalog.counts(),
        warnings,
        catalog,
    })
}

fn validate_lkg(lkg: &BasellmCatalogLkg) -> bool {
    lkg.schema_version == BASELLM_CATALOG_SCHEMA_VERSION
        && lkg.manifest_generation == BASELLM_CATALOG_MANIFEST_GENERATION
        && lkg.body_generation > 0
        && lkg.content_generation > 0
        && valid_content_hash(&lkg.content_hash)
        && valid_persisted_source_url(&lkg.source_url)
        && lkg.etag.as_deref().is_none_or(|value| {
            safe_header_value(value).is_some() && !value.chars().any(char::is_control)
        })
        && lkg.last_modified.as_deref().is_none_or(|value| {
            safe_header_value(value).is_some() && !value.chars().any(char::is_control)
        })
        && lkg.warnings.len() <= MAX_WARNINGS
        && lkg.warnings.iter().all(|warning| {
            warning.len() <= MAX_WARNING_BYTES && !warning.chars().any(char::is_control)
        })
        && lkg.counts == lkg.catalog.counts()
        && lkg
            .catalog
            .providers
            .get("openai")
            .is_some_and(|provider| !provider.models.is_empty())
        && validate_catalog_semantics(&lkg.catalog)
        && content_hash(&lkg.catalog).is_ok_and(|hash| hash == lkg.content_hash)
}

fn validate_catalog_semantics(catalog: &BasellmCatalogContent) -> bool {
    catalog.providers.iter().all(|(provider_key, provider)| {
        valid_identifier(provider_key)
            && normalize_id(provider_key) == *provider_key
            && provider.provider_id == *provider_key
            && provider
                .display_name
                .as_deref()
                .is_none_or(|value| valid_metadata_text(value, MAX_METADATA_DISPLAY_NAME_BYTES))
            && valid_provider_aliases(&provider.models)
            && provider.models.iter().all(|(model_key, model)| {
                valid_identifier(model_key)
                    && valid_identifier(&model.model_id)
                    && valid_identifier(&model.source_provider_id)
                    && model.metadata.display_name.as_deref().is_none_or(|value| {
                        valid_metadata_text(value, MAX_METADATA_DISPLAY_NAME_BYTES)
                    })
                    && model.metadata.description.as_deref().is_none_or(|value| {
                        valid_metadata_text(value, MAX_METADATA_DESCRIPTION_BYTES)
                    })
                    && normalize_id(model_key) == *model_key
                    && normalize_id(&model.model_id) == *model_key
                    && canonical_provider(&model.source_provider_id).as_deref()
                        == Some(provider_key.as_str())
                    && model.price.as_ref().is_none_or(|price| {
                        price.base.input_per_1m_usd.is_some()
                            && price.base.output_per_1m_usd.is_some()
                            && valid_price_fields(&price.base)
                            && price.context_tiers.iter().all(|tier| {
                                tier.threshold_tokens > 0
                                    && !tier.prices.is_empty()
                                    && valid_price_fields(&tier.prices)
                            })
                            && price
                                .context_tiers
                                .windows(2)
                                .all(|pair| pair[0].threshold_tokens < pair[1].threshold_tokens)
                    })
            })
    })
}

fn valid_price_fields(prices: &BasellmCatalogPriceFields) -> bool {
    [
        &prices.input_per_1m_usd,
        &prices.output_per_1m_usd,
        &prices.cache_read_input_per_1m_usd,
        &prices.cache_creation_input_per_1m_usd,
    ]
    .into_iter()
    .flatten()
    .all(|price| canonical_price_decimal(price).as_deref() == Some(price.as_str()))
}

fn validate_attempt_state(attempt: &BasellmCatalogAttemptState) -> bool {
    attempt.schema_version == BASELLM_CATALOG_SCHEMA_VERSION
        && attempt.check_generation > 0
        && (attempt.source_url == "invalid-source-url"
            || valid_persisted_source_url(&attempt.source_url))
        && attempt
            .content_hash
            .as_deref()
            .is_none_or(valid_content_hash)
        && attempt
            .quarantined_candidate_hash
            .as_deref()
            .is_none_or(valid_content_hash)
}

fn decode_runtime_document(document: Option<RuntimeDocument>) -> BasellmCatalogRuntimeState {
    let Some(document) = document else {
        return BasellmCatalogRuntimeState::default();
    };
    if document.schema_version > BASELLM_CATALOG_SCHEMA_VERSION {
        return BasellmCatalogRuntimeState {
            lkg: BasellmCatalogLoad::UnsupportedSchema(document.schema_version),
            attempt: None,
            document_revision: Some(document.revision),
        };
    }
    if document.schema_version != BASELLM_CATALOG_SCHEMA_VERSION {
        return BasellmCatalogRuntimeState {
            lkg: BasellmCatalogLoad::Corrupt,
            attempt: None,
            document_revision: Some(document.revision),
        };
    }
    let Ok(payload) = serde_json::from_str::<BasellmCatalogRuntimeDocument>(&document.payload_json)
    else {
        return BasellmCatalogRuntimeState {
            lkg: BasellmCatalogLoad::Corrupt,
            attempt: None,
            document_revision: Some(document.revision),
        };
    };
    if payload.schema_version != BASELLM_CATALOG_SCHEMA_VERSION {
        return BasellmCatalogRuntimeState {
            lkg: BasellmCatalogLoad::Corrupt,
            attempt: None,
            document_revision: Some(document.revision),
        };
    }
    let lkg = match payload.lkg {
        Some(lkg) if validate_lkg(&lkg) => BasellmCatalogLoad::Valid(Arc::new(lkg)),
        Some(_) => BasellmCatalogLoad::Corrupt,
        None => BasellmCatalogLoad::Missing,
    };
    let attempt = match payload.attempt {
        Some(attempt) if validate_attempt_state(&attempt) => Some(attempt),
        Some(_) => {
            return BasellmCatalogRuntimeState {
                lkg: BasellmCatalogLoad::Corrupt,
                attempt: None,
                document_revision: Some(document.revision),
            };
        }
        None => None,
    };
    if let (BasellmCatalogLoad::Valid(lkg), Some(attempt)) = (&lkg, &attempt)
        && (attempt.content_hash.as_deref() != Some(lkg.content_hash.as_str())
            || attempt.content_generation != Some(lkg.content_generation))
    {
        return BasellmCatalogRuntimeState {
            lkg: BasellmCatalogLoad::Corrupt,
            attempt: None,
            document_revision: Some(document.revision),
        };
    }
    BasellmCatalogRuntimeState {
        lkg,
        attempt,
        document_revision: Some(document.revision),
    }
}

/// Loads BaseLLM state through the canonical runtime writer.
pub fn load_basellm_catalog_runtime_state(
    runtime_store: &RuntimeStore,
) -> Result<BasellmCatalogRuntimeState, RuntimeStoreError> {
    runtime_store
        .read_runtime_document(RuntimeDocumentKind::BasellmCatalog)
        .map(decode_runtime_document)
}

/// Loads BaseLLM state without acquiring runtime writer ownership.
pub fn load_basellm_catalog_runtime_state_from_reader(
    runtime_store: &RuntimeStoreReader,
) -> Result<BasellmCatalogRuntimeState, RuntimeStoreError> {
    runtime_store
        .read_runtime_document(RuntimeDocumentKind::BasellmCatalog)
        .map(decode_runtime_document)
}

/// Loads the existing default runtime state, treating an uninitialized store as never synced.
pub fn load_default_basellm_catalog_runtime_state()
-> Result<BasellmCatalogRuntimeState, RuntimeStoreError> {
    match RuntimeStoreReader::open_default() {
        Ok(runtime_store) => load_basellm_catalog_runtime_state_from_reader(&runtime_store),
        Err(RuntimeStoreError::DatabaseMissing { .. }) => Ok(BasellmCatalogRuntimeState::default()),
        Err(error) => Err(error),
    }
}

#[derive(Debug, Clone, Default)]
struct ResponseValidators {
    etag: Option<String>,
    last_modified: Option<String>,
}

enum FetchResult {
    Body {
        body: String,
        validators: ResponseValidators,
    },
    NotModified,
    Failure {
        category: BasellmSyncErrorCategory,
        retryable: bool,
        retry_after: Option<Duration>,
    },
}

/// Fetches and validates a BaseLLM document for an explicit manual import.
///
/// This path shares the canonical response bound, redirect policy, parser, and LKG validator, but
/// does not publish or persist runtime state. Error values intentionally carry no request URL.
pub async fn fetch_basellm_catalog_for_import(
    source_url: &str,
) -> Result<BasellmCatalogLkg, BasellmCatalogImportError> {
    let source =
        reqwest::Url::parse(source_url).map_err(|_| BasellmCatalogImportError::InvalidUrl)?;
    if !matches!(source.scheme(), "http" | "https") || source.host_str().is_none() {
        return Err(BasellmCatalogImportError::UnsupportedScheme);
    }
    if !source.username().is_empty() || source.password().is_some() {
        return Err(BasellmCatalogImportError::UserInfoNotAllowed);
    }

    let mut options = BasellmCatalogSyncOptions::default().with_source_url(source_url);
    options.require_https = false;
    let client = build_client(&source, &options).map_err(BasellmCatalogImportError::Fetch)?;
    let (body, validators) = match fetch_once(&client, source, None, DEFAULT_RESPONSE_LIMIT).await {
        FetchResult::Body { body, validators } => (body, validators),
        FetchResult::NotModified => {
            return Err(BasellmCatalogImportError::Fetch(
                BasellmSyncErrorCategory::Http,
            ));
        }
        FetchResult::Failure { category, .. } => {
            return Err(BasellmCatalogImportError::Fetch(category));
        }
    };
    let (catalog, warnings) = parse_basellm_catalog_json(&body)?;
    build_lkg(catalog, warnings, source_url, validators, None, unix_now())
        .map_err(BasellmCatalogImportError::Validation)
}

pub async fn sync_basellm_catalog(
    runtime_store: Arc<RuntimeStore>,
    options: BasellmCatalogSyncOptions,
) -> BasellmCatalogSyncReport {
    let observed_state = match load_basellm_catalog_runtime_state(runtime_store.as_ref()) {
        Ok(state) => state,
        Err(_) => {
            return runtime_unpersisted_report(
                &options,
                None,
                BasellmSyncOutcome::Unavailable,
                Some(BasellmSyncErrorCategory::Persistence),
                None,
                None,
            );
        }
    };
    if let BasellmCatalogLoad::UnsupportedSchema(version) = &observed_state.lkg {
        return runtime_unpersisted_report(
            &options,
            None,
            BasellmSyncOutcome::ReadOnly,
            Some(BasellmSyncErrorCategory::UnsupportedSchema),
            Some(*version),
            observed_state.attempt.as_ref(),
        );
    }
    let observed = match &observed_state.lkg {
        BasellmCatalogLoad::Valid(snapshot) => Some(snapshot.clone()),
        _ => None,
    };
    if let Some(snapshot) = &observed {
        install_catalog_snapshot(snapshot.clone());
    }

    let source = match reqwest::Url::parse(&options.source_url) {
        Ok(source)
            if (!options.require_https || source.scheme() == "https")
                && source.username().is_empty()
                && source.password().is_none() =>
        {
            source
        }
        _ => {
            return runtime_report_with_state(
                &options,
                runtime_store,
                &observed_state,
                observed,
                BasellmSyncOutcome::Unavailable,
                Some(BasellmSyncErrorCategory::Schema),
                None,
                None,
            )
            .await;
        }
    };
    let client = match build_client(&source, &options) {
        Ok(client) => client,
        Err(category) => {
            return runtime_report_with_state(
                &options,
                runtime_store,
                &observed_state,
                observed,
                BasellmSyncOutcome::Unavailable,
                Some(category),
                None,
                None,
            )
            .await;
        }
    };

    let sanitized_source = sanitize_source_url(&options.source_url);
    let source_changed = observed
        .as_ref()
        .is_some_and(|snapshot| snapshot.source_url != sanitized_source)
        || !source_path_may_be_persisted(&source);
    let mut unconditional = options.force || observed.is_none() || source_changed;
    let mut retried_unconditional_304 = false;
    let mut attempts = 0;
    loop {
        attempts += 1;
        let fetch = fetch_once(
            &client,
            source.clone(),
            (!unconditional).then_some(observed.as_deref()).flatten(),
            options.response_limit,
        )
        .await;
        match fetch {
            FetchResult::NotModified if observed.is_none() && !retried_unconditional_304 => {
                unconditional = true;
                retried_unconditional_304 = true;
            }
            FetchResult::NotModified if observed.is_none() || unconditional => {
                return runtime_report_with_state(
                    &options,
                    runtime_store,
                    &observed_state,
                    observed,
                    BasellmSyncOutcome::Unavailable,
                    Some(BasellmSyncErrorCategory::Semantic),
                    None,
                    None,
                )
                .await;
            }
            FetchResult::NotModified => {
                return runtime_report_with_state(
                    &options,
                    runtime_store,
                    &observed_state,
                    observed,
                    BasellmSyncOutcome::NotModified,
                    None,
                    None,
                    None,
                )
                .await;
            }
            FetchResult::Body { body, validators } => {
                let (catalog, warnings) = match parse_basellm_catalog_json(&body) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        let category = match error {
                            BasellmCatalogParseError::Json => BasellmSyncErrorCategory::Json,
                            BasellmCatalogParseError::InvalidPrice
                            | BasellmCatalogParseError::InvalidTier => {
                                BasellmSyncErrorCategory::Semantic
                            }
                            _ => BasellmSyncErrorCategory::Schema,
                        };
                        return runtime_report_with_state(
                            &options,
                            runtime_store,
                            &observed_state,
                            observed,
                            BasellmSyncOutcome::Unavailable,
                            Some(category),
                            None,
                            None,
                        )
                        .await;
                    }
                };
                let candidate_hash = match content_hash(&catalog) {
                    Ok(hash) => hash,
                    Err(_) => {
                        return runtime_report_with_state(
                            &options,
                            runtime_store,
                            &observed_state,
                            observed,
                            BasellmSyncOutcome::Unavailable,
                            Some(BasellmSyncErrorCategory::Schema),
                            None,
                            None,
                        )
                        .await;
                    }
                };
                if suspicious_count_collapse(observed.as_deref(), &catalog) {
                    return runtime_report_with_state(
                        &options,
                        runtime_store,
                        &observed_state,
                        observed,
                        BasellmSyncOutcome::Quarantined,
                        Some(BasellmSyncErrorCategory::Sanity),
                        Some(candidate_hash),
                        None,
                    )
                    .await;
                }
                if economic_change_requires_quarantine(
                    observed.as_deref(),
                    &catalog,
                    options.approved_economic_change_hash.as_deref(),
                ) {
                    return runtime_report_with_state(
                        &options,
                        runtime_store,
                        &observed_state,
                        observed,
                        BasellmSyncOutcome::Quarantined,
                        Some(BasellmSyncErrorCategory::EconomicAnomaly),
                        Some(candidate_hash),
                        None,
                    )
                    .await;
                }
                let candidate = match build_lkg(
                    catalog,
                    warnings,
                    &options.source_url,
                    validators,
                    observed.as_deref(),
                    unix_now(),
                ) {
                    Ok(candidate) => Arc::new(candidate),
                    Err(category) => {
                        return runtime_report_with_state(
                            &options,
                            runtime_store,
                            &observed_state,
                            observed,
                            BasellmSyncOutcome::Unavailable,
                            Some(category),
                            None,
                            None,
                        )
                        .await;
                    }
                };
                return runtime_report_with_state(
                    &options,
                    runtime_store,
                    &observed_state,
                    Some(candidate),
                    BasellmSyncOutcome::Updated,
                    None,
                    None,
                    None,
                )
                .await;
            }
            FetchResult::Failure {
                category: _,
                retryable,
                retry_after,
            } if retryable && attempts < options.max_attempts => {
                tokio::time::sleep(retry_wait_delay(attempts, retry_after)).await;
            }
            FetchResult::Failure {
                category,
                retry_after,
                ..
            } => {
                return runtime_report_with_state(
                    &options,
                    runtime_store,
                    &observed_state,
                    observed,
                    BasellmSyncOutcome::Unavailable,
                    Some(category),
                    None,
                    retry_after,
                )
                .await;
            }
        }
    }
}

fn retry_wait_delay(attempt: usize, retry_after: Option<Duration>) -> Duration {
    if let Some(retry_after) = retry_after {
        return bounded_retry_after(retry_after);
    }
    let factor = 1_u64 << attempt.saturating_sub(1).min(4);
    jittered(Duration::from_millis(250_u64.saturating_mul(factor)).min(Duration::from_secs(5)))
}

fn bounded_retry_after(retry_after: Duration) -> Duration {
    retry_after.min(MAX_RETRY_AFTER)
}

fn build_client(
    source: &reqwest::Url,
    options: &BasellmCatalogSyncOptions,
) -> Result<reqwest::Client, BasellmSyncErrorCategory> {
    let origin = (
        source.scheme().to_string(),
        source.host_str().map(str::to_string),
        source.port_or_known_default(),
    );
    let require_https = options.require_https;
    let redirect = reqwest::redirect::Policy::custom(move |attempt| {
        let next = attempt.url();
        let same_origin = next.scheme() == origin.0
            && next.host_str() == origin.1.as_deref()
            && next.port_or_known_default() == origin.2;
        if attempt.previous().len() > 3 {
            attempt.error("too many BaseLLM redirects")
        } else if !same_origin || (require_https && next.scheme() != "https") {
            attempt.error("BaseLLM redirect left the allowed HTTPS origin")
        } else {
            attempt.follow()
        }
    });
    reqwest::Client::builder()
        .connect_timeout(options.connect_timeout)
        .read_timeout(options.read_timeout)
        .timeout(options.total_timeout)
        .redirect(redirect)
        .build()
        .map_err(|_| BasellmSyncErrorCategory::Transport)
}

async fn fetch_once(
    client: &reqwest::Client,
    source: reqwest::Url,
    existing: Option<&BasellmCatalogLkg>,
    limit: usize,
) -> FetchResult {
    let mut request = client
        .get(source)
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(existing) = existing {
        if let Some(etag) = existing.etag.as_deref().and_then(safe_header_value) {
            request = request.header(IF_NONE_MATCH, etag);
        }
        if let Some(last_modified) = existing
            .last_modified
            .as_deref()
            .and_then(safe_header_value)
        {
            request = request.header(IF_MODIFIED_SINCE, last_modified);
        }
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let category = if error.is_timeout() {
                BasellmSyncErrorCategory::Timeout
            } else if error.is_redirect() {
                BasellmSyncErrorCategory::Redirect
            } else {
                BasellmSyncErrorCategory::Transport
            };
            return FetchResult::Failure {
                category,
                retryable: true,
                retry_after: None,
            };
        }
    };
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return FetchResult::NotModified;
    }
    if response.status() != reqwest::StatusCode::OK {
        let rate_limited = response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS;
        let retryable = rate_limited || response.status().is_server_error();
        let retry_after = parse_retry_after(response.headers());
        return FetchResult::Failure {
            category: if rate_limited {
                BasellmSyncErrorCategory::RateLimited
            } else {
                BasellmSyncErrorCategory::Http
            },
            retryable,
            retry_after,
        };
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return FetchResult::Failure {
            category: BasellmSyncErrorCategory::BodyTooLarge,
            retryable: false,
            retry_after: None,
        };
    }
    let validators = ResponseValidators {
        etag: bounded_header(response.headers(), ETAG),
        last_modified: bounded_header(response.headers(), LAST_MODIFIED),
    };
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                return FetchResult::Failure {
                    category: if error.is_timeout() {
                        BasellmSyncErrorCategory::Timeout
                    } else {
                        BasellmSyncErrorCategory::Transport
                    },
                    retryable: true,
                    retry_after: None,
                };
            }
        };
        if body.len().saturating_add(chunk.len()) > limit {
            return FetchResult::Failure {
                category: BasellmSyncErrorCategory::BodyTooLarge,
                retryable: false,
                retry_after: None,
            };
        }
        body.extend_from_slice(&chunk);
    }
    match String::from_utf8(body) {
        Ok(body) => FetchResult::Body { body, validators },
        Err(_) => FetchResult::Failure {
            category: BasellmSyncErrorCategory::Utf8,
            retryable: false,
            retry_after: None,
        },
    }
}

fn suspicious_count_collapse(
    existing: Option<&BasellmCatalogLkg>,
    candidate: &BasellmCatalogContent,
) -> bool {
    let Some(existing) = existing else {
        return false;
    };
    let old_openai = existing
        .catalog
        .providers
        .get("openai")
        .map(|provider| provider.models.len())
        .unwrap_or(0);
    let new_openai = candidate
        .providers
        .get("openai")
        .map(|provider| provider.models.len())
        .unwrap_or(0);
    (old_openai >= 10 && new_openai.saturating_mul(5) < old_openai.saturating_mul(3))
        || (existing.counts.model_count >= 100
            && candidate.counts().model_count.saturating_mul(5)
                < existing.counts.model_count.saturating_mul(3))
}

fn economic_anomaly(
    existing: Option<&BasellmCatalogLkg>,
    candidate: &BasellmCatalogContent,
) -> bool {
    let Some(existing) = existing else {
        return false;
    };
    for (provider_id, provider) in &candidate.providers {
        let Some(old_provider) = existing.catalog.providers.get(provider_id) else {
            continue;
        };
        for (model_id, model) in &provider.models {
            let Some(old_model) = old_provider.models.get(model_id) else {
                continue;
            };
            let (old_price, new_price) = match (&old_model.price, &model.price) {
                (Some(old_price), Some(new_price)) => (old_price, new_price),
                (None, None) => continue,
                (Some(_), None) | (None, Some(_)) => return true,
            };
            if tier_boundaries(old_price) != tier_boundaries(new_price) {
                return true;
            }
            if price_ratio_anomaly(&old_price.base, &new_price.base) {
                return true;
            }
            for (old_tier, new_tier) in old_price.context_tiers.iter().zip(&new_price.context_tiers)
            {
                if price_ratio_anomaly(&old_tier.prices, &new_tier.prices) {
                    return true;
                }
            }
        }
    }
    false
}

fn economic_change_requires_quarantine(
    existing: Option<&BasellmCatalogLkg>,
    candidate: &BasellmCatalogContent,
    approved_hash: Option<&str>,
) -> bool {
    economic_anomaly(existing, candidate)
        && content_hash(candidate)
            .ok()
            .as_deref()
            .is_none_or(|candidate_hash| Some(candidate_hash) != approved_hash)
}

fn tier_boundaries(price: &BasellmCatalogPrice) -> Vec<u64> {
    price
        .context_tiers
        .iter()
        .map(|tier| tier.threshold_tokens)
        .collect()
}

fn price_ratio_anomaly(old: &BasellmCatalogPriceFields, new: &BasellmCatalogPriceFields) -> bool {
    [
        (&old.input_per_1m_usd, &new.input_per_1m_usd),
        (&old.output_per_1m_usd, &new.output_per_1m_usd),
        (
            &old.cache_read_input_per_1m_usd,
            &new.cache_read_input_per_1m_usd,
        ),
        (
            &old.cache_creation_input_per_1m_usd,
            &new.cache_creation_input_per_1m_usd,
        ),
    ]
    .into_iter()
    .any(|(old, new)| match (old, new) {
        (Some(old), Some(new)) => {
            let old = decimal_femto(old).unwrap_or(0);
            let new = decimal_femto(new).unwrap_or(0);
            (old == 0 && new > 0)
                || (old > 0 && (new > old.saturating_mul(10) || new.saturating_mul(10) < old))
        }
        (None, None) => false,
        (Some(_), None) | (None, Some(_)) => true,
    })
}

fn decimal_femto(value: &str) -> Option<i128> {
    let canonical = canonical_price_decimal(value)?;
    let (whole, fractional) = canonical.split_once('.').unwrap_or((&canonical, ""));
    let mut fractional = fractional.to_string();
    fractional.extend(std::iter::repeat_n(
        '0',
        15_usize.saturating_sub(fractional.len()),
    ));
    whole
        .parse::<i128>()
        .ok()?
        .checked_mul(1_000_000_000_000_000)?
        .checked_add(fractional.parse::<i128>().ok()?)
}

#[allow(clippy::too_many_arguments)]
async fn runtime_report_with_state(
    options: &BasellmCatalogSyncOptions,
    runtime_store: Arc<RuntimeStore>,
    observed: &BasellmCatalogRuntimeState,
    snapshot: Option<Arc<BasellmCatalogLkg>>,
    outcome: BasellmSyncOutcome,
    category: Option<BasellmSyncErrorCategory>,
    quarantined_candidate_hash: Option<String>,
    retry_after: Option<Duration>,
) -> BasellmCatalogSyncReport {
    let retained_snapshot = match &observed.lkg {
        BasellmCatalogLoad::Valid(snapshot) => Some(snapshot.clone()),
        _ => None,
    };
    if matches!(&observed.lkg, BasellmCatalogLoad::Corrupt)
        && outcome != BasellmSyncOutcome::Updated
    {
        return runtime_unpersisted_report(
            options,
            snapshot,
            outcome,
            category,
            None,
            observed.attempt.as_ref(),
        );
    }

    let mut attempt = attempt_state_from_previous(
        options,
        observed.attempt.as_ref(),
        outcome,
        category,
        snapshot.as_deref(),
        None,
        retry_after,
    );
    attempt.quarantined_candidate_hash = quarantined_candidate_hash;
    let payload = BasellmCatalogRuntimeDocument {
        schema_version: BASELLM_CATALOG_SCHEMA_VERSION,
        lkg: snapshot.as_deref().cloned(),
        attempt: Some(attempt.clone()),
    };
    let payload_json = match serde_json::to_string(&payload) {
        Ok(payload_json) => payload_json,
        Err(_) => {
            return runtime_unpersisted_report(
                options,
                retained_snapshot,
                BasellmSyncOutcome::Unavailable,
                Some(BasellmSyncErrorCategory::Persistence),
                None,
                observed.attempt.as_ref(),
            );
        }
    };
    let expected_revision = observed.document_revision;
    let commit = tokio::task::spawn_blocking(move || {
        runtime_store.compare_and_write_runtime_document(
            expected_revision,
            RuntimeDocumentWrite {
                kind: RuntimeDocumentKind::BasellmCatalog,
                schema_version: BASELLM_CATALOG_SCHEMA_VERSION,
                payload_json: &payload_json,
            },
        )
    })
    .await;
    match commit {
        Ok(Ok(RuntimeDocumentCommit::Committed(_))) => {
            replace_catalog_snapshot(snapshot.clone());
            BasellmCatalogSyncReport {
                outcome,
                snapshot,
                attempt,
            }
        }
        Ok(Ok(RuntimeDocumentCommit::Stale(current))) => {
            runtime_stale_report(options, decode_runtime_document(current))
        }
        Ok(Err(_)) | Err(_) => runtime_unpersisted_report(
            options,
            retained_snapshot,
            BasellmSyncOutcome::Unavailable,
            Some(BasellmSyncErrorCategory::Persistence),
            None,
            observed.attempt.as_ref(),
        ),
    }
}

fn runtime_stale_report(
    options: &BasellmCatalogSyncOptions,
    current: BasellmCatalogRuntimeState,
) -> BasellmCatalogSyncReport {
    match current.lkg {
        BasellmCatalogLoad::UnsupportedSchema(version) => runtime_unpersisted_report(
            options,
            None,
            BasellmSyncOutcome::ReadOnly,
            Some(BasellmSyncErrorCategory::UnsupportedSchema),
            Some(version),
            current.attempt.as_ref(),
        ),
        BasellmCatalogLoad::Valid(snapshot) => {
            install_catalog_snapshot(snapshot.clone());
            runtime_unpersisted_report(
                options,
                Some(snapshot),
                BasellmSyncOutcome::StaleResponse,
                Some(BasellmSyncErrorCategory::StaleResponse),
                None,
                current.attempt.as_ref(),
            )
        }
        BasellmCatalogLoad::Missing | BasellmCatalogLoad::Corrupt => {
            replace_catalog_snapshot(None);
            runtime_unpersisted_report(
                options,
                None,
                BasellmSyncOutcome::StaleResponse,
                Some(BasellmSyncErrorCategory::StaleResponse),
                None,
                current.attempt.as_ref(),
            )
        }
    }
}

fn runtime_unpersisted_report(
    options: &BasellmCatalogSyncOptions,
    snapshot: Option<Arc<BasellmCatalogLkg>>,
    outcome: BasellmSyncOutcome,
    category: Option<BasellmSyncErrorCategory>,
    read_only_schema: Option<u32>,
    previous_attempt: Option<&BasellmCatalogAttemptState>,
) -> BasellmCatalogSyncReport {
    let attempt = attempt_state_from_previous(
        options,
        previous_attempt,
        outcome,
        category,
        snapshot.as_deref(),
        read_only_schema,
        None,
    );
    BasellmCatalogSyncReport {
        outcome,
        snapshot,
        attempt,
    }
}

fn attempt_state_from_previous(
    options: &BasellmCatalogSyncOptions,
    previous: Option<&BasellmCatalogAttemptState>,
    outcome: BasellmSyncOutcome,
    category: Option<BasellmSyncErrorCategory>,
    snapshot: Option<&BasellmCatalogLkg>,
    read_only_schema_version: Option<u32>,
    retry_after: Option<Duration>,
) -> BasellmCatalogAttemptState {
    let now = unix_now();
    BasellmCatalogAttemptState {
        schema_version: BASELLM_CATALOG_SCHEMA_VERSION,
        check_generation: previous
            .map(|attempt| attempt.check_generation.saturating_add(1))
            .unwrap_or(1),
        source_url: sanitize_source_url(&options.source_url),
        last_checked_at_unix: now,
        outcome,
        last_error_category: category,
        content_hash: snapshot.map(|snapshot| snapshot.content_hash.clone()),
        content_generation: snapshot.map(|snapshot| snapshot.content_generation),
        quarantined_candidate_hash: None,
        read_only_schema_version,
        retry_after_unix: retry_after
            .map(bounded_retry_after)
            .map(|delay| now.saturating_add(i64::try_from(delay.as_secs()).unwrap_or(i64::MAX))),
    }
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(bounded_retry_after(Duration::from_secs(seconds)));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    Some(
        date.duration_since(SystemTime::now())
            .unwrap_or_default()
            .min(MAX_RETRY_AFTER),
    )
}

fn bounded_header(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 1024)
        .filter(|value| !value.chars().any(char::is_control))
        .map(str::to_string)
}

fn safe_header_value(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= 1024
        && !value
            .chars()
            .any(|character| character == '\r' || character == '\n'))
    .then_some(value)
}

fn sanitize_source_url(source_url: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(source_url) else {
        return "invalid-source-url".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    if !source_path_may_be_persisted(&url) {
        url.set_path("/");
    }
    url.to_string()
}

fn source_path_may_be_persisted(source: &reqwest::Url) -> bool {
    let mut source = source.clone();
    let _ = source.set_username("");
    let _ = source.set_password(None);
    source.set_query(None);
    source.set_fragment(None);

    if reqwest::Url::parse(basellm_all_json_url()).is_ok_and(|official| source == official) {
        return true;
    }
    source.scheme() == "http"
        && source
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|address| address.is_loopback())
}

fn valid_persisted_source_url(source_url: &str) -> bool {
    reqwest::Url::parse(source_url).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && sanitize_source_url(source_url) == source_url
    })
}

fn valid_content_hash(content_hash: &str) -> bool {
    content_hash.len() == 71
        && content_hash
            .strip_prefix("sha256:")
            .is_some_and(|digest| digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn json_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

fn normalize_id(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn valid_identifier(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn valid_metadata_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn push_warning(warnings: &mut Vec<String>, mut warning: String) {
    if warnings.len() >= MAX_WARNINGS {
        return;
    }
    if warning.len() > MAX_WARNING_BYTES {
        let mut boundary = MAX_WARNING_BYTES;
        while !warning.is_char_boundary(boundary) {
            boundary = boundary.saturating_sub(1);
        }
        warning.truncate(boundary);
    }
    warnings.push(warning);
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn jittered(duration: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    let percent = 75_u128 + u128::from(nanos % 51);
    let millis = duration.as_millis().saturating_mul(percent) / 100;
    Duration::from_millis(u64::try_from(millis).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{HeaderMap as AxumHeaderMap, StatusCode, header};
    use axum::response::{IntoResponse, Redirect, Response};
    use axum::routing::get;

    use super::*;

    type ConditionalHeaders = Vec<(Option<String>, Option<String>)>;

    #[derive(Clone)]
    struct HttpFixture {
        calls: Arc<AtomicUsize>,
        conditional_headers: Arc<Mutex<ConditionalHeaders>>,
        body: Arc<String>,
    }

    async fn catalog_fixture_handler(
        State(fixture): State<HttpFixture>,
        headers: AxumHeaderMap,
    ) -> Response {
        let call = fixture.calls.fetch_add(1, Ordering::SeqCst);
        let etag = headers
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let last_modified = headers
            .get(header::IF_MODIFIED_SINCE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        fixture
            .conditional_headers
            .lock()
            .expect("fixture headers lock")
            .push((etag, last_modified));

        match call {
            0 => (
                StatusCode::OK,
                [
                    (header::ETAG, "W/\"fixture-v1\""),
                    (header::LAST_MODIFIED, "Sat, 11 Jul 2026 00:00:00 GMT"),
                ],
                fixture.body.as_str().to_string(),
            )
                .into_response(),
            1 => StatusCode::NOT_MODIFIED.into_response(),
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }

    fn fixture(input: &str, tier_input: &str, threshold: u64) -> String {
        format!(
            r#"{{
              "openai": {{
                "name": "OpenAI",
                "models": {{
                  "gpt-test": {{
                    "name": "GPT Test",
                    "description": "Test model",
                    "aliases": ["relay-gpt-test"],
                    "limit": {{ "input": 100000, "context": 128000 }},
                    "modalities": {{ "input": ["text", "image", "pdf"] }},
                    "reasoning": true,
                    "tool_call": true,
                    "structured_output": true,
                    "experimental": {{
                      "modes": {{
                        "fast": {{
                          "provider": {{ "body": {{ "service_tier": "priority" }} }}
                        }}
                      }}
                    }},
                    "cost": {{
                      "input": {input}, "output": 30, "cache_read": 0.5,
                      "tiers": [{{
                        "tier": {{ "type": "context", "size": {threshold} }},
                        "input": {tier_input}, "output": 45
                      }}]
                    }}
                  }}
                }}
              }},
              "routing-run": {{
                "models": {{
                  "gpt-test": {{ "cost": {{ "input": 1, "output": 2 }} }}
                }}
              }}
            }}"#
        )
    }

    fn fixture_with_metadata(mutate: impl FnOnce(&mut Value)) -> String {
        let mut value =
            serde_json::from_str(&fixture("5", "10", 272_000)).expect("decode metadata fixture");
        mutate(&mut value);
        serde_json::to_string(&value).expect("encode metadata fixture")
    }

    fn fixture_with_unknown_tier() -> String {
        let mut value: Value =
            serde_json::from_str(&fixture("5", "10", 272_000)).expect("decode tier fixture");
        let tiers = value["openai"]["models"]["gpt-test"]["cost"]["tiers"]
            .as_array_mut()
            .expect("fixture tiers");
        tiers.insert(
            0,
            serde_json::json!({
                "tier": { "type": "request_count", "size": 0 },
                "input": -1,
                "output": "not-a-price"
            }),
        );
        serde_json::to_string(&value).expect("encode tier fixture")
    }

    fn lkg_from_fixture(text: &str, generation: u64) -> BasellmCatalogLkg {
        let (catalog, warnings) = parse_basellm_catalog_json(text).expect("parse fixture");
        let mut lkg = build_lkg(
            catalog,
            warnings,
            "https://example.test/all.json",
            ResponseValidators::default(),
            None,
            1,
        )
        .expect("build LKG");
        lkg.body_generation = generation;
        lkg.content_generation = generation;
        lkg
    }

    async fn assert_rejected_refresh_retains_valid_lkg(
        case: &str,
        body: Vec<u8>,
        expected_category: BasellmSyncErrorCategory,
    ) {
        let response_body = body;
        let app = Router::new().route(
            "/all.json",
            get(move || {
                let body = response_body.clone();
                async move {
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from(body))
                        .expect("build fixture response")
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind invalid-candidate fixture");
        let address = listener.local_addr().expect("fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve invalid-candidate fixture");
        });

        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let options = BasellmCatalogSyncOptions::default()
            .with_source_url(format!("http://{address}/all.json"))
            .with_max_attempts(1)
            .allow_http_for_fixture();
        let lkg = lkg_from_fixture(&fixture("5", "10", 272_000), 7);
        let initial_attempt = attempt_state_from_previous(
            &options,
            None,
            BasellmSyncOutcome::Updated,
            None,
            Some(&lkg),
            None,
            None,
        );
        let initial_document = BasellmCatalogRuntimeDocument {
            schema_version: BASELLM_CATALOG_SCHEMA_VERSION,
            lkg: Some(lkg.clone()),
            attempt: Some(initial_attempt),
        };
        let payload_json =
            serde_json::to_string(&initial_document).expect("encode initial runtime document");
        runtime_store
            .write_runtime_documents(&[RuntimeDocumentWrite {
                kind: RuntimeDocumentKind::BasellmCatalog,
                schema_version: BASELLM_CATALOG_SCHEMA_VERSION,
                payload_json: &payload_json,
            }])
            .expect("seed valid runtime document");
        let before_state = load_basellm_catalog_runtime_state(runtime_store.as_ref())
            .expect("load initial runtime state");
        assert_eq!(before_state.document_revision, Some(1), "{case}");
        assert_eq!(
            before_state.lkg,
            BasellmCatalogLoad::Valid(Arc::new(lkg.clone())),
            "{case}"
        );
        install_catalog_snapshot(Arc::new(lkg.clone()));
        let effective_before = crate::pricing::refresh_effective_pricing_catalog();

        let report = sync_basellm_catalog(Arc::clone(&runtime_store), options).await;

        assert_eq!(report.outcome, BasellmSyncOutcome::Unavailable, "{case}");
        assert_eq!(
            report.attempt.last_error_category,
            Some(expected_category),
            "{case}"
        );
        assert_eq!(report.attempt.check_generation, 2, "{case}");
        assert_eq!(report.snapshot.as_deref(), Some(&lkg), "{case}");
        let after_state = load_basellm_catalog_runtime_state(runtime_store.as_ref())
            .expect("load rejected-refresh runtime state");
        assert_eq!(after_state.document_revision, Some(2), "{case}");
        assert_eq!(
            after_state.lkg,
            BasellmCatalogLoad::Valid(Arc::new(lkg.clone())),
            "{case}"
        );
        let persisted_attempt = after_state.attempt.expect("persisted rejected attempt");
        assert_eq!(persisted_attempt.check_generation, 2, "{case}");
        assert_eq!(
            persisted_attempt.last_error_category,
            Some(expected_category),
            "{case}"
        );
        assert_eq!(basellm_catalog_snapshot().as_deref(), Some(&lkg), "{case}");

        let effective_after = crate::pricing::refresh_effective_pricing_catalog();
        assert_eq!(
            effective_after.revision, effective_before.revision,
            "{case}"
        );
        assert_eq!(
            effective_after.catalog_snapshot(),
            effective_before.catalog_snapshot(),
            "{case}"
        );

        server.abort();
        let _ = server.await;
    }

    #[test]
    fn parser_keeps_provider_namespace_and_context_tier() {
        let (catalog, warnings) =
            parse_basellm_catalog_json(&fixture("5", "10", 272_000)).expect("parse");
        assert!(warnings.is_empty());
        let openai = catalog.model("openai", "GPT-TEST").expect("openai row");
        let routing = catalog
            .model("routing-run", "gpt-test")
            .expect("routing row");
        assert_eq!(
            openai
                .price
                .as_ref()
                .expect("price")
                .base
                .input_per_1m_usd
                .as_deref(),
            Some("5")
        );
        assert_eq!(
            routing
                .price
                .as_ref()
                .expect("price")
                .base
                .input_per_1m_usd
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            openai.price.as_ref().expect("price").context_tiers[0].threshold_tokens,
            272_000
        );

        let lkg = lkg_from_fixture(&fixture("5", "10", 272_000), 1);
        let openai_prices = lkg.model_prices_for_provider("openai");
        let routing_prices = lkg.model_prices_for_provider("routing-run");
        assert_eq!(openai_prices.len(), 1);
        assert_eq!(routing_prices.len(), 1);
        assert_eq!(openai_prices[0].provider, "openai");
        assert_eq!(routing_prices[0].provider, "routing-run");
        assert_eq!(openai.aliases, vec!["relay-gpt-test"]);
        assert_eq!(openai.metadata.display_name.as_deref(), Some("GPT Test"));
        assert_eq!(openai.metadata.description.as_deref(), Some("Test model"));
        assert_eq!(openai.metadata.context_window, Some(100_000));
        assert_eq!(openai.metadata.max_context_window, Some(128_000));
        assert_eq!(openai.metadata.input_modalities, vec!["text", "image"]);
        assert_eq!(openai.metadata.reasoning, Some(true));
        assert_eq!(openai.metadata.tool_call, Some(true));
        assert_eq!(openai.metadata.structured_output, Some(true));
        assert!(openai.metadata.supports_fast_priority);
        assert_eq!(openai_prices[0].aliases, vec!["relay-gpt-test"]);
        assert_eq!(openai.source_provider_id, "openai");
        assert_eq!(routing.source_provider_id, "routing-run");
        assert_eq!(openai_prices[0].tiers[0].threshold_tokens, 272_000);
        assert_eq!(
            openai_prices[0].source_generation.as_deref(),
            Some(lkg.content_hash.as_str())
        );
    }

    #[test]
    fn parser_warns_and_skips_unknown_tier_without_losing_valid_context_tier() {
        let candidate = fixture_with_unknown_tier();
        let (catalog, warnings) = parse_basellm_catalog_json(&candidate).expect("parse fixture");

        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0], "skipped unknown tier for openai/gpt-test");
        let price = catalog
            .model("openai", "gpt-test")
            .and_then(|model| model.price.as_ref())
            .expect("openai price");
        assert_eq!(price.context_tiers.len(), 1);
        assert_eq!(price.context_tiers[0].threshold_tokens, 272_000);
        assert_eq!(
            price.context_tiers[0].prices.input_per_1m_usd.as_deref(),
            Some("10")
        );
        assert!(validate_catalog_semantics(&catalog));

        let lkg = lkg_from_fixture(&candidate, 1);
        assert_eq!(lkg.warnings, warnings);
        assert!(validate_lkg(&lkg));
        let prices = lkg.model_prices_for_provider("openai");
        assert_eq!(prices.len(), 1);
        assert_eq!(prices[0].tiers.len(), 1);
        assert_eq!(prices[0].tiers[0].threshold_tokens, 272_000);
    }

    #[test]
    fn parser_rejects_missing_openai_and_invalid_known_tier() {
        assert_eq!(
            parse_basellm_catalog_json(r#"{"routing-run":{"models":{}}}"#),
            Err(BasellmCatalogParseError::MissingOpenAiModels)
        );
        let invalid = fixture("5", "-10", 272_000);
        assert_eq!(
            parse_basellm_catalog_json(&invalid),
            Err(BasellmCatalogParseError::InvalidPrice)
        );
    }

    #[test]
    fn parser_rejects_control_characters_and_oversized_display_metadata() {
        let candidates = [
            fixture_with_metadata(|root| {
                root["openai"]["name"] = Value::String("OpenAI\u{1b}[31m".to_string());
            }),
            fixture_with_metadata(|root| {
                root["openai"]["models"]["gpt-test"]["name"] =
                    Value::String("GPT\u{7} Test".to_string());
            }),
            fixture_with_metadata(|root| {
                root["openai"]["models"]["gpt-test"]["description"] =
                    Value::String("unsafe\nterminal text".to_string());
            }),
            fixture_with_metadata(|root| {
                root["openai"]["name"] =
                    Value::String("x".repeat(MAX_METADATA_DISPLAY_NAME_BYTES + 1));
            }),
            fixture_with_metadata(|root| {
                root["openai"]["models"]["gpt-test"]["name"] =
                    Value::String("x".repeat(MAX_METADATA_DISPLAY_NAME_BYTES + 1));
            }),
            fixture_with_metadata(|root| {
                root["openai"]["models"]["gpt-test"]["description"] =
                    Value::String("x".repeat(MAX_METADATA_DESCRIPTION_BYTES + 1));
            }),
        ];

        for candidate in candidates {
            assert_eq!(
                parse_basellm_catalog_json(&candidate),
                Err(BasellmCatalogParseError::InvalidMetadata)
            );
        }
    }

    #[test]
    fn lkg_semantics_reject_control_characters_in_display_metadata() {
        let baseline = lkg_from_fixture(&fixture("5", "10", 272_000), 1);
        let mut candidates = Vec::new();

        let mut provider_escape = baseline.clone();
        provider_escape
            .catalog
            .providers
            .get_mut("openai")
            .expect("openai provider")
            .display_name = Some("OpenAI\u{1b}[31m".to_string());
        candidates.push(provider_escape);

        let mut model_escape = baseline.clone();
        model_escape
            .catalog
            .providers
            .get_mut("openai")
            .and_then(|provider| provider.models.get_mut("gpt-test"))
            .expect("openai model")
            .metadata
            .display_name = Some("GPT\u{7} Test".to_string());
        candidates.push(model_escape);

        let mut description_escape = baseline.clone();
        description_escape
            .catalog
            .providers
            .get_mut("openai")
            .and_then(|provider| provider.models.get_mut("gpt-test"))
            .expect("openai model")
            .metadata
            .description = Some("unsafe\nterminal text".to_string());
        candidates.push(description_escape);

        let mut oversized_description = baseline;
        oversized_description
            .catalog
            .providers
            .get_mut("openai")
            .and_then(|provider| provider.models.get_mut("gpt-test"))
            .expect("openai model")
            .metadata
            .description = Some("x".repeat(MAX_METADATA_DESCRIPTION_BYTES + 1));
        candidates.push(oversized_description);

        for mut candidate in candidates {
            candidate.content_hash = content_hash(&candidate.catalog).expect("candidate hash");
            assert!(!validate_lkg(&candidate));
        }
    }

    #[tokio::test]
    async fn malformed_json_200_retains_valid_lkg_and_effective_catalog() {
        assert_rejected_refresh_retains_valid_lkg(
            "malformed-json",
            br#"{"openai":{"models":  "#.to_vec(),
            BasellmSyncErrorCategory::Json,
        )
        .await;
    }

    #[tokio::test]
    async fn invalid_utf8_200_retains_valid_lkg_and_effective_catalog() {
        assert_rejected_refresh_retains_valid_lkg(
            "invalid-utf8",
            vec![b'{', 0xff, b'}'],
            BasellmSyncErrorCategory::Utf8,
        )
        .await;
    }

    #[tokio::test]
    async fn invalid_known_tier_200_retains_valid_lkg_and_effective_catalog() {
        assert_rejected_refresh_retains_valid_lkg(
            "invalid-tier",
            fixture("5", "10", 0).into_bytes(),
            BasellmSyncErrorCategory::Semantic,
        )
        .await;
    }

    #[tokio::test]
    async fn unsafe_metadata_200_retains_valid_lkg_and_installed_catalog() {
        let candidate = fixture_with_metadata(|root| {
            root["openai"]["models"]["gpt-test"]["description"] =
                Value::String("clear screen: \u{1b}[2J".to_string());
        });
        assert_rejected_refresh_retains_valid_lkg(
            "unsafe-metadata",
            candidate.into_bytes(),
            BasellmSyncErrorCategory::Schema,
        )
        .await;
    }

    #[test]
    fn parser_rejects_canonical_provider_model_collisions() {
        let model_collision = r#"{
          "openai": {
            "models": {
              "GPT-Test": { "cost": { "input": 1, "output": 2 } },
              "gpt-test": { "cost": { "input": 3, "output": 4 } }
            }
          }
        }"#;
        assert!(parse_basellm_catalog_json(model_collision).is_err());

        let provider_collision = r#"{
          "codex": {
            "models": {
              "gpt-test": { "cost": { "input": 1, "output": 2 } }
            }
          },
          "openai": {
            "models": {
              "GPT-TEST": { "cost": { "input": 3, "output": 4 } }
            }
          }
        }"#;
        assert!(parse_basellm_catalog_json(provider_collision).is_err());

        let source_provider = r#"{
          "Codex": {
            "models": {
              "gpt-source": { "cost": { "input": 1, "output": 2 } }
            }
          }
        }"#;
        let (catalog, _) = parse_basellm_catalog_json(source_provider).expect("canonical provider");
        let model = catalog
            .model("openai", "gpt-source")
            .expect("canonical provider model");
        assert_eq!(model.source_provider_id, "Codex");
    }

    #[test]
    fn normalized_content_hash_is_stable() {
        let (left, _) = parse_basellm_catalog_json(&fixture("5.0", "1e1", 272_000)).unwrap();
        let (right, _) = parse_basellm_catalog_json(&fixture("5", "10", 272_000)).unwrap();
        assert_eq!(content_hash(&left).unwrap(), content_hash(&right).unwrap());
    }

    #[test]
    fn economic_changes_are_quarantined() {
        let existing = lkg_from_fixture(&fixture("5", "10", 272_000), 1);
        let (price_shock, _) = parse_basellm_catalog_json(&fixture("100", "10", 272_000)).unwrap();
        let (tier_move, _) = parse_basellm_catalog_json(&fixture("5", "10", 200_000)).unwrap();
        assert!(economic_anomaly(Some(&existing), &price_shock));
        assert!(economic_anomaly(Some(&existing), &tier_move));

        let mut zero_to_positive = existing.catalog.clone();
        let price = zero_to_positive
            .providers
            .get_mut("openai")
            .and_then(|provider| provider.models.get_mut("gpt-test"))
            .and_then(|model| model.price.as_mut())
            .expect("fixture price");
        price.base.cache_read_input_per_1m_usd = Some("0".to_string());
        let zero_existing = BasellmCatalogLkg {
            content_hash: content_hash(&zero_to_positive).expect("zero-price hash"),
            counts: zero_to_positive.counts(),
            catalog: zero_to_positive,
            ..existing
        };
        let (positive_cache_price, _) =
            parse_basellm_catalog_json(&fixture("5", "10", 272_000)).unwrap();
        assert!(economic_anomaly(
            Some(&zero_existing),
            &positive_cache_price
        ));
    }

    #[test]
    fn quarantine_approval_is_bound_to_candidate_hash() {
        let existing = lkg_from_fixture(&fixture("5", "10", 272_000), 1);
        let (approved_candidate, _) =
            parse_basellm_catalog_json(&fixture("100", "10", 272_000)).unwrap();
        let (changed_candidate, _) =
            parse_basellm_catalog_json(&fixture("101", "10", 272_000)).unwrap();
        let approved_hash = content_hash(&approved_candidate).expect("candidate hash");
        let options = BasellmCatalogSyncOptions::default()
            .with_approved_quarantine_hash(approved_hash.clone());
        assert_eq!(
            options.approved_economic_change_hash.as_deref(),
            Some(approved_hash.as_str())
        );

        assert!(!economic_change_requires_quarantine(
            Some(&existing),
            &approved_candidate,
            Some(&approved_hash)
        ));
        assert!(economic_change_requires_quarantine(
            Some(&existing),
            &changed_candidate,
            Some(&approved_hash)
        ));
        assert!(economic_change_requires_quarantine(
            Some(&existing),
            &approved_candidate,
            None
        ));
    }

    #[test]
    fn retry_after_is_a_bounded_minimum_wait() {
        let server_minimum = Duration::from_secs(120);
        let wait = retry_wait_delay(1, Some(server_minimum));
        assert!(wait >= server_minimum);
        assert!(wait <= Duration::from_secs(300));

        assert_eq!(
            retry_wait_delay(5, Some(Duration::from_secs(900))),
            Duration::from_secs(300)
        );

        let options = BasellmCatalogSyncOptions::default();
        let attempt = attempt_state_from_previous(
            &options,
            None,
            BasellmSyncOutcome::Unavailable,
            Some(BasellmSyncErrorCategory::RateLimited),
            None,
            None,
            Some(Duration::from_secs(900)),
        );
        assert_eq!(
            attempt.retry_after_unix,
            Some(attempt.last_checked_at_unix.saturating_add(300))
        );
    }

    #[test]
    fn persisted_custom_source_url_does_not_expose_path_credentials() {
        assert_eq!(
            sanitize_source_url(
                "https://relay.example.test/accounts/path-secret/catalog.json?api_key=query-secret#fragment-secret"
            ),
            "https://relay.example.test/"
        );
        assert_eq!(
            sanitize_source_url(basellm_all_json_url()),
            basellm_all_json_url()
        );
    }

    #[tokio::test]
    async fn runtime_future_schema_is_read_only_and_revision_stable() {
        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        runtime_store
            .write_runtime_documents(&[RuntimeDocumentWrite {
                kind: RuntimeDocumentKind::BasellmCatalog,
                schema_version: 999,
                payload_json: r#"{"opaque":"future-state"}"#,
            }])
            .expect("seed future runtime document");
        let before = runtime_store
            .read_runtime_document(RuntimeDocumentKind::BasellmCatalog)
            .expect("read future document")
            .expect("future document");

        let report = sync_basellm_catalog(
            Arc::clone(&runtime_store),
            BasellmCatalogSyncOptions::default().with_source_url("invalid source URL"),
        )
        .await;

        assert_eq!(report.outcome, BasellmSyncOutcome::ReadOnly);
        assert_eq!(report.attempt.read_only_schema_version, Some(999));
        let after = runtime_store
            .read_runtime_document(RuntimeDocumentKind::BasellmCatalog)
            .expect("reread future document")
            .expect("future document");
        assert_eq!(after, before);
    }

    #[tokio::test]
    async fn manual_import_fetch_rejects_oversized_body_without_leaking_source_secrets() {
        let app = Router::new().route(
            "/all.json",
            get(|| async { vec![b'x'; DEFAULT_RESPONSE_LIMIT + 1] }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind oversized import fixture");
        let address = listener.local_addr().expect("oversized fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve oversized import fixture");
        });
        let secret = "oversized-query-secret";
        let source = format!("http://{address}/all.json?token={secret}#private-fragment");

        let error = fetch_basellm_catalog_for_import(&source)
            .await
            .expect_err("oversized BaseLLM import response");

        assert_eq!(
            error,
            BasellmCatalogImportError::Fetch(BasellmSyncErrorCategory::BodyTooLarge)
        );
        let message = error.to_string();
        assert!(!message.contains(secret));
        assert!(!message.contains("private-fragment"));
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn manual_import_fetch_rejects_userinfo_without_echoing_credentials() {
        let secret = "userinfo-secret";
        let source = format!(
            "https://operator:{secret}@example.invalid/all.json?token=query-secret#private-fragment"
        );

        let error = fetch_basellm_catalog_for_import(&source)
            .await
            .expect_err("userinfo must be rejected");

        assert_eq!(error, BasellmCatalogImportError::UserInfoNotAllowed);
        let message = error.to_string();
        for forbidden in [secret, "query-secret", "private-fragment", "operator"] {
            assert!(
                !message.contains(forbidden),
                "leaked {forbidden}: {message}"
            );
        }
    }

    #[tokio::test]
    async fn fetch_rejects_partial_content_cross_origin_redirect_and_body_overflow() {
        let target_app = Router::new().route(
            "/all.json",
            get(|| async { (StatusCode::OK, "should not be reached") }),
        );
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind redirect target");
        let target_address = target_listener.local_addr().expect("target address");
        let target_server = tokio::spawn(async move {
            axum::serve(target_listener, target_app)
                .await
                .expect("serve redirect target");
        });

        let redirect_target = format!("http://{target_address}/all.json");
        let source_app = Router::new()
            .route(
                "/partial",
                get(|| async { (StatusCode::PARTIAL_CONTENT, "{}") }),
            )
            .route(
                "/overflow",
                get(|| async { (StatusCode::OK, "response exceeds limit") }),
            )
            .route(
                "/redirect",
                get(move || {
                    let redirect_target = redirect_target.clone();
                    async move { Redirect::temporary(&redirect_target) }
                }),
            );
        let source_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind source");
        let source_address = source_listener.local_addr().expect("source address");
        let source_server = tokio::spawn(async move {
            axum::serve(source_listener, source_app)
                .await
                .expect("serve source");
        });

        let options = BasellmCatalogSyncOptions::default().allow_http_for_fixture();
        let source =
            reqwest::Url::parse(&format!("http://{source_address}/partial")).expect("source URL");
        let client = build_client(&source, &options).expect("fixture client");

        assert!(matches!(
            fetch_once(&client, source.clone(), None, 1_024).await,
            FetchResult::Failure {
                category: BasellmSyncErrorCategory::Http,
                retryable: false,
                ..
            }
        ));
        assert!(matches!(
            fetch_once(
                &client,
                source.join("/overflow").expect("overflow URL"),
                None,
                4
            )
            .await,
            FetchResult::Failure {
                category: BasellmSyncErrorCategory::BodyTooLarge,
                retryable: false,
                ..
            }
        ));
        assert!(matches!(
            fetch_once(
                &client,
                source.join("/redirect").expect("redirect URL"),
                None,
                1_024
            )
            .await,
            FetchResult::Failure {
                category: BasellmSyncErrorCategory::Redirect,
                ..
            }
        ));

        source_server.abort();
        target_server.abort();
        let _ = source_server.await;
        let _ = target_server.await;
    }

    #[tokio::test]
    async fn runtime_coordinator_keeps_lkg_on_validator_free_304_and_http_failure() {
        let fixture = HttpFixture {
            calls: Arc::new(AtomicUsize::new(0)),
            conditional_headers: Arc::new(Mutex::new(Vec::new())),
            body: Arc::new(fixture("5", "10", 272_000)),
        };
        let app = Router::new()
            .route("/all.json", get(catalog_fixture_handler))
            .with_state(fixture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fixture");
        });

        let runtime_store = Arc::new(RuntimeStore::open_in_memory().expect("open runtime store"));
        let options = BasellmCatalogSyncOptions::default()
            .with_source_url(format!("http://{address}/all.json"))
            .with_max_attempts(1)
            .allow_http_for_fixture();

        let first = sync_basellm_catalog(Arc::clone(&runtime_store), options.clone()).await;
        assert_eq!(first.outcome, BasellmSyncOutcome::Updated);
        assert_eq!(first.attempt.check_generation, 1);
        let first_snapshot = first.snapshot.expect("first snapshot");
        assert_eq!(first_snapshot.etag.as_deref(), Some("W/\"fixture-v1\""));
        let first_state = load_basellm_catalog_runtime_state(runtime_store.as_ref())
            .expect("load first runtime state");
        assert_eq!(first_state.document_revision, Some(1));

        let second = sync_basellm_catalog(Arc::clone(&runtime_store), options.clone()).await;
        assert_eq!(second.outcome, BasellmSyncOutcome::NotModified);
        assert_eq!(second.attempt.check_generation, 2);
        let second_snapshot = second.snapshot.expect("second snapshot");
        assert_eq!(second_snapshot, first_snapshot);
        let second_state = load_basellm_catalog_runtime_state(runtime_store.as_ref())
            .expect("load 304 runtime state");
        assert_eq!(second_state.document_revision, Some(2));
        assert_eq!(
            second_state.lkg,
            BasellmCatalogLoad::Valid(first_snapshot.clone())
        );

        let third = sync_basellm_catalog(Arc::clone(&runtime_store), options).await;
        assert_eq!(third.outcome, BasellmSyncOutcome::Unavailable);
        assert_eq!(
            third.attempt.last_error_category,
            Some(BasellmSyncErrorCategory::Http)
        );
        assert_eq!(third.attempt.check_generation, 3);
        assert_eq!(third.snapshot.as_deref(), Some(first_snapshot.as_ref()));
        let third_state = load_basellm_catalog_runtime_state(runtime_store.as_ref())
            .expect("load failed-refresh runtime state");
        assert_eq!(third_state.document_revision, Some(3));
        assert_eq!(third_state.lkg, BasellmCatalogLoad::Valid(first_snapshot));

        {
            let seen = fixture
                .conditional_headers
                .lock()
                .expect("fixture headers lock");
            assert_eq!(seen[0], (None, None));
            assert_eq!(seen[1].0.as_deref(), Some("W/\"fixture-v1\""));
            assert_eq!(seen[1].1.as_deref(), Some("Sat, 11 Jul 2026 00:00:00 GMT"));
        }
        server.abort();
        let _ = server.await;
    }
}
