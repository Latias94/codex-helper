use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use reqwest::header::{
    ETAG, HeaderMap, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, RETRY_AFTER,
};
use serde::de::{Error as DeError, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::fs;

use crate::basellm_metadata::{BasellmMetadataCache, BasellmOpenAiModelMetadata};
use crate::config::proxy_home_dir;
use crate::file_replace::{
    AtomicWriteError, write_bytes_file_async, write_bytes_file_validated_async,
};
use crate::pricing::basellm_all_json_url;
use crate::pricing::{ModelPrice, ModelPriceTier, canonical_provider};

pub const BASELLM_CATALOG_SCHEMA_VERSION: u32 = 1;
pub const BASELLM_CATALOG_MANIFEST_GENERATION: u64 = 1;
const DEFAULT_RESPONSE_LIMIT: usize = 16 * 1024 * 1024;
const DEFAULT_LKG_LIMIT: u64 = 8 * 1024 * 1024;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const COMMIT_LOCK_TIMEOUT: Duration = Duration::from_secs(3);
const STALE_COMMIT_LOCK_AGE: Duration = Duration::from_secs(10 * 60);
const MAX_WARNINGS: usize = 128;
const MAX_WARNING_BYTES: usize = 240;
const MAX_METADATA_DISPLAY_NAME_BYTES: usize = 512;
const MAX_METADATA_DESCRIPTION_BYTES: usize = 8 * 1024;
const MAX_PRICE_PER_MILLION_USD: i128 = 1_000_000;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

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

    pub fn metadata_cache(&self, provider: &str) -> BasellmMetadataCache {
        let models = self
            .catalog
            .providers
            .get(&normalize_id(provider))
            .map(|provider| {
                provider
                    .models
                    .iter()
                    .map(|(key, model)| (key.clone(), model.metadata.clone()))
                    .collect()
            })
            .unwrap_or_default();
        BasellmMetadataCache {
            source_url: self.source_url.clone(),
            fetched_at_unix: self.fetched_at_unix,
            etag: self.etag.clone(),
            last_modified: self.last_modified.clone(),
            content_hash: Some(self.content_hash.clone()),
            content_generation: Some(self.content_generation),
            openai_models: models,
        }
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
    Lock,
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

#[derive(Debug, Clone)]
pub struct BasellmCatalogSyncOptions {
    source_url: String,
    lkg_path: PathBuf,
    attempt_path: PathBuf,
    force: bool,
    require_https: bool,
    response_limit: usize,
    lkg_limit: u64,
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
            lkg_path: basellm_catalog_lkg_path(),
            attempt_path: basellm_catalog_attempt_path(),
            force: false,
            require_https: true,
            response_limit: DEFAULT_RESPONSE_LIMIT,
            lkg_limit: DEFAULT_LKG_LIMIT,
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

    pub fn with_paths(mut self, lkg_path: PathBuf, attempt_path: PathBuf) -> Self {
        self.lkg_path = lkg_path;
        self.attempt_path = attempt_path;
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

pub fn basellm_catalog_lkg_path() -> PathBuf {
    proxy_home_dir()
        .join("state")
        .join("basellm-catalog-lkg-v1.json")
}

pub fn basellm_catalog_attempt_path() -> PathBuf {
    proxy_home_dir()
        .join("state")
        .join("basellm-catalog-attempt-v1.json")
}

static CATALOG_SNAPSHOT: OnceLock<RwLock<Option<Arc<BasellmCatalogLkg>>>> = OnceLock::new();

fn catalog_snapshot_slot() -> &'static RwLock<Option<Arc<BasellmCatalogLkg>>> {
    CATALOG_SNAPSHOT.get_or_init(|| {
        let initial = match load_basellm_catalog_lkg_from_path_blocking(&basellm_catalog_lkg_path())
        {
            BasellmCatalogLoad::Valid(snapshot) => Some(snapshot),
            _ => None,
        };
        RwLock::new(initial)
    })
}

pub fn basellm_catalog_snapshot() -> Option<Arc<BasellmCatalogLkg>> {
    catalog_snapshot_slot()
        .read()
        .ok()
        .and_then(|snapshot| snapshot.clone())
}

fn install_catalog_snapshot(snapshot: Arc<BasellmCatalogLkg>) {
    if let Ok(mut current) = catalog_snapshot_slot().write() {
        *current = Some(snapshot);
    }
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
            let metadata =
                crate::basellm_metadata::parse_basellm_model_metadata(model_key, model_value);
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
                    price,
                },
            );
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

pub fn load_basellm_catalog_lkg_from_path_blocking(path: &Path) -> BasellmCatalogLoad {
    let Ok(metadata) = std::fs::metadata(path) else {
        return BasellmCatalogLoad::Missing;
    };
    if metadata.len() > DEFAULT_LKG_LIMIT {
        return oversized_lkg_load(path);
    }
    let Ok(bytes) = std::fs::read(path) else {
        return BasellmCatalogLoad::Corrupt;
    };
    decode_lkg_bytes(&bytes)
}

pub fn load_basellm_catalog_attempt_state() -> Option<BasellmCatalogAttemptState> {
    load_attempt_from_path(&basellm_catalog_attempt_path())
}

fn load_attempt_from_path(path: &Path) -> Option<BasellmCatalogAttemptState> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > 64 * 1024 {
        return None;
    }
    let attempt: BasellmCatalogAttemptState =
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if attempt.schema_version == BASELLM_CATALOG_SCHEMA_VERSION
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
    {
        Some(attempt)
    } else {
        None
    }
}

async fn load_lkg(path: &Path, limit: u64) -> BasellmCatalogLoad {
    let Ok(metadata) = fs::metadata(path).await else {
        return BasellmCatalogLoad::Missing;
    };
    if metadata.len() > limit {
        let path = path.to_path_buf();
        return tokio::task::spawn_blocking(move || oversized_lkg_load(&path))
            .await
            .unwrap_or(BasellmCatalogLoad::Corrupt);
    }
    let Ok(bytes) = fs::read(path).await else {
        return BasellmCatalogLoad::Corrupt;
    };
    decode_lkg_bytes(&bytes)
}

fn oversized_lkg_load(path: &Path) -> BasellmCatalogLoad {
    match probe_lkg_schema_version(path) {
        Some(version) if version > BASELLM_CATALOG_SCHEMA_VERSION => {
            BasellmCatalogLoad::UnsupportedSchema(version)
        }
        _ => BasellmCatalogLoad::Corrupt,
    }
}

fn probe_lkg_schema_version(path: &Path) -> Option<u32> {
    struct SchemaVersionVisitor<'a> {
        version: &'a std::cell::Cell<Option<u32>>,
    }

    impl<'de> Visitor<'de> for SchemaVersionVisitor<'_> {
        type Value = ();

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a BaseLLM LKG JSON object")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            while let Some(key) = map.next_key::<String>()? {
                if key == "schema_version" {
                    let raw = map.next_value::<u64>()?;
                    let version = u32::try_from(raw)
                        .map_err(|_| A::Error::custom("schema_version exceeds u32"))?;
                    self.version.set(Some(version));
                    return Err(A::Error::custom("schema probe complete"));
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(())
        }
    }

    let file = std::fs::File::open(path).ok()?;
    let version = std::cell::Cell::new(None);
    let mut deserializer = serde_json::Deserializer::from_reader(file);
    let _ = deserializer.deserialize_map(SchemaVersionVisitor { version: &version });
    version.get()
}

