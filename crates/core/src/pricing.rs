use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::basellm_catalog::{BasellmCatalogLkg, basellm_catalog_snapshot};
use crate::file_replace::write_text_file;
use crate::usage::{CacheInputAccounting, UsageMetrics};

const BASELLM_ALL_JSON_URL: &str = "https://basellm.github.io/llm-metadata/api/all.json";
const FEMTO_USD_PER_USD: i128 = 1_000_000_000_000_000;
const TOKENS_PER_MILLION: i128 = 1_000_000;
const MULTIPLIER_SCALE: i128 = 1_000_000;
const DEFAULT_CANONICAL_PROVIDER: &str = "openai";
const MODEL_PRICE_OVERRIDES_SCHEMA_VERSION: u32 = 2;
const EFFECTIVE_PRICING_REVISION_SCHEMA: &str = "codex-helper/effective-pricing/v1";
const CANONICAL_PROVIDER_MAPPING_REVISION: &str =
    "codex=openai;openai=openai;claude=anthropic;anthropic=anthropic;gemini=google;google=google";
const MAX_MODEL_PRICE_OVERRIDES_BYTES: u64 = 1024 * 1024;
const MODEL_PRICE_OVERRIDES_DOC_HEADER: &str = r#"# codex-helper pricing_overrides.toml
#
# Managed by `codex-helper pricing`.
# Use this file for provider-specific model aliases, custom relay prices, or local corrections.
"#;

fn u64_is_zero(value: &u64) -> bool {
    *value == 0
}

