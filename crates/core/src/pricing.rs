use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::usage::UsageMetrics;

const FEMTO_USD_PER_USD: i128 = 1_000_000_000_000_000;
const TOKENS_PER_MILLION: i128 = 1_000_000;
const MULTIPLIER_SCALE: i128 = 1_000_000;

fn u64_is_zero(value: &u64) -> bool {
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

    pub fn saturating_add(self, other: Self) -> Self {
        Self::from_femto_usd(self.femto_usd.saturating_add(other.femto_usd))
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
        let numerator = amount.femto_usd.saturating_mul(self.scaled);
        let q = numerator / MULTIPLIER_SCALE;
        let r = (numerator % MULTIPLIER_SCALE).abs();
        let rounded = if r.saturating_mul(2) >= MULTIPLIER_SCALE {
            q.saturating_add(1)
        } else {
            q
        };
        UsdAmount::from_femto_usd(rounded)
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
        let mut out = amount;
        if let Some(multiplier) = self.service_tier_multiplier {
            out = multiplier.apply(out);
        }
        if let Some(multiplier) = self.provider_multiplier {
            out = multiplier.apply(out);
        }
        out
    }
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
    #[serde(skip)]
    total_cost_femto_usd: Option<i128>,
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
            total_cost_femto_usd: 0,
        }
    }
}