fn decode_lkg_bytes(bytes: &[u8]) -> BasellmCatalogLoad {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return BasellmCatalogLoad::Corrupt;
    };
    let Some(version) = value.get("schema_version").and_then(Value::as_u64) else {
        return BasellmCatalogLoad::Corrupt;
    };
    let Ok(version) = u32::try_from(version) else {
        return BasellmCatalogLoad::Corrupt;
    };
    if version > BASELLM_CATALOG_SCHEMA_VERSION {
        return BasellmCatalogLoad::UnsupportedSchema(version);
    }
    let Ok(lkg) = serde_json::from_value::<BasellmCatalogLkg>(value) else {
        return BasellmCatalogLoad::Corrupt;
    };
    if validate_lkg(&lkg) {
        BasellmCatalogLoad::Valid(Arc::new(lkg))
    } else {
        BasellmCatalogLoad::Corrupt
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

pub async fn sync_basellm_catalog(options: BasellmCatalogSyncOptions) -> BasellmCatalogSyncReport {
    let observed_load = load_lkg(&options.lkg_path, options.lkg_limit).await;
    if let BasellmCatalogLoad::UnsupportedSchema(version) = observed_load {
        return report_with_state(
            &options,
            None,
            BasellmSyncOutcome::ReadOnly,
            Some(BasellmSyncErrorCategory::UnsupportedSchema),
            Some(version),
            None,
        )
        .await;
    }
    let observed = match &observed_load {
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
            return failure_report(&options, observed, BasellmSyncErrorCategory::Schema, None)
                .await;
        }
    };
    let client = match build_client(&source, &options) {
        Ok(client) => client,
        Err(category) => return failure_report(&options, observed, category, None).await,
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
                continue;
            }
            FetchResult::NotModified if observed.is_none() => {
                return failure_report(
                    &options,
                    observed,
                    BasellmSyncErrorCategory::Semantic,
                    None,
                )
                .await;
            }
            FetchResult::NotModified if unconditional => {
                return failure_report(
                    &options,
                    observed,
                    BasellmSyncErrorCategory::Semantic,
                    None,
                )
                .await;
            }
            FetchResult::NotModified => match observed.clone() {
                Some(observed) => return commit_not_modified(&options, observed).await,
                None => {
                    return failure_report(
                        &options,
                        None,
                        BasellmSyncErrorCategory::Semantic,
                        None,
                    )
                    .await;
                }
            },
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
                        return failure_report(&options, observed, category, None).await;
                    }
                };
                let candidate_hash = match content_hash(&catalog) {
                    Ok(hash) => hash,
                    Err(_) => {
                        return failure_report(
                            &options,
                            observed,
                            BasellmSyncErrorCategory::Schema,
                            None,
                        )
                        .await;
                    }
                };
                if suspicious_count_collapse(observed.as_deref(), &catalog) {
                    return quarantine_report(
                        &options,
                        observed,
                        BasellmSyncErrorCategory::Sanity,
                        candidate_hash,
                    )
                    .await;
                }
                if economic_change_requires_quarantine(
                    observed.as_deref(),
                    &catalog,
                    options.approved_economic_change_hash.as_deref(),
                ) {
                    return quarantine_report(
                        &options,
                        observed,
                        BasellmSyncErrorCategory::EconomicAnomaly,
                        candidate_hash,
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
                        return failure_report(&options, observed, category, None).await;
                    }
                };
                return commit_candidate(&options, observed, candidate).await;
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
            } => return failure_report(&options, observed, category, retry_after).await,
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