fn u32_is_zero(value: &u32) -> bool {
    *value == 0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CostConfidence {
    #[default]
    Unknown,
    Partial,
    Estimated,
    Exact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct UsdAmount {
    femto_usd: i128,
}

impl UsdAmount {
    pub const ZERO: Self = Self { femto_usd: 0 };

    pub fn from_femto_usd(femto_usd: i128) -> Self {
        Self {
            femto_usd: femto_usd.max(0),
        }
    }

    pub fn from_decimal_str(value: &str) -> Option<Self> {
        parse_decimal_usd_to_femto(value).map(Self::from_femto_usd)
    }

    pub fn femto_usd(self) -> i128 {
        self.femto_usd
    }

    pub fn is_zero(self) -> bool {
        self.femto_usd == 0
    }

    pub fn checked_div_u64(self, divisor: u64) -> Option<Self> {
        (divisor > 0).then(|| Self::from_femto_usd(self.femto_usd / divisor as i128))
    }

    pub fn saturating_add(self, other: Self) -> Self {
        Self::from_femto_usd(self.femto_usd.saturating_add(other.femto_usd))
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self::from_femto_usd(self.femto_usd.saturating_sub(other.femto_usd))
    }

    pub fn cost_for_tokens_per_million(tokens: i64, price_per_million: Self) -> Self {
        let tokens = tokens.max(0) as i128;
        Self::from_femto_usd(
            tokens
                .saturating_mul(price_per_million.femto_usd)
                .saturating_div(TOKENS_PER_MILLION),
        )
    }

    pub fn format_usd(self) -> String {
        format_femto_usd(self.femto_usd)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceMultiplier {
    scaled: i128,
}

impl Default for PriceMultiplier {
    fn default() -> Self {
        Self::one()
    }
}

impl PriceMultiplier {
    pub const fn one() -> Self {
        Self {
            scaled: MULTIPLIER_SCALE,
        }
    }

    pub fn from_decimal_str(value: &str) -> Option<Self> {
        let amount = parse_decimal_usd_to_femto(value)?;
        let scaled = amount
            .saturating_mul(MULTIPLIER_SCALE)
            .saturating_div(FEMTO_USD_PER_USD);
        (scaled > 0).then_some(Self { scaled })
    }

    pub fn apply(self, amount: UsdAmount) -> UsdAmount {
        apply_scaled_ratio(amount, self.scaled, MULTIPLIER_SCALE)
    }

    pub fn format(self) -> String {
        let whole = self.scaled / MULTIPLIER_SCALE;
        let frac = self.scaled % MULTIPLIER_SCALE;
        if frac == 0 {
            return whole.to_string();
        }
        let mut frac_s = format!("{frac:06}");
        while frac_s.ends_with('0') {
            frac_s.pop();
        }
        format!("{whole}.{frac_s}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CostAdjustments {
    pub service_tier_multiplier: Option<PriceMultiplier>,
    pub provider_multiplier: Option<PriceMultiplier>,
}

impl CostAdjustments {
    fn apply(self, amount: UsdAmount) -> UsdAmount {
        let (scaled, denominator) = match (self.service_tier_multiplier, self.provider_multiplier) {
            (None, None) => return amount,
            (Some(multiplier), None) | (None, Some(multiplier)) => {
                (multiplier.scaled, MULTIPLIER_SCALE)
            }
            (Some(service), Some(provider)) => (
                service.scaled.saturating_mul(provider.scaled),
                MULTIPLIER_SCALE.saturating_mul(MULTIPLIER_SCALE),
            ),
        };
        apply_scaled_ratio(amount, scaled, denominator)
    }
}

fn apply_scaled_ratio(amount: UsdAmount, scaled: i128, denominator: i128) -> UsdAmount {
    let numerator = amount.femto_usd.saturating_mul(scaled);
    let q = numerator / denominator;
    let r = numerator % denominator;
    let rounded = if r.saturating_mul(2) >= denominator {
        q.saturating_add(1)
    } else {
        q
    };
    UsdAmount::from_femto_usd(rounded)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CostBreakdown {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_cost_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_cost_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier_multiplier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_cost_multiplier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<String>,
    #[serde(default)]
    pub confidence: CostConfidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_pricing_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_tier: Option<SelectedPriceTier>,
    #[serde(skip)]
    total_cost_femto_usd: Option<i128>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedPriceTier {
    pub tier_type: String,
    pub threshold_tokens: u64,
    pub matched_input_tokens: u64,
}

impl Default for CostBreakdown {
    fn default() -> Self {
        Self::unknown()
    }
}

impl CostBreakdown {
    pub fn unknown() -> Self {
        Self {
            input_cost_usd: None,
            output_cost_usd: None,
            cache_read_cost_usd: None,
            cache_creation_cost_usd: None,
            service_tier_multiplier: None,
            provider_cost_multiplier: None,
            total_cost_usd: None,
            confidence: CostConfidence::Unknown,
            pricing_source: None,
            pricing_provider: None,
            pricing_generation: None,
            effective_pricing_revision: None,
            selected_tier: None,
            total_cost_femto_usd: None,
        }
    }

    pub fn is_unknown(&self) -> bool {
        self.confidence == CostConfidence::Unknown && self.total_cost_usd.is_none()
    }

    pub fn total_cost_femto_usd(&self) -> Option<i128> {
        self.total_cost_femto_usd
    }

    pub fn display_total(&self) -> String {
        format_cost_display(self.total_cost_usd.as_deref())
    }

    pub fn display_total_with_confidence(&self) -> String {
        format_cost_with_confidence(self.total_cost_usd.as_deref(), self.confidence)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CostSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<String>,
    #[serde(default)]
    pub confidence: CostConfidence,
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub priced_requests: u64,
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub unpriced_requests: u64,
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub partial_requests: u64,
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub exact_requests: u64,
    #[serde(skip)]
    total_cost_femto_usd: i128,
}

impl Default for CostSummary {
    fn default() -> Self {
        Self {
            total_cost_usd: None,
            confidence: CostConfidence::Unknown,
            priced_requests: 0,
            unpriced_requests: 0,
            partial_requests: 0,
            exact_requests: 0,
            total_cost_femto_usd: 0,
        }
    }
}

impl CostSummary {
    pub fn is_empty(&self) -> bool {
        self.priced_requests == 0 && self.unpriced_requests == 0 && self.total_cost_usd.is_none()
    }

    pub fn add_assign(&mut self, other: &Self) {
        self.priced_requests = self.priced_requests.saturating_add(other.priced_requests);
        self.unpriced_requests = self
            .unpriced_requests
            .saturating_add(other.unpriced_requests);
        self.partial_requests = self.partial_requests.saturating_add(other.partial_requests);
        self.exact_requests = self.exact_requests.saturating_add(other.exact_requests);
        self.total_cost_femto_usd = self
            .total_cost_femto_usd
            .saturating_add(other.total_cost_femto_usd);
        self.refresh_display();
    }

    pub fn record_usage_cost(&mut self, cost: &CostBreakdown) {
        if matches!(cost.confidence, CostConfidence::Unknown) {
            self.unpriced_requests = self.unpriced_requests.saturating_add(1);
            self.refresh_display();
            return;
        }

        let total = cost.total_cost_femto_usd().or_else(|| {
            cost.total_cost_usd
                .as_deref()
                .and_then(parse_decimal_usd_to_femto)
        });

        let Some(total) = total else {
            self.unpriced_requests = self.unpriced_requests.saturating_add(1);
            self.refresh_display();
            return;
        };

        self.priced_requests = self.priced_requests.saturating_add(1);
        if cost.confidence == CostConfidence::Partial {
            self.partial_requests = self.partial_requests.saturating_add(1);
        }
        if cost.confidence == CostConfidence::Exact {
            self.exact_requests = self.exact_requests.saturating_add(1);
        }
        self.total_cost_femto_usd = self.total_cost_femto_usd.saturating_add(total.max(0));
        self.refresh_display();
    }

    pub fn display_total(&self) -> String {
        format_cost_display(self.total_cost_usd.as_deref())
    }

    pub fn display_total_with_confidence(&self) -> String {
        format_cost_with_confidence(self.total_cost_usd.as_deref(), self.confidence)
    }

    fn refresh_display(&mut self) {
        if self.priced_requests == 0 {
            self.total_cost_usd = None;
            self.confidence = CostConfidence::Unknown;
            return;
        }

        self.total_cost_usd = Some(format_femto_usd(self.total_cost_femto_usd));
        self.confidence = if self.unpriced_requests > 0 || self.partial_requests > 0 {
            CostConfidence::Partial
        } else if self.exact_requests == self.priced_requests {
            CostConfidence::Exact
        } else {
            CostConfidence::Estimated
        };
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillableTokenUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
}

impl BillableTokenUsage {
    pub fn from_usage(usage: &UsageMetrics) -> Self {
        Self::from_usage_with_accounting(usage, CacheInputAccounting::default())
    }

    pub fn from_usage_with_accounting(
        usage: &UsageMetrics,
        accounting: CacheInputAccounting,
    ) -> Self {
        let breakdown = usage.cache_usage_breakdown(accounting);

        Self {
            input_tokens: breakdown.effective_input_tokens,
            output_tokens: usage.output_tokens.max(0),
            cache_read_input_tokens: breakdown.cache_read_input_tokens,
            cache_creation_input_tokens: breakdown.cache_creation_input_tokens,
        }
    }

    pub fn context_input_tokens(&self) -> u64 {
        self.input_tokens
            .max(0)
            .saturating_add(self.cache_read_input_tokens.max(0)) as u64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPriceTier {
    pub threshold_tokens: u64,
    pub input_per_1m: Option<UsdAmount>,
    pub output_per_1m: Option<UsdAmount>,
    pub cache_read_input_per_1m: Option<UsdAmount>,
    pub cache_creation_input_per_1m: Option<UsdAmount>,
}

impl ModelPriceTier {
    pub fn from_per_million_usd(
        threshold_tokens: u64,
        input: Option<&str>,
        output: Option<&str>,
        cache_read: Option<&str>,
        cache_creation: Option<&str>,
    ) -> Option<Self> {
        Some(Self {
            threshold_tokens,
            input_per_1m: parse_optional_usd(input)?,
            output_per_1m: parse_optional_usd(output)?,
            cache_read_input_per_1m: parse_optional_usd(cache_read)?,
            cache_creation_input_per_1m: parse_optional_usd(cache_creation)?,
        })
    }

    fn has_price_overlay(&self) -> bool {
        self.input_per_1m.is_some()
            || self.output_per_1m.is_some()
            || self.cache_read_input_per_1m.is_some()
            || self.cache_creation_input_per_1m.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPrice {
    pub provider: String,
    pub model_id: String,
    pub display_name: Option<String>,
    pub aliases: Vec<String>,
    pub input_per_1m: UsdAmount,
    pub output_per_1m: UsdAmount,
    pub cache_read_input_per_1m: Option<UsdAmount>,
    pub cache_creation_input_per_1m: Option<UsdAmount>,
    pub tiers: Vec<ModelPriceTier>,
    pub source: String,
    pub source_generation: Option<String>,
    pub confidence: CostConfidence,
}

impl ModelPrice {
    pub fn from_per_million_usd(
        model_id: impl Into<String>,
        display_name: Option<String>,
        input: &str,
        output: &str,
        cache_read: Option<&str>,
        cache_creation: Option<&str>,
        source: impl Into<String>,
    ) -> Option<Self> {
        Self::from_per_million_usd_for_provider(
            DEFAULT_CANONICAL_PROVIDER,
            model_id,
            display_name,
            input,
            output,
            cache_read,
            cache_creation,
            source,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "provider-aware variant mirrors the existing component-price constructor"
    )]
    pub fn from_per_million_usd_for_provider(
        provider: impl AsRef<str>,
        model_id: impl Into<String>,
        display_name: Option<String>,
        input: &str,
        output: &str,
        cache_read: Option<&str>,
        cache_creation: Option<&str>,
        source: impl Into<String>,
    ) -> Option<Self> {
        let provider = canonical_provider(provider.as_ref())?;
        Some(Self {
            provider,
            model_id: model_id.into(),
            display_name,
            aliases: Vec::new(),
            input_per_1m: UsdAmount::from_decimal_str(input)?,
            output_per_1m: UsdAmount::from_decimal_str(output)?,
            cache_read_input_per_1m: parse_optional_usd(cache_read)?,
            cache_creation_input_per_1m: parse_optional_usd(cache_creation)?,
            tiers: Vec::new(),
            source: source.into(),
            source_generation: None,
            confidence: CostConfidence::Estimated,
        })
    }

    pub fn with_aliases(mut self, aliases: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_tiers(
        mut self,
        tiers: impl IntoIterator<Item = ModelPriceTier>,
    ) -> Result<Self, String> {
        self.tiers = tiers.into_iter().collect();
        validate_model_price_tiers(&self.tiers)?;
        self.tiers.sort_by_key(|tier| tier.threshold_tokens);
        Ok(self)
    }

    pub fn with_source_generation(mut self, generation: impl Into<String>) -> Self {
        self.source_generation = Some(generation.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LocalModelPriceOverridesDocument {
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, LocalProviderPriceOverrides>,
    // Version 1 compatibility: bare model rows belong to Codex/OpenAI.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, LocalModelPriceOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LocalProviderPriceOverrides {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, LocalModelPriceOverride>,
}

impl LocalModelPriceOverridesDocument {
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
            && self
                .providers
                .values()
                .all(|provider| provider.models.is_empty())
    }

    pub fn normalized(&self) -> Result<Self, String> {
        if self.version > MODEL_PRICE_OVERRIDES_SCHEMA_VERSION {
            return Err(format!(
                "unsupported pricing override schema version {}; maximum supported version is {}",
                self.version, MODEL_PRICE_OVERRIDES_SCHEMA_VERSION
            ));
        }

        let mut normalized = Self {
            version: MODEL_PRICE_OVERRIDES_SCHEMA_VERSION,
            providers: BTreeMap::new(),
            models: BTreeMap::new(),
        };

        for (raw_provider, provider_rows) in &self.providers {
            let provider = require_canonical_provider(raw_provider)?;
            for (model_id, row) in &provider_rows.models {
                normalized.insert_normalized_row(&provider, model_id, row.clone(), false)?;
            }
        }

        for (model_id, row) in &self.models {
            normalized.insert_normalized_row(
                DEFAULT_CANONICAL_PROVIDER,
                model_id,
                row.clone(),
                true,
            )?;
        }

        for (provider, rows) in &normalized.providers {
            validate_provider_aliases(provider, &rows.models)?;
        }

        Ok(normalized)
    }

    fn into_prices(self, source: &str) -> Result<Vec<ModelPrice>, String> {
        let normalized = self.normalized()?;
        let mut prices = Vec::new();
        for (provider, provider_rows) in normalized.providers {
            for (model_id, override_row) in provider_rows.models {
                let price = override_row
                    .into_model_price(&provider, model_id.clone(), source)
                    .map_err(|err| {
                        format!(
                            "invalid pricing override for provider '{provider}' model '{model_id}': {err}"
                        )
                    })?;
                prices.push(price);
            }
        }
        Ok(prices)
    }

    pub fn insert_model(
        &mut self,
        provider: &str,
        model_id: &str,
        row: LocalModelPriceOverride,
    ) -> Result<Option<LocalModelPriceOverride>, String> {
        if self.version != MODEL_PRICE_OVERRIDES_SCHEMA_VERSION || !self.models.is_empty() {
            *self = self.normalized()?;
        }
        let provider = require_canonical_provider(provider)?;
        let model_id = normalized_model_id(model_id)?;
        let row = row.sanitized(&model_id)?;
        self.version = MODEL_PRICE_OVERRIDES_SCHEMA_VERSION;
        Ok(self
            .providers
            .entry(provider)
            .or_default()
            .models
            .insert(model_id, row))
    }

    pub fn remove_model(
        &mut self,
        provider: &str,
        model_id: &str,
    ) -> Result<Option<LocalModelPriceOverride>, String> {
        if self.version != MODEL_PRICE_OVERRIDES_SCHEMA_VERSION || !self.models.is_empty() {
            *self = self.normalized()?;
        }
        let provider = require_canonical_provider(provider)?;
        let model_key = normalize_model_key(model_id);
        let Some(rows) = self.providers.get_mut(&provider) else {
            return Ok(None);
        };
        let Some(existing_id) = rows
            .models
            .keys()
            .find(|candidate| normalize_model_key(candidate) == model_key)
            .cloned()
        else {
            return Ok(None);
        };
        let removed = rows.models.remove(&existing_id);
        if rows.models.is_empty() {
            self.providers.remove(&provider);
        }
        Ok(removed)
    }

    pub fn model(&self, provider: &str, model_id: &str) -> Option<&LocalModelPriceOverride> {
        let provider = canonical_provider(provider)?;
        let model_key = normalize_model_key(model_id);
        self.providers
            .get(&provider)
            .and_then(|rows| {
                rows.models
                    .iter()
                    .find(|(candidate, _)| normalize_model_key(candidate) == model_key)
                    .map(|(_, row)| row)
            })
            .or_else(|| {
                (provider == DEFAULT_CANONICAL_PROVIDER).then_some(())?;
                self.models
                    .iter()
                    .find(|(candidate, _)| normalize_model_key(candidate) == model_key)
                    .map(|(_, row)| row)
            })
    }

    fn insert_normalized_row(
        &mut self,
        provider: &str,
        raw_model_id: &str,
        row: LocalModelPriceOverride,
        allow_identical_existing: bool,
    ) -> Result<(), String> {
        let model_id = normalized_model_id(raw_model_id)?;
        let model_key = normalize_model_key(&model_id);
        let row = row.sanitized(&model_id)?;
        let rows = &mut self
            .providers
            .entry(provider.to_string())
            .or_default()
            .models;
        if let Some((existing_id, existing_row)) = rows
            .iter()
            .find(|(candidate, _)| normalize_model_key(candidate) == model_key)
        {
            if allow_identical_existing
                && existing_id.as_str() == model_id.as_str()
                && existing_row == &row
            {
                return Ok(());
            }
            return Err(format!(
                "pricing override provider '{provider}' model id '{model_id}' conflicts with '{existing_id}' after normalization"
            ));
        }
        rows.insert(model_id, row);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalModelPriceTier {
    pub threshold_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_per_1m_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_per_1m_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_per_1m_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_per_1m_usd: Option<String>,
}

impl LocalModelPriceTier {
    fn sanitized(self) -> Result<Self, String> {
        if self.threshold_tokens == 0 {
            return Err("context tier threshold_tokens must be greater than zero".to_string());
        }
        validate_optional_usd_decimal("tier.input_per_1m_usd", &self.input_per_1m_usd)?;
        validate_optional_usd_decimal("tier.output_per_1m_usd", &self.output_per_1m_usd)?;
        validate_optional_usd_decimal(
            "tier.cache_read_input_per_1m_usd",
            &self.cache_read_input_per_1m_usd,
        )?;
        validate_optional_usd_decimal(
            "tier.cache_creation_input_per_1m_usd",
            &self.cache_creation_input_per_1m_usd,
        )?;
        if self.input_per_1m_usd.is_none()
            && self.output_per_1m_usd.is_none()
            && self.cache_read_input_per_1m_usd.is_none()
            && self.cache_creation_input_per_1m_usd.is_none()
        {
            return Err("context tier must overlay at least one price field".to_string());
        }
        Ok(self)
    }

    fn into_model_price_tier(self) -> Result<ModelPriceTier, String> {
        let tier = self.sanitized()?;
        ModelPriceTier::from_per_million_usd(
            tier.threshold_tokens,
            tier.input_per_1m_usd.as_deref(),
            tier.output_per_1m_usd.as_deref(),
            tier.cache_read_input_per_1m_usd.as_deref(),
            tier.cache_creation_input_per_1m_usd.as_deref(),
        )
        .ok_or_else(|| "invalid context tier USD decimal price".to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalModelPriceOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub input_per_1m_usd: String,
    pub output_per_1m_usd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_per_1m_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_per_1m_usd: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        alias = "context_tiers"
    )]
    pub tiers: Vec<LocalModelPriceTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<CostConfidence>,
}

impl LocalModelPriceOverride {
    pub fn sanitized(mut self, model_id: &str) -> Result<Self, String> {
        self.validate_prices()?;

        self.display_name = self
            .display_name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let model_key = normalize_model_key(model_id);
        let mut seen_aliases = BTreeSet::new();
        let mut aliases = Vec::new();
        for alias in self.aliases {
            let alias = alias.trim().to_string();
            if alias.is_empty() {
                return Err(format!("model '{model_id}' contains an empty alias"));
            }
            let alias_key = normalize_model_key(&alias);
            if alias_key == model_key {
                continue;
            }
            if seen_aliases.insert(alias_key) {
                aliases.push(alias);
            }
        }
        self.aliases = aliases;

        let mut seen_thresholds = BTreeSet::new();
        let mut tiers = Vec::with_capacity(self.tiers.len());
        for tier in self.tiers {
            let tier = tier.sanitized()?;
            if !seen_thresholds.insert(tier.threshold_tokens) {
                return Err(format!(
                    "model '{model_id}' contains duplicate context tier threshold {}",
                    tier.threshold_tokens
                ));
            }
            tiers.push(tier);
        }
        tiers.sort_by_key(|tier| tier.threshold_tokens);
        self.tiers = tiers;

        Ok(self)
    }

    fn validate_prices(&self) -> Result<(), String> {
        validate_usd_decimal("input_per_1m_usd", &self.input_per_1m_usd)?;
        validate_usd_decimal("output_per_1m_usd", &self.output_per_1m_usd)?;
        if let Some(value) = self.cache_read_input_per_1m_usd.as_deref() {
            validate_usd_decimal("cache_read_input_per_1m_usd", value)?;
        }
        if let Some(value) = self.cache_creation_input_per_1m_usd.as_deref() {
            validate_usd_decimal("cache_creation_input_per_1m_usd", value)?;
        }
        Ok(())
    }

    fn into_model_price(
        self,
        provider: &str,
        model_id: String,
        source: &str,
    ) -> Result<ModelPrice, String> {
        let row = self.sanitized(&model_id)?;
        let tiers = row
            .tiers
            .iter()
            .cloned()
            .map(LocalModelPriceTier::into_model_price_tier)
            .collect::<Result<Vec<_>, _>>()?;
        let mut price = ModelPrice::from_per_million_usd_for_provider(
            provider,
            model_id,
            row.display_name,
            &row.input_per_1m_usd,
            &row.output_per_1m_usd,
            row.cache_read_input_per_1m_usd.as_deref(),
            row.cache_creation_input_per_1m_usd.as_deref(),
            source.to_string(),
        )
        .ok_or_else(|| "invalid USD decimal price".to_string())?
        .with_aliases(row.aliases)
        .with_tiers(tiers)?;
        if let Some(confidence) = row.confidence {
            price.confidence = confidence;
        }
        Ok(price)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelPriceTierView {
    pub threshold_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_per_1m_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_per_1m_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_per_1m_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_per_1m_usd: Option<String>,
}

impl From<&ModelPriceTier> for ModelPriceTierView {
    fn from(tier: &ModelPriceTier) -> Self {
        Self {
            threshold_tokens: tier.threshold_tokens,
            input_per_1m_usd: tier.input_per_1m.map(UsdAmount::format_usd),
            output_per_1m_usd: tier.output_per_1m.map(UsdAmount::format_usd),
            cache_read_input_per_1m_usd: tier.cache_read_input_per_1m.map(UsdAmount::format_usd),
            cache_creation_input_per_1m_usd: tier
                .cache_creation_input_per_1m
                .map(UsdAmount::format_usd),
        }
    }
}

impl From<&ModelPriceTierView> for LocalModelPriceTier {
    fn from(tier: &ModelPriceTierView) -> Self {
        Self {
            threshold_tokens: tier.threshold_tokens,
            input_per_1m_usd: tier.input_per_1m_usd.clone(),
            output_per_1m_usd: tier.output_per_1m_usd.clone(),
            cache_read_input_per_1m_usd: tier.cache_read_input_per_1m_usd.clone(),
            cache_creation_input_per_1m_usd: tier.cache_creation_input_per_1m_usd.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelPriceView {
    #[serde(default = "default_canonical_provider", alias = "canonical_provider")]
    pub provider: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub input_per_1m_usd: String,
    pub output_per_1m_usd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_per_1m_usd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_per_1m_usd: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        alias = "context_tiers"
    )]
    pub tiers: Vec<ModelPriceTierView>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_generation: Option<String>,
    pub confidence: CostConfidence,
}

impl ModelPriceView {
    pub fn matches_model(&self, model: &str) -> bool {
        let lookup_keys = model_lookup_keys(model);
        std::iter::once(self.model_id.as_str())
            .chain(self.aliases.iter().map(String::as_str))
            .map(normalize_model_key)
            .any(|price_key| {
                lookup_keys
                    .iter()
                    .any(|lookup_key| lookup_key == &price_key)
            })
    }

    pub fn matches_provider(&self, provider: &str) -> bool {
        canonical_provider(provider)
            .zip(canonical_provider(&self.provider))
            .is_some_and(|(expected, actual)| expected == actual)
    }
}

impl From<&ModelPrice> for ModelPriceView {
    fn from(price: &ModelPrice) -> Self {
        Self {
            provider: price.provider.clone(),
            model_id: price.model_id.clone(),
            display_name: price.display_name.clone(),
            aliases: price.aliases.clone(),
            input_per_1m_usd: price.input_per_1m.format_usd(),
            output_per_1m_usd: price.output_per_1m.format_usd(),
            cache_read_input_per_1m_usd: price.cache_read_input_per_1m.map(UsdAmount::format_usd),
            cache_creation_input_per_1m_usd: price
                .cache_creation_input_per_1m
                .map(UsdAmount::format_usd),
            tiers: price.tiers.iter().map(ModelPriceTierView::from).collect(),
            source: price.source.clone(),
            source_generation: price.source_generation.clone(),
            confidence: price.confidence,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelPriceCatalogSnapshot {
    pub source: String,
    pub model_count: usize,
    #[serde(default)]
    pub models: Vec<ModelPriceView>,
}

impl ModelPriceCatalogSnapshot {
    pub fn prioritized_models<I, S>(&self, observed_models: I, limit: usize) -> Vec<&ModelPriceView>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.prioritized_models_for_provider(DEFAULT_CANONICAL_PROVIDER, observed_models, limit)
    }

    pub fn prioritized_models_for_service<I, S>(
        &self,
        service: &str,
        observed_models: I,
        limit: usize,
    ) -> Vec<&ModelPriceView>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.prioritized_models_for_provider(
            &canonical_provider_for_service(service),
            observed_models,
            limit,
        )
    }

    pub fn prioritized_models_for_provider<I, S>(
        &self,
        provider: &str,
        observed_models: I,
        limit: usize,
    ) -> Vec<&ModelPriceView>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let Some(provider) = canonical_provider(provider) else {
            return Vec::new();
        };
        if limit == 0 {
            return Vec::new();
        }
        let mut used = BTreeSet::new();
        let mut rows = Vec::new();

        for model in observed_models {
            let model = model.as_ref().trim();
            if model.is_empty() {
                continue;
            }
            if let Some((idx, row)) = self.models.iter().enumerate().find(|(idx, row)| {
                !used.contains(idx) && row.matches_provider(&provider) && row.matches_model(model)
            }) {
                used.insert(idx);
                rows.push(row);
                if rows.len() >= limit {
                    return rows;
                }
            }
        }

        for (idx, row) in self.models.iter().enumerate() {
            if row.matches_provider(&provider) && used.insert(idx) {
                rows.push(row);
                if rows.len() >= limit {
                    break;
                }
            }
        }

        rows
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModelPriceCatalog {
    entries: BTreeMap<(String, String), ModelPrice>,
    aliases: BTreeMap<(String, String), (String, String)>,
}

impl ModelPriceCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_prices(prices: impl IntoIterator<Item = ModelPrice>) -> Self {
        let mut catalog = Self::new();
        for price in prices {
            catalog.insert(price);
        }
        catalog
    }

    pub fn insert(&mut self, mut price: ModelPrice) {
        let Some(provider) = canonical_provider(&price.provider) else {
            tracing::warn!(model = %price.model_id, "ignored model price with an empty provider");
            return;
        };
        price.provider = provider.clone();
        let key = (provider.clone(), normalize_model_key(&price.model_id));
        self.aliases.retain(|_, target| target != &key);
        for alias in &price.aliases {
            self.aliases
                .insert((provider.clone(), normalize_model_key(alias)), key.clone());
        }
        self.entries.insert(key, price);
    }

    pub fn price_for_model(&self, model: &str) -> Option<&ModelPrice> {
        self.price_for_provider_model(DEFAULT_CANONICAL_PROVIDER, model)
    }

    pub fn price_for_service_model(&self, service: &str, model: &str) -> Option<&ModelPrice> {
        let provider = canonical_provider_for_service(service);
        self.price_for_provider_model(&provider, model)
    }

    pub fn price_for_provider_model(&self, provider: &str, model: &str) -> Option<&ModelPrice> {
        let provider = canonical_provider(provider)?;
        for key in model_lookup_keys(model) {
            let lookup_key = (provider.clone(), key);
            if let Some(price) = self.entries.get(&lookup_key) {
                return Some(price);
            }
            if let Some(target) = self.aliases.get(&lookup_key)
                && let Some(price) = self.entries.get(target)
            {
                return Some(price);
            }
        }
        None
    }

    pub fn estimate_usage_cost(
        &self,
        model: &str,
        usage: &UsageMetrics,
        adjustments: CostAdjustments,
    ) -> CostBreakdown {
        self.estimate_usage_cost_with_accounting(
            model,
            usage,
            adjustments,
            CacheInputAccounting::default(),
        )
    }

    pub fn estimate_usage_cost_with_accounting(
        &self,
        model: &str,
        usage: &UsageMetrics,
        adjustments: CostAdjustments,
        accounting: CacheInputAccounting,
    ) -> CostBreakdown {
        let Some(price) = self.price_for_model(model) else {
            return CostBreakdown::unknown();
        };
        estimate_usage_cost_with_accounting(usage, price, adjustments, accounting)
    }

    pub fn estimate_usage_cost_for_service_with_accounting(
        &self,
        service: &str,
        model: &str,
        usage: &UsageMetrics,
        adjustments: CostAdjustments,
        accounting: CacheInputAccounting,
    ) -> CostBreakdown {
        let Some(price) = self.price_for_service_model(service, model) else {
            return CostBreakdown::unknown();
        };
        estimate_usage_cost_with_accounting(usage, price, adjustments, accounting)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn snapshot(&self, source: impl Into<String>) -> ModelPriceCatalogSnapshot {
        let models = self
            .entries
            .values()
            .map(ModelPriceView::from)
            .collect::<Vec<_>>();
        ModelPriceCatalogSnapshot {
            source: source.into(),
            model_count: models.len(),
            models,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ManualPricingLayerStatus {
    #[default]
    Missing,
    Applied,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct EffectivePricingCatalogSnapshot {
    pub revision: String,
    pub source: String,
    pub model_count: usize,
    pub bundled_model_count: usize,
    pub remote_model_count: usize,
    pub manual_model_count: usize,
    pub remote_content_revision: Option<String>,
    pub remote_projection_warnings: Vec<String>,
    pub manual_content_hash: Option<String>,
    pub manual_file_size: Option<u64>,
    pub manual_modified_unix_ms: Option<u64>,
    pub manual_status: ManualPricingLayerStatus,
    pub manual_error: Option<String>,
    catalog: ModelPriceCatalog,
}

impl EffectivePricingCatalogSnapshot {
    pub fn catalog(&self) -> &ModelPriceCatalog {
        &self.catalog
    }

    pub fn catalog_snapshot(&self) -> ModelPriceCatalogSnapshot {
        self.catalog.snapshot(self.source.clone())
    }

    pub fn estimate_request_cost(
        &self,
        model: Option<&str>,
        usage: Option<&UsageMetrics>,
        adjustments: CostAdjustments,
        accounting: CacheInputAccounting,
    ) -> CostBreakdown {
        let cost = match (model, usage) {
            (Some(model), Some(usage)) => self.catalog.estimate_usage_cost_with_accounting(
                model,
                usage,
                adjustments,
                accounting,
            ),
            _ => CostBreakdown::unknown(),
        };
        self.capture_revision(cost)
    }

    pub fn estimate_request_cost_for_service(
        &self,
        service: &str,
        model: Option<&str>,
        usage: Option<&UsageMetrics>,
        adjustments: CostAdjustments,
    ) -> CostBreakdown {
        let cost = match (model, usage) {
            (Some(model), Some(usage)) => self
                .catalog
                .estimate_usage_cost_for_service_with_accounting(
                    service,
                    model,
                    usage,
                    adjustments,
                    CacheInputAccounting::for_service(service),
                ),
            _ => CostBreakdown::unknown(),
        };
        self.capture_revision(cost)
    }

    fn capture_revision(&self, mut cost: CostBreakdown) -> CostBreakdown {
        cost.effective_pricing_revision = Some(self.revision.clone());
        cost
    }

    fn has_same_observation(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.source == other.source
            && self.model_count == other.model_count
            && self.bundled_model_count == other.bundled_model_count
            && self.remote_model_count == other.remote_model_count
            && self.manual_model_count == other.manual_model_count
            && self.remote_content_revision == other.remote_content_revision
            && self.remote_projection_warnings == other.remote_projection_warnings
            && self.manual_content_hash == other.manual_content_hash
            && self.manual_file_size == other.manual_file_size
            && self.manual_modified_unix_ms == other.manual_modified_unix_ms
            && self.manual_status == other.manual_status
            && self.manual_error == other.manual_error
    }
}

#[derive(Debug)]
struct EffectivePricingCatalogStore {
    current: RwLock<Arc<EffectivePricingCatalogSnapshot>>,
    refresh_lock: Mutex<()>,
}

impl EffectivePricingCatalogStore {
    fn new(snapshot: EffectivePricingCatalogSnapshot) -> Self {
        Self {
            current: RwLock::new(Arc::new(snapshot)),
            refresh_lock: Mutex::new(()),
        }
    }

    fn load(&self) -> Arc<EffectivePricingCatalogSnapshot> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn install(
        &self,
        candidate: EffectivePricingCatalogSnapshot,
    ) -> Arc<EffectivePricingCatalogSnapshot> {
        let current = self.load();
        if current.has_same_observation(&candidate) {
            return current;
        }

        let candidate = Arc::new(candidate);
        *self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = candidate.clone();
        candidate
    }
}

#[derive(Debug, Clone)]
struct ManualPricingLayer {
    status: ManualPricingLayerStatus,
    content_hash: Option<String>,
    file_size: Option<u64>,
    modified_unix_ms: Option<u64>,
    error: Option<String>,
    prices: Vec<ModelPrice>,
}

impl ManualPricingLayer {
    fn missing() -> Self {
        Self {
            status: ManualPricingLayerStatus::Missing,
            content_hash: None,
            file_size: None,
            modified_unix_ms: None,
            error: None,
            prices: Vec::new(),
        }
    }

    fn invalid(
        content_hash: Option<String>,
        file_size: Option<u64>,
        modified_unix_ms: Option<u64>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            status: ManualPricingLayerStatus::Invalid,
            content_hash,
            file_size,
            modified_unix_ms,
            error: Some(error.into()),
            prices: Vec::new(),
        }
    }
}

static EFFECTIVE_PRICING_CATALOG: OnceLock<EffectivePricingCatalogStore> = OnceLock::new();

fn effective_pricing_catalog_store() -> &'static EffectivePricingCatalogStore {
    EFFECTIVE_PRICING_CATALOG.get_or_init(|| {
        EffectivePricingCatalogStore::new(build_effective_pricing_catalog_snapshot(
            basellm_catalog_snapshot().as_deref(),
            &model_price_overrides_path(),
        ))
    })
}

pub fn effective_pricing_catalog_snapshot() -> Arc<EffectivePricingCatalogSnapshot> {
    effective_pricing_catalog_store().load()
}

pub fn refresh_effective_pricing_catalog() -> Arc<EffectivePricingCatalogSnapshot> {
    let store = effective_pricing_catalog_store();
    let _refresh_guard = store
        .refresh_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let candidate = build_effective_pricing_catalog_snapshot(
        basellm_catalog_snapshot().as_deref(),
        &model_price_overrides_path(),
    );
    store.install(candidate)
}

fn build_effective_pricing_catalog_snapshot(
    remote: Option<&BasellmCatalogLkg>,
    manual_path: &Path,
) -> EffectivePricingCatalogSnapshot {
    let remote_prices = remote.map(project_basellm_model_prices).unwrap_or_default();
    let manual = load_manual_pricing_layer(manual_path);
    build_effective_pricing_catalog_from_layers(
        remote.map(|snapshot| snapshot.content_hash.clone()),
        remote_prices,
        manual,
    )
}

fn project_basellm_model_prices(snapshot: &BasellmCatalogLkg) -> Vec<ModelPrice> {
    snapshot
        .catalog
        .providers
        .keys()
        .flat_map(|provider| snapshot.model_prices_for_provider(provider))
        .collect()
}

fn build_effective_pricing_catalog_from_layers(
    remote_content_revision: Option<String>,
    remote_prices: Vec<ModelPrice>,
    manual: ManualPricingLayer,
) -> EffectivePricingCatalogSnapshot {
    build_effective_pricing_catalog_from_layers_with_warnings(
        remote_content_revision,
        remote_prices,
        Vec::new(),
        manual,
    )
}

fn build_effective_pricing_catalog_from_layers_with_warnings(
    remote_content_revision: Option<String>,
    remote_prices: Vec<ModelPrice>,
    mut remote_projection_warnings: Vec<String>,
    manual: ManualPricingLayer,
) -> EffectivePricingCatalogSnapshot {
    let bundled = bundled_model_price_catalog();
    let bundled_prices = bundled.entries.values().cloned().collect::<Vec<_>>();
    let (remote_prices, collision_warnings) = reject_ambiguous_remote_prices(remote_prices);
    remote_projection_warnings.extend(collision_warnings);
    remote_projection_warnings.sort();
    remote_projection_warnings.dedup();
    let applied_manual_prices = if manual.status == ManualPricingLayerStatus::Applied {
        manual.prices
    } else {
        Vec::new()
    };
    let revision = effective_pricing_content_revision(
        &bundled_prices,
        remote_content_revision.as_deref(),
        &remote_prices,
        &applied_manual_prices,
    );

    let bundled_model_count = bundled_prices.len();
    let remote_model_count = remote_prices.len();
    let manual_model_count = applied_manual_prices.len();
    let mut catalog = bundled.clone();
    for price in remote_prices {
        catalog.insert(price);
    }
    for price in applied_manual_prices {
        catalog.insert(price);
    }

    let source = match (remote_model_count > 0, manual_model_count > 0) {
        (false, false) => "bundled".to_string(),
        (true, false) => format!("bundled+basellm({remote_model_count})"),
        (false, true) => format!("bundled+manual({manual_model_count})"),
        (true, true) => {
            format!("bundled+basellm({remote_model_count})+manual({manual_model_count})")
        }
    };

    EffectivePricingCatalogSnapshot {
        revision,
        source,
        model_count: catalog.len(),
        bundled_model_count,
        remote_model_count,
        manual_model_count,
        remote_content_revision,
        remote_projection_warnings,
        manual_content_hash: manual.content_hash,
        manual_file_size: manual.file_size,
        manual_modified_unix_ms: manual.modified_unix_ms,
        manual_status: manual.status,
        manual_error: manual.error,
        catalog,
    }
}

fn reject_ambiguous_remote_prices(prices: Vec<ModelPrice>) -> (Vec<ModelPrice>, Vec<String>) {
    let mut unique = BTreeMap::<(String, String), ModelPrice>::new();
    let mut rejected = BTreeSet::<(String, String)>::new();
    let mut warnings = Vec::new();

    for price in prices {
        let Some(provider) = canonical_provider(&price.provider) else {
            continue;
        };
        let key = (provider, normalize_model_key(&price.model_id));
        if rejected.contains(&key) {
            continue;
        }
        if unique.remove(&key).is_some() {
            rejected.insert(key.clone());
            warnings.push(format!(
                "ignored ambiguous remote price for canonical provider/model '{}/{}'",
                key.0, key.1
            ));
            continue;
        }
        unique.insert(key, price);
    }

    (unique.into_values().collect(), warnings)
}

fn load_manual_pricing_layer(path: &Path) -> ManualPricingLayer {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return ManualPricingLayer::missing(),
        Err(err) => {
            return ManualPricingLayer::invalid(
                None,
                None,
                None,
                format!("failed to open {}: {err}", path.display()),
            );
        }
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(err) => {
            return ManualPricingLayer::invalid(
                None,
                None,
                None,
                format!("failed to inspect {}: {err}", path.display()),
            );
        }
    };
    let file_size = metadata.len();
    let modified_unix_ms = metadata.modified().ok().and_then(|modified| {
        modified
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
    });
    if file_size > MAX_MODEL_PRICE_OVERRIDES_BYTES {
        return ManualPricingLayer::invalid(
            None,
            Some(file_size),
            modified_unix_ms,
            format!(
                "pricing override is {file_size} bytes; maximum is {MAX_MODEL_PRICE_OVERRIDES_BYTES}"
            ),
        );
    }

    let mut bytes = Vec::with_capacity(usize::try_from(file_size).unwrap_or(0));
    let mut bounded = file.take(MAX_MODEL_PRICE_OVERRIDES_BYTES.saturating_add(1));
    if let Err(err) = bounded.read_to_end(&mut bytes) {
        return ManualPricingLayer::invalid(
            None,
            Some(file_size),
            modified_unix_ms,
            format!("failed to read {}: {err}", path.display()),
        );
    }
    if bytes.len() as u64 > MAX_MODEL_PRICE_OVERRIDES_BYTES {
        return ManualPricingLayer::invalid(
            None,
            Some(bytes.len() as u64),
            modified_unix_ms,
            format!(
                "pricing override exceeded {MAX_MODEL_PRICE_OVERRIDES_BYTES} bytes while reading"
            ),
        );
    }
    let content_hash = sha256_hex(&bytes);
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(err) => {
            return ManualPricingLayer::invalid(
                Some(content_hash),
                Some(bytes.len() as u64),
                modified_unix_ms,
                format!("pricing override is not UTF-8: {err}"),
            );
        }
    };
    let document = match parse_model_price_overrides_document(text) {
        Ok(document) => document,
        Err(err) => {
            return ManualPricingLayer::invalid(
                Some(content_hash),
                Some(bytes.len() as u64),
                modified_unix_ms,
                err,
            );
        }
    };
    let source = format!("local:{}", path.display());
    match document.into_prices(&source) {
        Ok(prices) => ManualPricingLayer {
            status: ManualPricingLayerStatus::Applied,
            content_hash: Some(content_hash),
            file_size: Some(bytes.len() as u64),
            modified_unix_ms,
            error: None,
            prices,
        },
        Err(err) => ManualPricingLayer::invalid(
            Some(content_hash),
            Some(bytes.len() as u64),
            modified_unix_ms,
            err,
        ),
    }
}

fn effective_pricing_content_revision(
    bundled_prices: &[ModelPrice],
    remote_content_revision: Option<&str>,
    remote_prices: &[ModelPrice],
    manual_prices: &[ModelPrice],
) -> String {
    let mut hasher = Sha256::new();
    hash_revision_text(&mut hasher, EFFECTIVE_PRICING_REVISION_SCHEMA);
    hash_revision_text(&mut hasher, CANONICAL_PROVIDER_MAPPING_REVISION);
    hash_price_layer(&mut hasher, "bundled", bundled_prices);
    hash_revision_optional_text(&mut hasher, remote_content_revision);
    hash_price_layer(&mut hasher, "remote", remote_prices);
    hash_price_layer(&mut hasher, "manual", manual_prices);
    digest_hex(hasher.finalize().as_slice())
}

fn hash_price_layer(hasher: &mut Sha256, layer: &str, prices: &[ModelPrice]) {
    hash_revision_text(hasher, layer);
    let mut prices = prices.iter().collect::<Vec<_>>();
    prices.sort_by(|left, right| {
        let left_key = (
            canonical_provider(&left.provider).unwrap_or_default(),
            normalize_model_key(&left.model_id),
        );
        let right_key = (
            canonical_provider(&right.provider).unwrap_or_default(),
            normalize_model_key(&right.model_id),
        );
        left_key.cmp(&right_key)
    });
    hasher.update((prices.len() as u64).to_be_bytes());
    for price in prices {
        hash_revision_text(
            hasher,
            &canonical_provider(&price.provider).unwrap_or_default(),
        );
        hash_revision_text(hasher, &normalize_model_key(&price.model_id));
        hash_revision_optional_text(hasher, price.display_name.as_deref().map(str::trim));
        let mut aliases = price
            .aliases
            .iter()
            .map(|alias| normalize_model_key(alias))
            .filter(|alias| !alias.is_empty())
            .collect::<Vec<_>>();
        aliases.sort();
        aliases.dedup();
        hasher.update((aliases.len() as u64).to_be_bytes());
        for alias in aliases {
            hash_revision_text(hasher, &alias);
        }
        hash_revision_usd(hasher, price.input_per_1m);
        hash_revision_usd(hasher, price.output_per_1m);
        hash_revision_optional_usd(hasher, price.cache_read_input_per_1m);
        hash_revision_optional_usd(hasher, price.cache_creation_input_per_1m);

        let mut tiers = price.tiers.iter().collect::<Vec<_>>();
        tiers.sort_by_key(|tier| tier.threshold_tokens);
        hasher.update((tiers.len() as u64).to_be_bytes());
        for tier in tiers {
            hasher.update(tier.threshold_tokens.to_be_bytes());
            hash_revision_optional_usd(hasher, tier.input_per_1m);
            hash_revision_optional_usd(hasher, tier.output_per_1m);
            hash_revision_optional_usd(hasher, tier.cache_read_input_per_1m);
            hash_revision_optional_usd(hasher, tier.cache_creation_input_per_1m);
        }
        hasher.update([match price.confidence {
            CostConfidence::Unknown => 0,
            CostConfidence::Partial => 1,
            CostConfidence::Estimated => 2,
            CostConfidence::Exact => 3,
        }]);
    }
}

fn hash_revision_text(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn hash_revision_optional_text(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hash_revision_text(hasher, value);
        }
        None => hasher.update([0]),
    }
}

fn hash_revision_usd(hasher: &mut Sha256, value: UsdAmount) {
    hasher.update(value.femto_usd().to_be_bytes());
}

fn hash_revision_optional_usd(hasher: &mut Sha256, value: Option<UsdAmount>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hash_revision_usd(hasher, value);
        }
        None => hasher.update([0]),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    digest_hex(Sha256::digest(bytes).as_slice())
}

fn digest_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn bundled_model_price_catalog() -> &'static ModelPriceCatalog {
    static CATALOG: OnceLock<ModelPriceCatalog> = OnceLock::new();
    CATALOG.get_or_init(build_bundled_model_price_catalog)
}

pub fn bundled_model_price_catalog_snapshot() -> ModelPriceCatalogSnapshot {
    bundled_model_price_catalog().snapshot("bundled")
}

pub fn basellm_all_json_url() -> &'static str {
    BASELLM_ALL_JSON_URL
}

pub fn basellm_model_price_catalog_snapshot_from_json(
    source: impl Into<String>,
    text: &str,
) -> Result<ModelPriceCatalogSnapshot, String> {
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|err| format!("invalid basellm JSON: {err}"))?;
    let provider_map = root
        .as_object()
        .ok_or_else(|| "basellm all.json root must be an object".to_string())?;

    let mut models = Vec::new();
    let mut seen_models = BTreeSet::new();
    for (provider_name, provider_value) in provider_map {
        let Some(provider) = canonical_provider(provider_name) else {
            continue;
        };
        let Some(models_map) = provider_value
            .get("models")
            .and_then(|value| value.as_object())
        else {
            continue;
        };

        for (model_id, model_value) in models_map {
            let Some(cost) = model_value.get("cost").and_then(|value| value.as_object()) else {
                continue;
            };
            let Some(input) = basellm_cost_field(cost, "input") else {
                continue;
            };
            let Some(output) = basellm_cost_field(cost, "output") else {
                continue;
            };

            let cache_read = basellm_cost_field(cost, "cache_read");
            let cache_creation = basellm_cost_field(cost, "cache_write");
            let tiers = basellm_context_tiers(cost, provider_name, model_id)?;
            let display_name = model_value
                .get("name")
                .and_then(json_scalar_to_string)
                .or_else(|| {
                    model_value
                        .get("display_name")
                        .and_then(json_scalar_to_string)
                })
                .filter(|value| value != model_id);

            let model_key = (provider.clone(), normalize_model_key(model_id));
            if !seen_models.insert(model_key) {
                return Err(format!(
                    "basellm contains duplicate canonical provider/model '{provider}/{model_id}'"
                ));
            }

            models.push(ModelPriceView {
                provider: provider.clone(),
                model_id: model_id.to_string(),
                display_name,
                aliases: basellm_aliases(model_value),
                input_per_1m_usd: input,
                output_per_1m_usd: output,
                cache_read_input_per_1m_usd: cache_read,
                cache_creation_input_per_1m_usd: cache_creation,
                tiers,
                source: format!("basellm:{provider_name}"),
                source_generation: None,
                confidence: CostConfidence::Estimated,
            });
        }
    }

    models.sort_by(|left, right| {
        left.provider
            .cmp(&right.provider)
            .then_with(|| left.model_id.cmp(&right.model_id))
    });
    Ok(ModelPriceCatalogSnapshot {
        source: source.into(),
        model_count: models.len(),
        models,
    })
}

fn basellm_cost_field(
    cost: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<String> {
    cost.get(key).and_then(json_scalar_to_string)
}

fn basellm_context_tiers(
    cost: &serde_json::Map<String, serde_json::Value>,
    provider: &str,
    model_id: &str,
) -> Result<Vec<ModelPriceTierView>, String> {
    let Some(raw_tiers) = cost.get("tiers") else {
        return Ok(Vec::new());
    };
    let tiers = raw_tiers
        .as_array()
        .ok_or_else(|| format!("basellm tiers for '{provider}/{model_id}' must be an array"))?;
    let mut parsed = Vec::new();
    let mut seen_thresholds = BTreeSet::new();

    for (index, raw_tier) in tiers.iter().enumerate() {
        let Some(row) = raw_tier.as_object() else {
            tracing::warn!(
                provider,
                model = model_id,
                index,
                "ignored non-object basellm tier"
            );
            continue;
        };
        let descriptor = row.get("tier").and_then(serde_json::Value::as_object);
        let tier_type = descriptor
            .and_then(|tier| tier.get("type"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim);
        if !tier_type.is_some_and(|tier_type| tier_type.eq_ignore_ascii_case("context")) {
            tracing::warn!(
                provider,
                model = model_id,
                index,
                tier_type = tier_type.unwrap_or("unknown"),
                "ignored unsupported basellm pricing tier"
            );
            continue;
        }

        let threshold_tokens = descriptor
            .and_then(|tier| tier.get("size"))
            .and_then(json_u64)
            .filter(|threshold| *threshold > 0)
            .ok_or_else(|| {
                format!(
                    "basellm context tier {index} for '{provider}/{model_id}' has an invalid positive size"
                )
            })?;
        if !seen_thresholds.insert(threshold_tokens) {
            return Err(format!(
                "basellm model '{provider}/{model_id}' contains duplicate context tier threshold {threshold_tokens}"
            ));
        }

        let tier = ModelPriceTierView {
            threshold_tokens,
            input_per_1m_usd: basellm_optional_tier_cost(row, "input", provider, model_id)?,
            output_per_1m_usd: basellm_optional_tier_cost(row, "output", provider, model_id)?,
            cache_read_input_per_1m_usd: basellm_optional_tier_cost(
                row,
                "cache_read",
                provider,
                model_id,
            )?,
            cache_creation_input_per_1m_usd: basellm_optional_tier_cost(
                row,
                "cache_write",
                provider,
                model_id,
            )?,
        };
        let local_tier = LocalModelPriceTier::from(&tier).sanitized()?;
        parsed.push(ModelPriceTierView {
            threshold_tokens: local_tier.threshold_tokens,
            input_per_1m_usd: local_tier.input_per_1m_usd,
            output_per_1m_usd: local_tier.output_per_1m_usd,
            cache_read_input_per_1m_usd: local_tier.cache_read_input_per_1m_usd,
            cache_creation_input_per_1m_usd: local_tier.cache_creation_input_per_1m_usd,
        });
    }

    parsed.sort_by_key(|tier| tier.threshold_tokens);
    Ok(parsed)
}

fn basellm_optional_tier_cost(
    row: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    provider: &str,
    model_id: &str,
) -> Result<Option<String>, String> {
    let Some(value) = row.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = json_scalar_to_string(value).ok_or_else(|| {
        format!("basellm tier field '{key}' for '{provider}/{model_id}' must be numeric")
    })?;
    validate_usd_decimal(key, &value)?;
    Ok(Some(value))
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn basellm_aliases(model_value: &serde_json::Value) -> Vec<String> {
    let mut aliases = Vec::new();
    if let Some(value) = model_value.get("aliases") {
        match value {
            serde_json::Value::Array(items) => {
                for item in items {
                    if let Some(alias) = json_scalar_to_string(item) {
                        let alias = alias.trim();
                        if !alias.is_empty() {
                            aliases.push(alias.to_string());
                        }
                    }
                }
            }
            serde_json::Value::String(alias) => {
                let alias = alias.trim();
                if !alias.is_empty() {
                    aliases.push(alias.to_string());
                }
            }
            _ => {}
        }
    }
    aliases
}

fn json_scalar_to_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        _ => None,
    }
}

pub fn model_price_overrides_path() -> PathBuf {
    crate::config::proxy_home_dir().join("pricing_overrides.toml")
}

fn parse_model_price_overrides_document(
    text: &str,
) -> Result<LocalModelPriceOverridesDocument, String> {
    let parsed: LocalModelPriceOverridesDocument =
        toml::from_str(text).map_err(|err| format!("invalid pricing override TOML: {err}"))?;
    validate_model_price_overrides_document(&parsed)?;
    Ok(parsed)
}

pub fn load_model_price_overrides_document() -> Result<LocalModelPriceOverridesDocument, String> {
    let path = model_price_overrides_path();
    if !path.exists() {
        return Ok(LocalModelPriceOverridesDocument::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    parse_model_price_overrides_document(&text)
}

fn render_model_price_overrides_document(
    document: &LocalModelPriceOverridesDocument,
) -> Result<String, String> {
    validate_model_price_overrides_document(document)?;
    let mut persisted = document.normalized()?;
    persisted.models = persisted
        .providers
        .get(DEFAULT_CANONICAL_PROVIDER)
        .map(|provider| provider.models.clone())
        .unwrap_or_default();
    validate_model_price_overrides_document(&persisted)?;

    let body = toml::to_string_pretty(&persisted)
        .map_err(|err| format!("failed to serialize pricing overrides: {err}"))?;
    Ok(if body.trim().is_empty() {
        MODEL_PRICE_OVERRIDES_DOC_HEADER.to_string()
    } else {
        format!("{MODEL_PRICE_OVERRIDES_DOC_HEADER}\n{body}")
    })
}

pub fn save_model_price_overrides_document(
    document: &LocalModelPriceOverridesDocument,
) -> Result<PathBuf, String> {
    let path = model_price_overrides_path();
    let text = render_model_price_overrides_document(document)?;
    write_text_file(&path, &text)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    let _snapshot = refresh_effective_pricing_catalog();
    Ok(path)
}

pub fn local_model_price_catalog_snapshot() -> Result<ModelPriceCatalogSnapshot, String> {
    let path = model_price_overrides_path();
    let document = load_model_price_overrides_document()?;
    let source = format!("local:{}", path.display());
    let prices = document.into_prices(&source)?;
    Ok(ModelPriceCatalog::with_prices(prices).snapshot(source))
}

pub fn validate_model_price_overrides_document(
    document: &LocalModelPriceOverridesDocument,
) -> Result<(), String> {
    document.normalized().map(|_| ())
}

fn validate_provider_aliases(
    provider: &str,
    models: &BTreeMap<String, LocalModelPriceOverride>,
) -> Result<(), String> {
    let mut seen_model_ids = BTreeMap::<String, String>::new();
    let mut seen_aliases = BTreeMap::<String, String>::new();

    for (model_id, row) in models {
        let model_key = normalize_model_key(model_id);

        if let Some(existing) = seen_aliases.get(&model_key)
            && existing.as_str() != model_id.as_str()
        {
            return Err(format!(
                "pricing override provider '{provider}' model id '{model_id}' conflicts with alias from '{existing}'"
            ));
        }

        if let Some(existing) = seen_model_ids.insert(model_key.clone(), model_id.to_string())
            && existing.as_str() != model_id.as_str()
        {
            return Err(format!(
                "pricing override provider '{provider}' model id '{model_id}' conflicts with '{existing}' after case-insensitive normalization"
            ));
        }

        row.clone().sanitized(model_id)?;

        let mut row_aliases = BTreeSet::new();
        for alias in &row.aliases {
            let alias = alias.trim();
            if alias.is_empty() {
                return Err(format!(
                    "pricing override model '{model_id}' contains an empty alias"
                ));
            }

            let alias_key = normalize_model_key(alias);
            if alias_key == model_key {
                continue;
            }
            if !row_aliases.insert(alias_key.clone()) {
                continue;
            }

            if let Some(existing) = seen_model_ids.get(&alias_key) {
                return Err(format!(
                    "pricing override provider '{provider}' alias '{alias}' for model '{model_id}' conflicts with model id '{existing}'"
                ));
            }

            if let Some(existing) = seen_aliases.insert(alias_key.clone(), model_id.to_string())
                && existing.as_str() != model_id.as_str()
            {
                return Err(format!(
                    "pricing override provider '{provider}' alias '{alias}' is used by both '{existing}' and '{model_id}'"
                ));
            }
        }
    }

    Ok(())
}

pub fn operator_model_price_catalog_snapshot() -> ModelPriceCatalogSnapshot {
    effective_pricing_catalog_snapshot().catalog_snapshot()
}

pub fn estimate_request_cost_from_operator_catalog(
    model: Option<&str>,
    usage: Option<&UsageMetrics>,
    adjustments: CostAdjustments,
) -> CostBreakdown {
    estimate_request_cost_from_operator_catalog_with_accounting(
        model,
        usage,
        adjustments,
        CacheInputAccounting::default(),
    )
}

pub fn estimate_request_cost_from_operator_catalog_for_service(
    model: Option<&str>,
    usage: Option<&UsageMetrics>,
    adjustments: CostAdjustments,
    service: &str,
) -> CostBreakdown {
    let snapshot = effective_pricing_catalog_snapshot();
    snapshot.estimate_request_cost_for_service(service, model, usage, adjustments)
}

pub fn estimate_request_cost_from_operator_catalog_with_accounting(
    model: Option<&str>,
    usage: Option<&UsageMetrics>,
    adjustments: CostAdjustments,
    accounting: CacheInputAccounting,
) -> CostBreakdown {
    let snapshot = effective_pricing_catalog_snapshot();
    snapshot.estimate_request_cost(model, usage, adjustments, accounting)
}

pub fn estimate_request_cost_from_bundled_catalog(
    model: Option<&str>,
    usage: Option<&UsageMetrics>,
    adjustments: CostAdjustments,
) -> CostBreakdown {
    estimate_request_cost_from_bundled_catalog_with_accounting(
        model,
        usage,
        adjustments,
        CacheInputAccounting::default(),
    )
}

pub fn estimate_request_cost_from_bundled_catalog_with_accounting(
    model: Option<&str>,
    usage: Option<&UsageMetrics>,
    adjustments: CostAdjustments,
    accounting: CacheInputAccounting,
) -> CostBreakdown {
    let (Some(model), Some(usage)) = (model, usage) else {
        return CostBreakdown::unknown();
    };
    bundled_model_price_catalog().estimate_usage_cost_with_accounting(
        model,
        usage,
        adjustments,
        accounting,
    )
}

pub fn estimate_usage_cost(
    usage: &UsageMetrics,
    price: &ModelPrice,
    adjustments: CostAdjustments,
) -> CostBreakdown {
    estimate_usage_cost_with_accounting(usage, price, adjustments, CacheInputAccounting::default())
}

pub fn estimate_usage_cost_with_accounting(
    usage: &UsageMetrics,
    price: &ModelPrice,
    adjustments: CostAdjustments,
    accounting: CacheInputAccounting,
) -> CostBreakdown {
    let billable = BillableTokenUsage::from_usage_with_accounting(usage, accounting);
    let matched_input_tokens = billable.context_input_tokens();
    let selected_tier = price
        .tiers
        .iter()
        .filter(|tier| matched_input_tokens > tier.threshold_tokens)
        .max_by_key(|tier| tier.threshold_tokens);

    let input_price = selected_tier
        .and_then(|tier| tier.input_per_1m)
        .unwrap_or(price.input_per_1m);
    let output_price = selected_tier
        .and_then(|tier| tier.output_per_1m)
        .unwrap_or(price.output_per_1m);
    let cache_read_price = selected_tier
        .and_then(|tier| tier.cache_read_input_per_1m)
        .or(price.cache_read_input_per_1m);
    let cache_creation_price = selected_tier
        .and_then(|tier| tier.cache_creation_input_per_1m)
        .or(price.cache_creation_input_per_1m);

    let input_cost = component_cost(billable.input_tokens, Some(input_price));
    let output_cost = component_cost(billable.output_tokens, Some(output_price));
    let cache_read_cost = component_cost(billable.cache_read_input_tokens, cache_read_price);
    let cache_creation_cost =
        component_cost(billable.cache_creation_input_tokens, cache_creation_price);
    let any_missing_price = [
        (billable.input_tokens, input_cost),
        (billable.output_tokens, output_cost),
        (billable.cache_read_input_tokens, cache_read_cost),
        (billable.cache_creation_input_tokens, cache_creation_cost),
    ]
    .into_iter()
    .any(|(tokens, cost)| tokens > 0 && cost.is_none());
    let any_priced_component = [
        (billable.input_tokens, input_cost),
        (billable.output_tokens, output_cost),
        (billable.cache_read_input_tokens, cache_read_cost),
        (billable.cache_creation_input_tokens, cache_creation_cost),
    ]
    .into_iter()
    .any(|(tokens, cost)| tokens > 0 && cost.is_some());
    let has_billable_tokens = billable.input_tokens > 0
        || billable.output_tokens > 0
        || billable.cache_read_input_tokens > 0
        || billable.cache_creation_input_tokens > 0;
    let base_total = [
        input_cost,
        output_cost,
        cache_read_cost,
        cache_creation_cost,
    ]
    .into_iter()
    .flatten()
    .fold(UsdAmount::ZERO, UsdAmount::saturating_add);
    let adjusted_total =
        (!has_billable_tokens || any_priced_component).then(|| adjustments.apply(base_total));

    CostBreakdown {
        input_cost_usd: (billable.input_tokens > 0)
            .then(|| input_cost.map(UsdAmount::format_usd))
            .flatten(),
        output_cost_usd: (billable.output_tokens > 0)
            .then(|| output_cost.map(UsdAmount::format_usd))
            .flatten(),
        cache_read_cost_usd: (billable.cache_read_input_tokens > 0)
            .then(|| cache_read_cost.map(UsdAmount::format_usd))
            .flatten(),
        cache_creation_cost_usd: (billable.cache_creation_input_tokens > 0)
            .then(|| cache_creation_cost.map(UsdAmount::format_usd))
            .flatten(),
        service_tier_multiplier: adjustments
            .service_tier_multiplier
            .map(PriceMultiplier::format),
        provider_cost_multiplier: adjustments.provider_multiplier.map(PriceMultiplier::format),
        total_cost_usd: adjusted_total.map(UsdAmount::format_usd),
        confidence: if any_missing_price {
            CostConfidence::Partial
        } else {
            price.confidence
        },
        pricing_source: Some(price.source.clone()),
        pricing_provider: Some(price.provider.clone()),
        pricing_generation: price.source_generation.clone(),
        effective_pricing_revision: None,
        selected_tier: selected_tier.map(|tier| SelectedPriceTier {
            tier_type: "context".to_string(),
            threshold_tokens: tier.threshold_tokens,
            matched_input_tokens,
        }),
        total_cost_femto_usd: adjusted_total.map(UsdAmount::femto_usd),
    }
}

pub fn format_cost_display(total_cost_usd: Option<&str>) -> String {
    total_cost_usd
        .map(|value| format!("${value}"))
        .unwrap_or_else(|| "-".to_string())
}

pub fn format_cost_with_confidence(
    total_cost_usd: Option<&str>,
    confidence: CostConfidence,
) -> String {
    let total = format_cost_display(total_cost_usd);
    if total == "-" {
        return "- (unknown)".to_string();
    }
    match confidence {
        CostConfidence::Unknown => format!("{total} (unknown)"),
        CostConfidence::Partial => format!("{total} (partial)"),
        CostConfidence::Estimated => format!("{total} (estimated)"),
        CostConfidence::Exact => format!("{total} (exact)"),
    }
}

fn component_cost(tokens: i64, price: Option<UsdAmount>) -> Option<UsdAmount> {
    if tokens <= 0 {
        Some(UsdAmount::ZERO)
    } else {
        price.map(|price| UsdAmount::cost_for_tokens_per_million(tokens, price))
    }
}

fn validate_usd_decimal(field: &str, value: &str) -> Result<(), String> {
    if UsdAmount::from_decimal_str(value).is_some() {
        return Ok(());
    }
    Err(format!("{field} must be a non-negative USD decimal string"))
}

fn validate_optional_usd_decimal(field: &str, value: &Option<String>) -> Result<(), String> {
    if let Some(value) = value {
        validate_usd_decimal(field, value)?;
    }
    Ok(())
}

fn parse_optional_usd(value: Option<&str>) -> Option<Option<UsdAmount>> {
    match value {
        Some(value) => UsdAmount::from_decimal_str(value).map(Some),
        None => Some(None),
    }
}

fn validate_model_price_tiers(tiers: &[ModelPriceTier]) -> Result<(), String> {
    let mut thresholds = BTreeSet::new();
    for tier in tiers {
        if tier.threshold_tokens == 0 {
            return Err("context tier threshold_tokens must be greater than zero".to_string());
        }
        if !thresholds.insert(tier.threshold_tokens) {
            return Err(format!(
                "duplicate context tier threshold {}",
                tier.threshold_tokens
            ));
        }
        if !tier.has_price_overlay() {
            return Err(format!(
                "context tier {} must overlay at least one price field",
                tier.threshold_tokens
            ));
        }
    }
    Ok(())
}

fn default_canonical_provider() -> String {
    DEFAULT_CANONICAL_PROVIDER.to_string()
}

pub fn canonical_provider_for_service(service: &str) -> String {
    canonical_provider(service).unwrap_or_else(default_canonical_provider)
}

pub fn canonical_provider(provider_or_service: &str) -> Option<String> {
    let provider = provider_or_service.trim().to_ascii_lowercase();
    if provider.is_empty() {
        return None;
    }
    Some(
        match provider.as_str() {
            "codex" | "openai" => "openai",
            "claude" | "anthropic" => "anthropic",
            "gemini" | "google" => "google",
            _ => provider.as_str(),
        }
        .to_string(),
    )
}

fn require_canonical_provider(provider: &str) -> Result<String, String> {
    canonical_provider(provider).ok_or_else(|| "pricing provider cannot be empty".to_string())
}

fn normalized_model_id(model: &str) -> Result<String, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("pricing override model id cannot be empty".to_string());
    }
    Ok(model.to_string())
}

fn normalize_model_key(model: &str) -> String {
    model.trim().to_ascii_lowercase()
}

fn model_lookup_keys(model: &str) -> Vec<String> {
    let normalized = normalize_model_key(model);
    let mut keys = vec![normalized.clone()];
    for suffix in ["-minimal", "-low", "-medium", "-high", "-xhigh"] {
        if let Some(stripped) = normalized.strip_suffix(suffix)
            && !stripped.is_empty()
        {
            keys.push(stripped.to_string());
        }
    }
    keys
}

fn build_bundled_model_price_catalog() -> ModelPriceCatalog {
    const SOURCE: &str = "bundled-openai-codex-seed";
    const ROWS: &[(&str, &str, &str, &str, &str, &str)] = &[
        ("gpt-5.5", "GPT-5.5", "5", "30", "0.50", "0"),
        ("gpt-5.4", "GPT-5.4", "2.50", "15", "0.25", "0"),
        ("gpt-5.4-mini", "GPT-5.4 Mini", "0.75", "4.50", "0.075", "0"),
        ("gpt-5.4-nano", "GPT-5.4 Nano", "0.20", "1.25", "0.02", "0"),
        ("gpt-5.3-codex", "GPT-5.3 Codex", "1.75", "14", "0.175", "0"),
        ("gpt-5.2", "GPT-5.2", "1.75", "14", "0.175", "0"),
        ("gpt-5.2-codex", "GPT-5.2 Codex", "1.75", "14", "0.175", "0"),
        ("gpt-5.1", "GPT-5.1", "1.25", "10", "0.125", "0"),
        ("gpt-5.1-codex", "GPT-5.1 Codex", "1.25", "10", "0.125", "0"),
        (
            "gpt-5.1-codex-max",
            "GPT-5.1 Codex Max",
            "1.25",
            "10",
            "0.125",
            "0",
        ),
        ("gpt-5", "GPT-5", "1.25", "10", "0.125", "0"),
        ("gpt-5-codex", "GPT-5 Codex", "1.25", "10", "0.125", "0"),
        (
            "gpt-5-codex-mini",
            "GPT-5 Codex Mini",
            "1.25",
            "10",
            "0.125",
            "0",
        ),
        ("gpt-5-mini", "GPT-5 Mini", "0.25", "2", "0.025", "0"),
        ("gpt-5-nano", "GPT-5 Nano", "0.05", "0.40", "0.005", "0"),
        ("codex-mini", "Codex Mini", "0.75", "3", "0.025", "0"),
        ("gpt-4.1", "GPT-4.1", "2", "8", "0.50", "0"),
        ("gpt-4.1-mini", "GPT-4.1 Mini", "0.40", "1.60", "0.10", "0"),
        ("gpt-4.1-nano", "GPT-4.1 Nano", "0.10", "0.40", "0.025", "0"),
        ("o3", "OpenAI o3", "2", "8", "0.50", "0"),
        ("o3-mini", "OpenAI o3-mini", "0.55", "2.20", "0.55", "0"),
        ("o3-pro", "OpenAI o3-pro", "20", "80", "0", "0"),
        ("o4-mini", "OpenAI o4-mini", "1.10", "4.40", "0.275", "0"),
        ("o1", "OpenAI o1", "15", "60", "7.50", "0"),
        ("o1-mini", "OpenAI o1-mini", "0.55", "2.20", "0.55", "0"),
    ];

    let prices = ROWS.iter().filter_map(
        |(model, display, input, output, cache_read, cache_creation)| {
            ModelPrice::from_per_million_usd(
                *model,
                Some((*display).to_string()),
                input,
                output,
                Some(cache_read),
                Some(cache_creation),
                SOURCE,
            )
        },
    );
    ModelPriceCatalog::with_prices(prices)
}

fn pow10_i128(exp: u32) -> i128 {
    let mut value = 1_i128;
    for _ in 0..exp {
        value = value.saturating_mul(10);
    }
    value
}

fn parse_decimal_usd_to_femto(value: &str) -> Option<i128> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') {
        return None;
    }
    let value = value.strip_prefix('+').unwrap_or(value);
    let (mantissa, exp10) = match value.split_once(['e', 'E']) {
        Some((mantissa, exp)) => (mantissa.trim(), exp.trim().parse::<i64>().ok()?),
        None => (value, 0),
    };

    let (whole, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    if !whole.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    if !frac.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    let mut digits = String::with_capacity(whole.len() + frac.len());
    digits.push_str(whole);
    digits.push_str(frac);
    let digits = digits.trim_start_matches('0');
    let mantissa_int = if digits.is_empty() {
        0
    } else {
        digits.parse::<i128>().ok()?
    };

    let exp_femto = exp10.saturating_sub(frac.len() as i64).saturating_add(15);
    if exp_femto >= 0 {
        return Some(mantissa_int.saturating_mul(pow10_i128(exp_femto as u32)));
    }

    let divisor = pow10_i128((-exp_femto) as u32);
    if divisor == 0 {
        return None;
    }
    let q = mantissa_int / divisor;
    let r = mantissa_int % divisor;
    if r.saturating_mul(2) >= divisor {
        Some(q.saturating_add(1))
    } else {
        Some(q)
    }
}

fn format_femto_usd(value: i128) -> String {
    let value = value.max(0);
    let whole = value / FEMTO_USD_PER_USD;
    let frac = value % FEMTO_USD_PER_USD;
    if frac == 0 {
        return whole.to_string();
    }
    let mut frac_s = format!("{frac:015}");
    while frac_s.ends_with('0') {
        frac_s.pop();
    }
    format!("{whole}.{frac_s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_precise_usd_amounts() {
        assert_eq!(
            UsdAmount::from_decimal_str("0.000001")
                .expect("amount")
                .femto_usd(),
            1_000_000_000
        );
        assert_eq!(
            UsdAmount::from_decimal_str("1e-9")
                .expect("amount")
                .format_usd(),
            "0.000000001"
        );
        assert_eq!(UsdAmount::from_decimal_str("-1"), None);
        assert_eq!(UsdAmount::from_decimal_str("abc"), None);
    }

    #[test]
    fn estimates_cache_aware_usage_cost_without_double_charging_cached_input() {
        let price = ModelPrice::from_per_million_usd(
            "test-model",
            None,
            "1",
            "2",
            Some("0.1"),
            Some("3"),
            "test",
        )
        .expect("price");
        let usage = UsageMetrics {
            input_tokens: 1_000,
            output_tokens: 500,
            cached_input_tokens: 100,
            cache_creation_input_tokens: 50,
            total_tokens: 1_500,
            ..UsageMetrics::default()
        };

        let cost = estimate_usage_cost(&usage, &price, CostAdjustments::default());

        assert_eq!(cost.input_cost_usd.as_deref(), Some("0.0009"));
        assert_eq!(cost.cache_read_cost_usd.as_deref(), Some("0.00001"));
        assert_eq!(cost.cache_creation_cost_usd.as_deref(), Some("0.00015"));
        assert_eq!(cost.output_cost_usd.as_deref(), Some("0.001"));
        assert_eq!(cost.total_cost_usd.as_deref(), Some("0.00206"));
        assert_eq!(cost.confidence, CostConfidence::Estimated);
    }

    #[test]
    fn keeps_anthropic_style_cache_tokens_outside_regular_input() {
        let usage = UsageMetrics {
            input_tokens: 10,
            output_tokens: 5,
            cache_read_input_tokens: 30,
            cache_creation_5m_input_tokens: 20,
            cache_creation_1h_input_tokens: 40,
            ..UsageMetrics::default()
        };

        let billable = BillableTokenUsage::from_usage(&usage);

        assert_eq!(billable.input_tokens, 10);
        assert_eq!(billable.cache_read_input_tokens, 30);
        assert_eq!(billable.cache_creation_input_tokens, 60);
    }

    #[test]
    fn subtracts_direct_cache_read_for_codex_style_accounting() {
        let usage = UsageMetrics {
            input_tokens: 100,
            output_tokens: 5,
            cache_read_input_tokens: 30,
            cache_creation_input_tokens: 10,
            ..UsageMetrics::default()
        };

        let billable = BillableTokenUsage::from_usage_with_accounting(
            &usage,
            CacheInputAccounting::DirectReadIncludedInInput,
        );

        assert_eq!(billable.input_tokens, 70);
        assert_eq!(billable.cache_read_input_tokens, 30);
        assert_eq!(billable.cache_creation_input_tokens, 10);
    }

    #[test]
    fn unknown_cost_is_not_zero() {
        let cost = CostBreakdown::default();

        assert_eq!(cost.confidence, CostConfidence::Unknown);
        assert_eq!(cost.display_total(), "-");
    }

    #[test]
    fn missing_required_cache_price_makes_cost_partial_and_unpriced_for_that_component() {
        let price =
            ModelPrice::from_per_million_usd("test-model", None, "1", "2", None, Some("3"), "test")
                .expect("price");
        let usage = UsageMetrics {
            input_tokens: 100,
            cached_input_tokens: 10,
            output_tokens: 20,
            ..UsageMetrics::default()
        };

        let cost = estimate_usage_cost(&usage, &price, CostAdjustments::default());

        assert_eq!(cost.confidence, CostConfidence::Partial);
        assert_eq!(cost.cache_read_cost_usd, None);
        assert_eq!(cost.total_cost_usd.as_deref(), Some("0.00013"));
        assert_eq!(cost.pricing_source.as_deref(), Some("test"));
    }

    #[test]
    fn model_lookup_accepts_reasoning_suffixes() {
        let catalog = bundled_model_price_catalog();

        assert!(catalog.price_for_model("gpt-5.3-codex-high").is_some());
        assert!(catalog.price_for_model("GPT-5.1-CODEX-MAX-XHIGH").is_some());
    }

    #[test]
    fn bundled_catalog_snapshot_exposes_operator_price_rows() {
        let snapshot = bundled_model_price_catalog_snapshot();

        assert_eq!(snapshot.source, "bundled");
        assert_eq!(snapshot.model_count, snapshot.models.len());
        let gpt5 = snapshot
            .models
            .iter()
            .find(|model| model.model_id == "gpt-5")
            .expect("gpt-5 price row");
        assert_eq!(gpt5.input_per_1m_usd, "1.25");
        assert_eq!(gpt5.provider, "openai");
        assert_eq!(gpt5.output_per_1m_usd, "10");
        assert_eq!(gpt5.cache_read_input_per_1m_usd.as_deref(), Some("0.125"));
        assert_eq!(gpt5.confidence, CostConfidence::Estimated);
    }

    #[test]
    fn model_price_view_matches_reasoning_suffixed_model() {
        let snapshot = bundled_model_price_catalog_snapshot();
        let row = snapshot
            .models
            .iter()
            .find(|model| model.model_id == "gpt-5.3-codex")
            .expect("gpt-5.3-codex price row");

        assert!(row.matches_model("GPT-5.3-CODEX-HIGH"));
    }

    #[test]
    fn catalog_snapshot_prioritizes_observed_models_then_fills_catalog_order() {
        let snapshot = bundled_model_price_catalog_snapshot();
        let rows = snapshot.prioritized_models(["gpt-5.4-mini", "unknown-model"], 3);

        assert_eq!(rows[0].model_id, "gpt-5.4-mini");
        assert_eq!(rows.len(), 3);
        assert!(rows[1..].iter().all(|row| row.model_id != "gpt-5.4-mini"));
    }

    #[test]
    fn catalog_snapshot_prioritization_is_provider_aware() {
        let catalog = ModelPriceCatalog::with_prices([
            effective_test_price("openai", "same-model", "1", "2", "openai"),
            effective_test_price("routing-run", "same-model", "9", "18", "routing"),
        ]);
        let snapshot = catalog.snapshot("test");

        let routing = snapshot.prioritized_models_for_provider("routing-run", ["same-model"], 1);
        assert_eq!(routing.len(), 1);
        assert_eq!(routing[0].provider, "routing-run");
        assert_eq!(routing[0].input_per_1m_usd, "9");

        let codex = snapshot.prioritized_models_for_service("codex", ["same-model"], 1);
        assert_eq!(codex.len(), 1);
        assert_eq!(codex[0].provider, "openai");
    }

    #[test]
    fn parses_local_price_overrides_and_replaces_bundled_rows() {
        let text = r#"
[models.gpt-5]
display_name = "Custom GPT-5"
aliases = ["custom-gpt5"]
input_per_1m_usd = "9"
output_per_1m_usd = "18"
cache_read_input_per_1m_usd = "0.9"
cache_creation_input_per_1m_usd = "0.1"
confidence = "exact"

[models.custom-relay]
input_per_1m_usd = "0.5"
output_per_1m_usd = "1.5"
"#;
        let document = parse_model_price_overrides_document(text).expect("overrides");
        let normalized = document.normalized().expect("normalized legacy overrides");
        assert_eq!(normalized.version, MODEL_PRICE_OVERRIDES_SCHEMA_VERSION);
        assert!(normalized.models.is_empty());
        assert!(normalized.model("openai", "gpt-5").is_some());
        let migrated_toml = toml::to_string_pretty(&normalized).expect("serialize migrated TOML");
        let reparsed = parse_model_price_overrides_document(&migrated_toml)
            .expect("reparse migrated TOML")
            .normalized()
            .expect("renormalize migrated TOML");
        assert_eq!(reparsed, normalized);
        let overrides = document.into_prices("local-test").expect("overrides");
        let mut catalog = bundled_model_price_catalog().clone();
        for price in overrides {
            catalog.insert(price);
        }

        let gpt5 = catalog
            .price_for_model("custom-gpt5")
            .expect("override alias");
        assert_eq!(gpt5.display_name.as_deref(), Some("Custom GPT-5"));
        assert_eq!(gpt5.input_per_1m.format_usd(), "9");
        assert_eq!(gpt5.output_per_1m.format_usd(), "18");
        assert_eq!(gpt5.confidence, CostConfidence::Exact);

        let custom = catalog
            .price_for_model("custom-relay")
            .expect("new override model");
        assert_eq!(custom.input_per_1m.format_usd(), "0.5");
        assert_eq!(custom.source, "local-test");
    }

    #[test]
    fn v2_writer_output_remains_readable_by_head_v1_shape() {
        #[derive(Debug, serde::Deserialize)]
        struct HeadV1PriceOverridesDocument {
            #[serde(default)]
            models: BTreeMap<String, HeadV1ModelPriceOverride>,
        }

        #[derive(Debug, serde::Deserialize)]
        struct HeadV1ModelPriceOverride {
            input_per_1m_usd: String,
            output_per_1m_usd: String,
        }

        let document = parse_model_price_overrides_document(
            r#"
version = 2

[providers.openai.models.gpt-custom]
input_per_1m_usd = "1.25"
output_per_1m_usd = "10"

[[providers.openai.models.gpt-custom.tiers]]
threshold_tokens = 272000
input_per_1m_usd = "2.5"

[providers.anthropic.models.claude-custom]
input_per_1m_usd = "3"
output_per_1m_usd = "15"
"#,
        )
        .expect("v2 provider overrides");

        let rendered = render_model_price_overrides_document(&document).expect("render v2 TOML");
        let head_v1: HeadV1PriceOverridesDocument =
            toml::from_str(&rendered).expect("HEAD v1 shape parses new writer output");
        let openai = head_v1.models.get("gpt-custom").expect("OpenAI mirror");
        assert_eq!(openai.input_per_1m_usd, "1.25");
        assert_eq!(openai.output_per_1m_usd, "10");
        assert!(!head_v1.models.contains_key("claude-custom"));

        let normalized = parse_model_price_overrides_document(&rendered)
            .expect("current reader accepts identical legacy mirror")
            .normalized()
            .expect("normalize mirrored document");
        assert!(normalized.models.is_empty());
        assert_eq!(
            normalized
                .model("openai", "gpt-custom")
                .expect("canonical OpenAI row")
                .tiers
                .len(),
            1
        );
        assert!(normalized.model("anthropic", "claude-custom").is_some());
    }

    #[test]
    fn v2_reader_rejects_divergent_legacy_openai_mirror() {
        let err = parse_model_price_overrides_document(
            r#"
version = 2

[providers.openai.models.gpt-custom]
input_per_1m_usd = "1"
output_per_1m_usd = "2"

[models.gpt-custom]
input_per_1m_usd = "9"
output_per_1m_usd = "2"
"#,
        )
        .expect_err("divergent provider/legacy duplicate must fail");

        assert!(err.contains("conflicts"));
    }

    #[test]
    fn local_price_override_document_rejects_conflicting_aliases() {
        let text = r#"
[models.gpt-5]
input_per_1m_usd = "1"
output_per_1m_usd = "2"
aliases = ["custom"]

[models.gpt-4]
input_per_1m_usd = "3"
output_per_1m_usd = "4"
aliases = ["CUSTOM"]
"#;
        let err = parse_model_price_overrides_document(text).expect_err("should fail");
        assert!(err.contains("used by both"));
    }

    #[test]
    fn summary_tracks_partial_confidence() {
        let mut summary = CostSummary::default();
        let known = CostBreakdown {
            total_cost_usd: Some("0.001".to_string()),
            confidence: CostConfidence::Estimated,
            total_cost_femto_usd: Some(1_000_000_000_000),
            ..CostBreakdown::unknown()
        };

        summary.record_usage_cost(&known);
        summary.record_usage_cost(&CostBreakdown::unknown());

        assert_eq!(summary.total_cost_usd.as_deref(), Some("0.001"));
        assert_eq!(summary.confidence, CostConfidence::Partial);
        assert_eq!(summary.priced_requests, 1);
        assert_eq!(summary.unpriced_requests, 1);
        assert_eq!(summary.partial_requests, 0);
    }

    #[test]
    fn summary_never_promotes_priced_partial_cost_to_estimated() {
        let partial = CostBreakdown {
            total_cost_usd: Some("0.001".to_string()),
            confidence: CostConfidence::Partial,
            total_cost_femto_usd: Some(1_000_000_000_000),
            ..CostBreakdown::unknown()
        };
        let mut left = CostSummary::default();
        left.record_usage_cost(&partial);
        assert_eq!(left.confidence, CostConfidence::Partial);
        assert_eq!(left.partial_requests, 1);

        let mut combined = CostSummary::default();
        combined.add_assign(&left);
        assert_eq!(combined.confidence, CostConfidence::Partial);
        assert_eq!(combined.partial_requests, 1);
    }

    #[test]
    fn basellm_snapshot_imports_per_million_cost_rows() {
        let text = r#"
{
  "openai": {
    "models": {
      "gpt-test": {
        "name": "GPT Test",
        "aliases": ["relay-gpt-test"],
        "cost": {
          "input": "1.5",
          "output": 6,
          "cache_read": "0.15",
          "cache_write": "0"
        }
      }
    }
  },
  "unknown-provider": {
    "models": {
      "ignored": {
        "cost": { "input": 1, "output": 2 }
      }
    }
  }
}
"#;

        let snapshot =
            basellm_model_price_catalog_snapshot_from_json("basellm-test", text).expect("snapshot");

        assert_eq!(snapshot.source, "basellm-test");
        assert_eq!(snapshot.model_count, 2);
        let row = snapshot
            .models
            .iter()
            .find(|row| row.model_id == "gpt-test")
            .expect("gpt-test row");
        assert_eq!(row.display_name.as_deref(), Some("GPT Test"));
        assert_eq!(row.aliases, vec!["relay-gpt-test"]);
        assert_eq!(row.input_per_1m_usd, "1.5");
        assert_eq!(row.output_per_1m_usd, "6");
        assert_eq!(row.cache_read_input_per_1m_usd.as_deref(), Some("0.15"));
        assert_eq!(row.cache_creation_input_per_1m_usd.as_deref(), Some("0"));
        assert_eq!(row.source, "basellm:openai");
        assert_eq!(row.provider, "openai");
    }

    #[test]
    fn basellm_context_tiers_are_sorted_and_partial_fields_are_preserved() {
        let text = r#"
{
  "openai": {
    "models": {
      "gpt-tiered": {
        "cost": {
          "input": 5,
          "output": 30,
          "cache_read": 0.5,
          "tiers": [
            {
              "tier": { "type": "context", "size": 500000 },
              "output": 60
            },
            {
              "tier": { "type": "context", "size": 272000 },
              "input": 10,
              "cache_read": 1
            },
            {
              "tier": { "type": "batch", "size": 1 },
              "input": 1
            }
          ]
        }
      }
    }
  }
}
"#;

        let snapshot =
            basellm_model_price_catalog_snapshot_from_json("test", text).expect("snapshot");
        let row = &snapshot.models[0];
        assert_eq!(
            row.tiers
                .iter()
                .map(|tier| tier.threshold_tokens)
                .collect::<Vec<_>>(),
            vec![272_000, 500_000]
        );
        assert_eq!(row.tiers[0].input_per_1m_usd.as_deref(), Some("10"));
        assert_eq!(row.tiers[0].output_per_1m_usd, None);
    }

    #[test]
    fn basellm_duplicate_or_malformed_context_threshold_is_rejected() {
        let duplicate = r#"
{
  "openai": {
    "models": {
      "gpt-tiered": {
        "cost": {
          "input": 5,
          "output": 30,
          "tiers": [
            { "tier": { "type": "context", "size": 272000 }, "input": 10 },
            { "tier": { "type": "context", "size": 272000 }, "output": 45 }
          ]
        }
      }
    }
  }
}
"#;
        let malformed = duplicate.replace(
            "{ \"type\": \"context\", \"size\": 272000 }",
            "{ \"type\": \"context\", \"size\": 0 }",
        );

        assert!(
            basellm_model_price_catalog_snapshot_from_json("test", duplicate)
                .expect_err("duplicate threshold")
                .contains("duplicate context tier")
        );
        assert!(
            basellm_model_price_catalog_snapshot_from_json("test", &malformed)
                .expect_err("malformed threshold")
                .contains("invalid positive size")
        );
    }

    #[test]
    fn aliases_collide_only_inside_one_provider_namespace() {
        let text = r#"
version = 2

[providers.openai.models.gpt-one]
aliases = ["shared"]
input_per_1m_usd = "1"
output_per_1m_usd = "2"

[providers.routing-run.models.gpt-two]
aliases = ["shared"]
input_per_1m_usd = "3"
output_per_1m_usd = "4"
"#;

        let document = parse_model_price_overrides_document(text).expect("provider scoped aliases");
        let encoded =
            toml::to_string_pretty(&document.normalized().expect("normalized")).expect("serialize");
        let reparsed = parse_model_price_overrides_document(&encoded)
            .expect("reparse")
            .normalized()
            .expect("renormalize");

        assert!(reparsed.model("openai", "gpt-one").is_some());
        assert!(reparsed.model("routing-run", "gpt-two").is_some());
    }

    fn effective_test_price(
        provider: &str,
        model: &str,
        input: &str,
        output: &str,
        source: &str,
    ) -> ModelPrice {
        ModelPrice::from_per_million_usd_for_provider(
            provider, model, None, input, output, None, None, source,
        )
        .expect("test price")
    }

    fn applied_manual_layer(prices: Vec<ModelPrice>, raw_hash: &str) -> ManualPricingLayer {
        ManualPricingLayer {
            status: ManualPricingLayerStatus::Applied,
            content_hash: Some(raw_hash.to_string()),
            file_size: Some(1),
            modified_unix_ms: Some(1),
            error: None,
            prices,
        }
    }

    #[test]
    fn effective_catalog_merges_bundled_then_remote_then_whole_manual_rows() {
        let remote = effective_test_price("openai", "gpt-5", "2", "12", "remote")
            .with_tiers([ModelPriceTier::from_per_million_usd(
                272_000,
                Some("4"),
                None,
                None,
                None,
            )
            .expect("remote tier")])
            .expect("tiered remote price");
        let remote_only = effective_test_price("openai", "remote-only", "3", "13", "remote");
        let manual = effective_test_price("openai", "gpt-5", "9", "18", "manual");

        let snapshot = build_effective_pricing_catalog_from_layers(
            Some("remote-revision".to_string()),
            vec![remote, remote_only],
            applied_manual_layer(vec![manual], "manual-raw-hash"),
        );

        let selected = snapshot
            .catalog()
            .price_for_model("gpt-5")
            .expect("manual row");
        assert_eq!(selected.source, "manual");
        assert_eq!(selected.input_per_1m.format_usd(), "9");
        assert!(selected.cache_read_input_per_1m.is_none());
        assert!(selected.tiers.is_empty());
        assert_eq!(
            snapshot
                .catalog()
                .price_for_model("remote-only")
                .expect("remote row")
                .source,
            "remote"
        );
        assert_eq!(
            snapshot
                .catalog()
                .price_for_model("gpt-4.1")
                .expect("bundled fallback")
                .source,
            "bundled-openai-codex-seed"
        );
    }

    #[test]
    fn corrupt_manual_file_falls_back_to_bundled_plus_remote() {
        let root = std::env::temp_dir().join(format!("pricing-corrupt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("create fixture directory");
        let path = root.join("pricing_overrides.toml");
        std::fs::write(&path, b"[providers.openai.models.gpt-5\ninput = ???")
            .expect("write corrupt override");

        let manual = load_manual_pricing_layer(&path);
        let snapshot = build_effective_pricing_catalog_from_layers(
            Some("remote-revision".to_string()),
            vec![effective_test_price("openai", "gpt-5", "2", "12", "remote")],
            manual,
        );

        assert_eq!(snapshot.manual_status, ManualPricingLayerStatus::Invalid);
        assert!(snapshot.manual_error.is_some());
        assert_eq!(snapshot.manual_model_count, 0);
        assert_eq!(
            snapshot
                .catalog()
                .price_for_model("gpt-5")
                .expect("remote fallback")
                .source,
            "remote"
        );

        std::fs::remove_file(&path).expect("remove fixture file");
        std::fs::remove_dir(&root).expect("remove fixture directory");
    }

    #[test]
    fn effective_revision_is_stable_for_equivalent_normalized_content() {
        let left_manual = effective_test_price("OpenAI", "GPT-CUSTOM", "1.0", "2", "local:a")
            .with_aliases(["Relay-B", "relay-a"]);
        let right_manual = effective_test_price("openai", "gpt-custom", "1", "2.00", "local:b")
            .with_aliases(["RELAY-A", "relay-b"]);
        let left = build_effective_pricing_catalog_from_layers(
            Some("remote-content".to_string()),
            Vec::new(),
            applied_manual_layer(vec![left_manual], "raw-left"),
        );
        let right = build_effective_pricing_catalog_from_layers(
            Some("remote-content".to_string()),
            Vec::new(),
            applied_manual_layer(vec![right_manual], "raw-right"),
        );

        assert_eq!(left.revision, right.revision);
        assert_ne!(left.manual_content_hash, right.manual_content_hash);

        let changed = build_effective_pricing_catalog_from_layers(
            Some("remote-content".to_string()),
            Vec::new(),
            applied_manual_layer(
                vec![effective_test_price(
                    "openai",
                    "gpt-custom",
                    "1.01",
                    "2",
                    "local:b",
                )],
                "raw-right",
            ),
        );
        assert_ne!(left.revision, changed.revision);
    }

    #[test]
    fn concurrent_snapshot_swap_never_mixes_cost_and_effective_revision() {
        use std::sync::Barrier;
        use std::thread;

        let initial = build_effective_pricing_catalog_from_layers(
            Some("remote-a".to_string()),
            vec![effective_test_price(
                "openai",
                "swap-model",
                "1",
                "1",
                "remote-a",
            )],
            ManualPricingLayer::missing(),
        );
        let replacement = build_effective_pricing_catalog_from_layers(
            Some("remote-b".to_string()),
            vec![effective_test_price(
                "openai",
                "swap-model",
                "2",
                "2",
                "remote-b",
            )],
            ManualPricingLayer::missing(),
        );
        let initial_revision = initial.revision.clone();
        let replacement_revision = replacement.revision.clone();
        let store = Arc::new(EffectivePricingCatalogStore::new(initial));
        let barrier = Arc::new(Barrier::new(9));
        let usage = UsageMetrics {
            input_tokens: 1_000_000,
            ..UsageMetrics::default()
        };
        let workers = (0..8)
            .map(|_| {
                let store = store.clone();
                let barrier = barrier.clone();
                let usage = usage.clone();
                let initial_revision = initial_revision.clone();
                let replacement_revision = replacement_revision.clone();
                thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..500 {
                        let snapshot = store.load();
                        let cost = snapshot.estimate_request_cost(
                            Some("swap-model"),
                            Some(&usage),
                            CostAdjustments::default(),
                            CacheInputAccounting::default(),
                        );
                        let revision = cost
                            .effective_pricing_revision
                            .as_deref()
                            .expect("captured effective revision");
                        match revision {
                            value if value == initial_revision => {
                                assert_eq!(cost.total_cost_usd.as_deref(), Some("1"));
                                assert_eq!(cost.pricing_source.as_deref(), Some("remote-a"));
                            }
                            value if value == replacement_revision => {
                                assert_eq!(cost.total_cost_usd.as_deref(), Some("2"));
                                assert_eq!(cost.pricing_source.as_deref(), Some("remote-b"));
                            }
                            other => panic!("unexpected effective revision {other}"),
                        }
                    }
                })
            })
            .collect::<Vec<_>>();

        barrier.wait();
        store.install(replacement);
        for worker in workers {
            worker.join().expect("worker completed");
        }
    }
}