impl CostSummary {
    pub fn is_empty(&self) -> bool {
        self.priced_requests == 0 && self.unpriced_requests == 0 && self.total_cost_usd.is_none()
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
        self.confidence = if self.unpriced_requests > 0 {
            CostConfidence::Partial
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
        let cached_input_tokens = usage.cached_input_tokens.max(0);
        let cache_read_input_tokens = cached_input_tokens
            .saturating_add(usage.cache_read_input_tokens.max(0))
            .max(0);

        Self {
            input_tokens: usage
                .input_tokens
                .max(0)
                .saturating_sub(cached_input_tokens),
            output_tokens: usage.output_tokens.max(0),
            cache_read_input_tokens,
            cache_creation_input_tokens: usage.cache_creation_tokens_total().max(0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPrice {
    pub model_id: String,
    pub display_name: Option<String>,
    pub aliases: Vec<String>,
    pub input_per_1m: UsdAmount,
    pub output_per_1m: UsdAmount,
    pub cache_read_input_per_1m: Option<UsdAmount>,
    pub cache_creation_input_per_1m: Option<UsdAmount>,
    pub source: String,
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
        Some(Self {
            model_id: model_id.into(),
            display_name,
            aliases: Vec::new(),
            input_per_1m: UsdAmount::from_decimal_str(input)?,
            output_per_1m: UsdAmount::from_decimal_str(output)?,
            cache_read_input_per_1m: cache_read.and_then(UsdAmount::from_decimal_str),
            cache_creation_input_per_1m: cache_creation.and_then(UsdAmount::from_decimal_str),
            source: source.into(),
            confidence: CostConfidence::Estimated,
        })
    }

    pub fn with_aliases(mut self, aliases: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.aliases = aliases.into_iter().map(Into::into).collect();
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModelPriceCatalog {
    entries: BTreeMap<String, ModelPrice>,
    aliases: BTreeMap<String, String>,
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

    pub fn insert(&mut self, price: ModelPrice) {
        let key = normalize_model_key(&price.model_id);
        for alias in &price.aliases {
            self.aliases.insert(normalize_model_key(alias), key.clone());
        }
        self.entries.insert(key, price);
    }

    pub fn price_for_model(&self, model: &str) -> Option<&ModelPrice> {
        for key in model_lookup_keys(model) {
            if let Some(price) = self.entries.get(&key) {
                return Some(price);
            }
            if let Some(target) = self.aliases.get(&key)
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
        let Some(price) = self.price_for_model(model) else {
            return CostBreakdown::unknown();
        };
        estimate_usage_cost(usage, price, adjustments)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub fn bundled_model_price_catalog() -> &'static ModelPriceCatalog {
    static CATALOG: OnceLock<ModelPriceCatalog> = OnceLock::new();
    CATALOG.get_or_init(build_bundled_model_price_catalog)
}

pub fn estimate_request_cost_from_bundled_catalog(
    model: Option<&str>,
    usage: Option<&UsageMetrics>,
    adjustments: CostAdjustments,
) -> CostBreakdown {
    let (Some(model), Some(usage)) = (model, usage) else {
        return CostBreakdown::unknown();
    };
    bundled_model_price_catalog().estimate_usage_cost(model, usage, adjustments)
}

pub fn estimate_usage_cost(
    usage: &UsageMetrics,
    price: &ModelPrice,
    adjustments: CostAdjustments,
) -> CostBreakdown {
    let billable = BillableTokenUsage::from_usage(usage);

    let Some(cache_read_price) = required_price(
        billable.cache_read_input_tokens,
        price.cache_read_input_per_1m,
    ) else {
        return unknown_with_source(&price.source);
    };
    let Some(cache_creation_price) = required_price(
        billable.cache_creation_input_tokens,
        price.cache_creation_input_per_1m,
    ) else {
        return unknown_with_source(&price.source);
    };

    let input_cost =
        UsdAmount::cost_for_tokens_per_million(billable.input_tokens, price.input_per_1m);
    let output_cost =
        UsdAmount::cost_for_tokens_per_million(billable.output_tokens, price.output_per_1m);
    let cache_read_cost =
        UsdAmount::cost_for_tokens_per_million(billable.cache_read_input_tokens, cache_read_price);
    let cache_creation_cost = UsdAmount::cost_for_tokens_per_million(
        billable.cache_creation_input_tokens,
        cache_creation_price,
    );
    let base_total = input_cost
        .saturating_add(output_cost)
        .saturating_add(cache_read_cost)
        .saturating_add(cache_creation_cost);
    let adjusted_total = adjustments.apply(base_total);

    CostBreakdown {
        input_cost_usd: (billable.input_tokens > 0).then(|| input_cost.format_usd()),
        output_cost_usd: (billable.output_tokens > 0).then(|| output_cost.format_usd()),
        cache_read_cost_usd: (billable.cache_read_input_tokens > 0)
            .then(|| cache_read_cost.format_usd()),
        cache_creation_cost_usd: (billable.cache_creation_input_tokens > 0)
            .then(|| cache_creation_cost.format_usd()),
        service_tier_multiplier: adjustments
            .service_tier_multiplier
            .map(PriceMultiplier::format),
        provider_cost_multiplier: adjustments.provider_multiplier.map(PriceMultiplier::format),
        total_cost_usd: Some(adjusted_total.format_usd()),
        confidence: price.confidence,
        pricing_source: Some(price.source.clone()),
        total_cost_femto_usd: Some(adjusted_total.femto_usd()),
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

fn required_price(tokens: i64, price: Option<UsdAmount>) -> Option<UsdAmount> {
    if tokens <= 0 {
        Some(UsdAmount::ZERO)
    } else {
        price
    }
}

fn unknown_with_source(source: &str) -> CostBreakdown {
    CostBreakdown {
        pricing_source: Some(source.to_string()),
        ..CostBreakdown::unknown()
    }
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
    fn unknown_cost_is_not_zero() {
        let cost = CostBreakdown::default();

        assert_eq!(cost.confidence, CostConfidence::Unknown);
        assert_eq!(cost.display_total(), "-");
    }

    #[test]
    fn missing_required_cache_price_makes_cost_unknown() {
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

        assert_eq!(cost.confidence, CostConfidence::Unknown);
        assert_eq!(cost.total_cost_usd, None);
        assert_eq!(cost.pricing_source.as_deref(), Some("test"));
    }

    #[test]
    fn model_lookup_accepts_reasoning_suffixes() {
        let catalog = bundled_model_price_catalog();

        assert!(catalog.price_for_model("gpt-5.3-codex-high").is_some());
        assert!(catalog.price_for_model("GPT-5.1-CODEX-MAX-XHIGH").is_some());
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
    }
}