async fn commit_candidate(
    options: &BasellmCatalogSyncOptions,
    observed: Option<Arc<BasellmCatalogLkg>>,
    candidate: Arc<BasellmCatalogLkg>,
) -> BasellmCatalogSyncReport {
    let _lock = match CatalogCommitLock::acquire(&options.lkg_path).await {
        Ok(lock) => lock,
        Err(()) => {
            return failure_report(options, observed, BasellmSyncErrorCategory::Lock, None).await;
        }
    };
    let current = load_lkg(&options.lkg_path, options.lkg_limit).await;
    if !same_observation(&observed, &current) {
        drop(_lock);
        return stale_report(options, current).await;
    }
    let previous_bytes = if observed.is_some() {
        match read_lkg_bytes_bounded(&options.lkg_path, options.lkg_limit).await {
            Ok(bytes) if same_observation(&observed, &decode_lkg_bytes(bytes.as_slice())) => {
                Some(bytes)
            }
            _ => {
                drop(_lock);
                return stale_report(
                    options,
                    load_lkg(&options.lkg_path, options.lkg_limit).await,
                )
                .await;
            }
        }
    } else {
        None
    };
    let bytes = match serde_json::to_vec_pretty(candidate.as_ref()) {
        Ok(bytes) if bytes.len() as u64 <= options.lkg_limit => bytes,
        _ => {
            drop(_lock);
            return failure_report(
                options,
                observed,
                BasellmSyncErrorCategory::Persistence,
                None,
            )
            .await;
        }
    };
    let write =
        write_bytes_file_validated_async(
            &options.lkg_path,
            &bytes,
            |bytes| match decode_lkg_bytes(bytes) {
                BasellmCatalogLoad::Valid(_) => Ok(()),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid BaseLLM LKG staging bytes",
                )),
            },
        )
        .await;
    let adopted = match write {
        Ok(()) => candidate,
        Err(AtomicWriteError::CommitStateUnknown { .. }) => {
            match read_lkg_bytes_bounded(&options.lkg_path, options.lkg_limit).await {
                Ok(recovered_bytes) => match (
                    classify_recovered_commit_bytes(
                        previous_bytes.as_deref(),
                        bytes.as_slice(),
                        recovered_bytes.as_slice(),
                    ),
                    decode_lkg_bytes(recovered_bytes.as_slice()),
                ) {
                    (RecoveredCommit::Candidate, BasellmCatalogLoad::Valid(recovered)) => recovered,
                    (RecoveredCommit::Previous, BasellmCatalogLoad::Valid(recovered)) => {
                        install_catalog_snapshot(recovered.clone());
                        let attempt = attempt_state(
                            options,
                            BasellmSyncOutcome::Unavailable,
                            Some(BasellmSyncErrorCategory::Persistence),
                            Some(recovered.as_ref()),
                            None,
                            None,
                        );
                        persist_attempt_best_effort(&options.attempt_path, &attempt).await;
                        return BasellmCatalogSyncReport {
                            outcome: BasellmSyncOutcome::Unavailable,
                            snapshot: Some(recovered),
                            attempt,
                        };
                    }
                    (RecoveredCommit::Unexpected, _)
                    | (_, BasellmCatalogLoad::Missing)
                    | (_, BasellmCatalogLoad::Corrupt)
                    | (_, BasellmCatalogLoad::UnsupportedSchema(_)) => {
                        drop(_lock);
                        return failure_report(
                            options,
                            observed,
                            BasellmSyncErrorCategory::Persistence,
                            None,
                        )
                        .await;
                    }
                },
                Err(()) => {
                    drop(_lock);
                    return failure_report(
                        options,
                        observed,
                        BasellmSyncErrorCategory::Persistence,
                        None,
                    )
                    .await;
                }
            }
        }
        Err(_) => {
            drop(_lock);
            return failure_report(
                options,
                observed,
                BasellmSyncErrorCategory::Persistence,
                None,
            )
            .await;
        }
    };
    install_catalog_snapshot(adopted.clone());
    let attempt = attempt_state(
        options,
        BasellmSyncOutcome::Updated,
        None,
        Some(adopted.as_ref()),
        None,
        None,
    );
    persist_attempt_best_effort(&options.attempt_path, &attempt).await;
    BasellmCatalogSyncReport {
        outcome: BasellmSyncOutcome::Updated,
        snapshot: Some(adopted),
        attempt,
    }
}

async fn read_lkg_bytes_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, ()> {
    let metadata = fs::metadata(path).await.map_err(|_| ())?;
    if metadata.len() > limit {
        return Err(());
    }
    let bytes = fs::read(path).await.map_err(|_| ())?;
    (bytes.len() as u64 <= limit).then_some(bytes).ok_or(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveredCommit {
    Candidate,
    Previous,
    Unexpected,
}

fn classify_recovered_commit_bytes(
    previous: Option<&[u8]>,
    candidate: &[u8],
    recovered: &[u8],
) -> RecoveredCommit {
    if recovered == candidate {
        RecoveredCommit::Candidate
    } else if previous.is_some_and(|previous| recovered == previous) {
        RecoveredCommit::Previous
    } else {
        RecoveredCommit::Unexpected
    }
}

#[cfg(test)]
fn classify_recovered_commit(
    observed: Option<&BasellmCatalogLkg>,
    candidate: &BasellmCatalogLkg,
    recovered: &BasellmCatalogLkg,
) -> RecoveredCommit {
    if recovered == candidate {
        RecoveredCommit::Candidate
    } else if observed.is_some_and(|observed| recovered == observed) {
        RecoveredCommit::Previous
    } else {
        RecoveredCommit::Unexpected
    }
}

async fn commit_not_modified(
    options: &BasellmCatalogSyncOptions,
    observed: Arc<BasellmCatalogLkg>,
) -> BasellmCatalogSyncReport {
    let _lock = match CatalogCommitLock::acquire(&options.lkg_path).await {
        Ok(lock) => lock,
        Err(()) => {
            return failure_report(
                options,
                Some(observed),
                BasellmSyncErrorCategory::Lock,
                None,
            )
            .await;
        }
    };
    let current = load_lkg(&options.lkg_path, options.lkg_limit).await;
    if !same_observation(&Some(observed.clone()), &current) {
        drop(_lock);
        return stale_report(options, current).await;
    }
    install_catalog_snapshot(observed.clone());
    let attempt = attempt_state(
        options,
        BasellmSyncOutcome::NotModified,
        None,
        Some(observed.as_ref()),
        None,
        None,
    );
    persist_attempt_best_effort(&options.attempt_path, &attempt).await;
    BasellmCatalogSyncReport {
        outcome: BasellmSyncOutcome::NotModified,
        snapshot: Some(observed),
        attempt,
    }
}

fn same_observation(
    observed: &Option<Arc<BasellmCatalogLkg>>,
    current: &BasellmCatalogLoad,
) -> bool {
    match (observed, current) {
        (None, BasellmCatalogLoad::Missing | BasellmCatalogLoad::Corrupt) => true,
        (Some(observed), BasellmCatalogLoad::Valid(current)) => {
            observed.body_generation == current.body_generation
                && observed.content_hash == current.content_hash
                && observed.etag == current.etag
                && observed.last_modified == current.last_modified
        }
        _ => false,
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

async fn quarantine_report(
    options: &BasellmCatalogSyncOptions,
    snapshot: Option<Arc<BasellmCatalogLkg>>,
    category: BasellmSyncErrorCategory,
    candidate_hash: String,
) -> BasellmCatalogSyncReport {
    let mut report = report_with_state(
        options,
        snapshot,
        BasellmSyncOutcome::Quarantined,
        Some(category),
        None,
        None,
    )
    .await;
    if report.outcome == BasellmSyncOutcome::Quarantined {
        report.attempt.quarantined_candidate_hash = Some(candidate_hash);
        persist_attempt_best_effort(&options.attempt_path, &report.attempt).await;
    }
    report
}

async fn failure_report(
    options: &BasellmCatalogSyncOptions,
    snapshot: Option<Arc<BasellmCatalogLkg>>,
    category: BasellmSyncErrorCategory,
    retry_after: Option<Duration>,
) -> BasellmCatalogSyncReport {
    report_with_state(
        options,
        snapshot,
        BasellmSyncOutcome::Unavailable,
        Some(category),
        None,
        retry_after,
    )
    .await
}

async fn stale_report(
    options: &BasellmCatalogSyncOptions,
    current: BasellmCatalogLoad,
) -> BasellmCatalogSyncReport {
    let snapshot = match current {
        BasellmCatalogLoad::Valid(snapshot) => {
            install_catalog_snapshot(snapshot.clone());
            Some(snapshot)
        }
        _ => basellm_catalog_snapshot(),
    };
    report_with_state(
        options,
        snapshot,
        BasellmSyncOutcome::StaleResponse,
        Some(BasellmSyncErrorCategory::StaleResponse),
        None,
        None,
    )
    .await
}

async fn report_with_state(
    options: &BasellmCatalogSyncOptions,
    snapshot: Option<Arc<BasellmCatalogLkg>>,
    mut outcome: BasellmSyncOutcome,
    mut category: Option<BasellmSyncErrorCategory>,
    read_only_schema: Option<u32>,
    retry_after: Option<Duration>,
) -> BasellmCatalogSyncReport {
    let mut snapshot = snapshot;
    let Ok(_lock) = CatalogCommitLock::acquire(&options.lkg_path).await else {
        let attempt = attempt_state(
            options,
            BasellmSyncOutcome::Unavailable,
            Some(BasellmSyncErrorCategory::Lock),
            snapshot.as_deref(),
            read_only_schema,
            retry_after,
        );
        return BasellmCatalogSyncReport {
            outcome: BasellmSyncOutcome::Unavailable,
            snapshot,
            attempt,
        };
    };
    let current = load_lkg(&options.lkg_path, options.lkg_limit).await;
    let same_content = same_observation(&snapshot, &current);
    match current {
        BasellmCatalogLoad::Valid(current) if !same_content => {
            snapshot = Some(current);
            outcome = BasellmSyncOutcome::StaleResponse;
            category = Some(BasellmSyncErrorCategory::StaleResponse);
        }
        BasellmCatalogLoad::UnsupportedSchema(version) => {
            snapshot = None;
            outcome = BasellmSyncOutcome::ReadOnly;
            category = Some(BasellmSyncErrorCategory::UnsupportedSchema);
            let attempt =
                attempt_state(options, outcome, category, None, Some(version), retry_after);
            persist_attempt_best_effort(&options.attempt_path, &attempt).await;
            return BasellmCatalogSyncReport {
                outcome,
                snapshot,
                attempt,
            };
        }
        _ if !same_content => {
            outcome = BasellmSyncOutcome::StaleResponse;
            category = Some(BasellmSyncErrorCategory::StaleResponse);
        }
        _ => {}
    }
    if let Some(snapshot) = &snapshot {
        install_catalog_snapshot(snapshot.clone());
    }
    let attempt = attempt_state(
        options,
        outcome,
        category,
        snapshot.as_deref(),
        read_only_schema,
        retry_after,
    );
    persist_attempt_best_effort(&options.attempt_path, &attempt).await;
    BasellmCatalogSyncReport {
        outcome,
        snapshot,
        attempt,
    }
}

fn attempt_state(
    options: &BasellmCatalogSyncOptions,
    outcome: BasellmSyncOutcome,
    category: Option<BasellmSyncErrorCategory>,
    snapshot: Option<&BasellmCatalogLkg>,
    read_only_schema_version: Option<u32>,
    retry_after: Option<Duration>,
) -> BasellmCatalogAttemptState {
    let now = unix_now();
    BasellmCatalogAttemptState {
        schema_version: BASELLM_CATALOG_SCHEMA_VERSION,
        check_generation: load_attempt_from_path(&options.attempt_path)
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

async fn persist_attempt_best_effort(path: &Path, attempt: &BasellmCatalogAttemptState) {
    if let Ok(bytes) = serde_json::to_vec_pretty(attempt) {
        let _ = write_bytes_file_async(path, &bytes).await;
    }
}

struct CatalogCommitLock {
    path: PathBuf,
    token: String,
}

impl CatalogCommitLock {
    async fn acquire(lkg_path: &Path) -> Result<Self, ()> {
        let lock_path = lkg_path.with_extension("lock");
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).await.map_err(|_| ())?;
        }
        let token = format!("{}:{}", std::process::id(), uuid::Uuid::new_v4());
        let deadline = tokio::time::Instant::now() + COMMIT_LOCK_TIMEOUT;
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
                .await
            {
                Ok(file) => {
                    drop(file);
                    if fs::write(&lock_path, token.as_bytes()).await.is_err() {
                        let _ = fs::remove_file(&lock_path).await;
                        return Err(());
                    }
                    if let Ok(file) = fs::File::open(&lock_path).await {
                        let _ = file.sync_all().await;
                    }
                    return Ok(Self {
                        path: lock_path,
                        token,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let stale = fs::symlink_metadata(&lock_path)
                        .await
                        .ok()
                        .filter(|metadata| {
                            metadata.file_type().is_file() && !metadata.file_type().is_symlink()
                        })
                        .and_then(|metadata| metadata.modified().ok())
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                        .is_some_and(|age| age >= STALE_COMMIT_LOCK_AGE);
                    if stale {
                        let _ = fs::remove_file(&lock_path).await;
                        continue;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Err(());
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(_) => return Err(()),
            }
        }
    }
}

impl Drop for CatalogCommitLock {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).is_ok_and(|token| token == self.token) {
            let _ = std::fs::remove_file(&self.path);
        }
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
                    "limit": {{ "input": 100000, "context": 128000 }},
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

        let root =
            std::env::temp_dir().join(format!("basellm-retention-{case}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).await.expect("create test root");
        let lkg_path = root.join("lkg.json");
        let attempt_path = root.join("attempt.json");
        let lkg = lkg_from_fixture(&fixture("5", "10", 272_000), 7);
        let lkg_bytes = serde_json::to_vec_pretty(&lkg).expect("encode valid LKG");
        fs::write(&lkg_path, &lkg_bytes)
            .await
            .expect("seed valid LKG");
        install_catalog_snapshot(Arc::new(lkg.clone()));
        let effective_before = crate::pricing::refresh_effective_pricing_catalog();

        let report = sync_basellm_catalog(
            BasellmCatalogSyncOptions::default()
                .with_source_url(format!("http://{address}/all.json"))
                .with_paths(lkg_path.clone(), attempt_path.clone())
                .with_max_attempts(1)
                .allow_http_for_fixture(),
        )
        .await;

        assert_eq!(report.outcome, BasellmSyncOutcome::Unavailable, "{case}");
        assert_eq!(
            report.attempt.last_error_category,
            Some(expected_category),
            "{case}"
        );
        assert_eq!(report.snapshot.as_deref(), Some(&lkg), "{case}");
        assert_eq!(
            fs::read(&lkg_path).await.expect("retained LKG bytes"),
            lkg_bytes,
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
        let _ = fs::remove_file(&attempt_path).await;
        fs::remove_file(&lkg_path).await.expect("remove test LKG");
        fs::remove_dir(&root).await.expect("remove test root");
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
            let bytes = serde_json::to_vec(&candidate).expect("encode invalid LKG");
            assert_eq!(decode_lkg_bytes(&bytes), BasellmCatalogLoad::Corrupt);
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

        let root = std::env::temp_dir().join(format!("basellm-retry-{}", uuid::Uuid::new_v4()));
        let options = BasellmCatalogSyncOptions::default()
            .with_paths(root.join("lkg.json"), root.join("attempt.json"));
        let attempt = attempt_state(
            &options,
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

    #[test]
    fn future_schema_is_read_only_load_state() {
        let bytes = br#"{"schema_version":999,"catalog":{}}"#;
        assert_eq!(
            decode_lkg_bytes(bytes),
            BasellmCatalogLoad::UnsupportedSchema(999)
        );
    }

    #[tokio::test]
    async fn oversized_future_schema_refresh_is_byte_identical() {
        let root = std::env::temp_dir().join(format!("basellm-future-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).await.expect("create test root");
        let lkg_path = root.join("lkg.json");
        let attempt_path = root.join("attempt.json");
        let mut bytes = br#"{"schema_version":999,"padding":""#.to_vec();
        bytes.resize(DEFAULT_LKG_LIMIT as usize + 1, b'x');
        bytes.extend_from_slice(br#""}"#);
        fs::write(&lkg_path, &bytes)
            .await
            .expect("write future LKG");

        let report = sync_basellm_catalog(
            BasellmCatalogSyncOptions::default()
                .with_source_url("invalid source URL")
                .with_paths(lkg_path.clone(), attempt_path.clone()),
        )
        .await;

        assert_eq!(report.outcome, BasellmSyncOutcome::ReadOnly);
        assert_eq!(report.attempt.read_only_schema_version, Some(999));
        assert_eq!(fs::read(&lkg_path).await.expect("future LKG bytes"), bytes);

        let _ = fs::remove_file(&attempt_path).await;
        fs::remove_file(&lkg_path).await.expect("remove future LKG");
        fs::remove_dir(&root).await.expect("remove test root");
    }

    #[test]
    fn uncertain_replace_only_reports_updated_for_the_candidate() {
        let previous = lkg_from_fixture(&fixture("5", "10", 272_000), 7);
        let candidate = lkg_from_fixture(&fixture("6", "10", 272_000), 8);
        let unrelated = lkg_from_fixture(&fixture("7", "10", 272_000), 9);

        let mut candidate_metadata_drift = candidate.clone();
        candidate_metadata_drift.etag = Some("\"unrelated-validator\"".to_string());
        let mut previous_metadata_drift = previous.clone();
        previous_metadata_drift.validated_at_unix =
            previous_metadata_drift.validated_at_unix.saturating_add(1);

        assert_eq!(
            classify_recovered_commit(Some(&previous), &candidate, &candidate),
            RecoveredCommit::Candidate
        );
        assert_eq!(
            classify_recovered_commit(Some(&previous), &candidate, &previous),
            RecoveredCommit::Previous
        );
        assert_eq!(
            classify_recovered_commit(Some(&previous), &candidate, &unrelated),
            RecoveredCommit::Unexpected
        );
        assert_eq!(
            classify_recovered_commit(Some(&previous), &candidate, &candidate_metadata_drift),
            RecoveredCommit::Unexpected
        );
        assert_eq!(
            classify_recovered_commit(Some(&previous), &candidate, &previous_metadata_drift),
            RecoveredCommit::Unexpected
        );
    }

    #[test]
    fn uncertain_replace_requires_exact_previous_or_candidate_bytes() {
        let previous = lkg_from_fixture(&fixture("5", "10", 272_000), 7);
        let candidate = lkg_from_fixture(&fixture("6", "10", 272_000), 8);
        let previous_bytes = serde_json::to_vec_pretty(&previous).expect("previous bytes");
        let candidate_bytes = serde_json::to_vec_pretty(&candidate).expect("candidate bytes");
        let semantically_equal_candidate =
            serde_json::to_vec(&candidate).expect("compact candidate bytes");

        assert_eq!(
            classify_recovered_commit_bytes(
                Some(previous_bytes.as_slice()),
                &candidate_bytes,
                &candidate_bytes
            ),
            RecoveredCommit::Candidate
        );
        assert_eq!(
            classify_recovered_commit_bytes(
                Some(previous_bytes.as_slice()),
                &candidate_bytes,
                &previous_bytes
            ),
            RecoveredCommit::Previous
        );
        assert_eq!(
            serde_json::from_slice::<BasellmCatalogLkg>(&semantically_equal_candidate)
                .expect("decode compact candidate"),
            candidate
        );
        assert_eq!(
            classify_recovered_commit_bytes(
                Some(previous_bytes.as_slice()),
                &candidate_bytes,
                &semantically_equal_candidate
            ),
            RecoveredCommit::Unexpected
        );
    }

    #[tokio::test]
    async fn atomic_store_round_trip_validates_complete_lkg() {
        let root = std::env::temp_dir().join(format!("basellm-lkg-{}", uuid::Uuid::new_v4()));
        let path = root.join("lkg.json");
        let lkg = lkg_from_fixture(&fixture("5", "10", 272_000), 1);
        let bytes = serde_json::to_vec_pretty(&lkg).unwrap();
        write_bytes_file_validated_async(&path, &bytes, |bytes| match decode_lkg_bytes(bytes) {
            BasellmCatalogLoad::Valid(_) => Ok(()),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "invalid LKG")),
        })
        .await
        .expect("write LKG");
        assert!(matches!(
            load_lkg(&path, DEFAULT_LKG_LIMIT).await,
            BasellmCatalogLoad::Valid(_)
        ));
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
    async fn coordinator_keeps_lkg_on_validator_free_304_and_http_failure() {
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

        let root = std::env::temp_dir().join(format!("basellm-sync-{}", uuid::Uuid::new_v4()));
        let lkg_path = root.join("lkg.json");
        let attempt_path = root.join("attempt.json");
        let options = BasellmCatalogSyncOptions::default()
            .with_source_url(format!("http://{address}/all.json"))
            .with_paths(lkg_path.clone(), attempt_path)
            .with_max_attempts(1)
            .allow_http_for_fixture();

        let first = sync_basellm_catalog(options.clone()).await;
        assert_eq!(first.outcome, BasellmSyncOutcome::Updated);
        assert_eq!(first.attempt.check_generation, 1);
        let first_bytes = fs::read(&lkg_path).await.expect("first LKG bytes");
        let first_snapshot = first.snapshot.expect("first snapshot");
        assert_eq!(first_snapshot.etag.as_deref(), Some("W/\"fixture-v1\""));

        let second = sync_basellm_catalog(options.clone()).await;
        assert_eq!(second.outcome, BasellmSyncOutcome::NotModified);
        assert_eq!(second.attempt.check_generation, 2);
        assert_eq!(
            fs::read(&lkg_path).await.expect("304 LKG bytes"),
            first_bytes
        );
        let second_snapshot = second.snapshot.expect("second snapshot");
        assert_eq!(second_snapshot.etag, first_snapshot.etag);
        assert_eq!(second_snapshot.last_modified, first_snapshot.last_modified);

        let third = sync_basellm_catalog(options).await;
        assert_eq!(third.outcome, BasellmSyncOutcome::Unavailable);
        assert_eq!(
            third.attempt.last_error_category,
            Some(BasellmSyncErrorCategory::Http)
        );
        assert_eq!(third.attempt.check_generation, 3);
        assert_eq!(
            fs::read(&lkg_path).await.expect("failed-fetch LKG bytes"),
            first_bytes
        );

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
