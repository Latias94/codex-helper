//! Remote quota pool identity, typed quantities, and bounded observations.
//!
//! This module deliberately contains no credential-bearing state.  Credentials may be
//! supplied to [`QuotaObservationContext`] while resolving a pool, but the resulting
//! identity never retains credential material. External operator projections
//! replace its key with an opaque token before crossing a process boundary.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::balance::{BalanceSnapshotStatus, ProviderBalanceSnapshot};
use crate::runtime_identity::ProviderEndpointKey;

pub const QUOTA_CHECKPOINT_SCHEMA_VERSION: u32 = 2;
pub const DEFAULT_MAX_SAMPLES_PER_POOL: usize = 512;
pub const DEFAULT_SAMPLE_RETENTION_MS: u64 = 2 * 60 * 60 * 1_000;
pub const DEFAULT_MAX_POOLS: usize = 128;
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;

fn is_false(value: &bool) -> bool {
    !*value
}

/// The unit in which a provider reports a quota counter.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaUnit {
    /// A provider-specific integer/fixed-point unit.  It cannot be reconciled to USD.
    #[default]
    Raw,
    /// US dollars represented by the quantity's fixed-point scale.
    Usd,
    /// Token count, when a provider exposes a token quota rather than money.
    Tokens,
    /// Unit was not advertised by the provider.
    Unknown,
}

impl QuotaUnit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Usd => "usd",
            Self::Tokens => "tokens",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_name(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "usd" | "dollar" | "dollars" => Self::Usd,
            "token" | "tokens" => Self::Tokens,
            "raw" | "quota" | "credits" | "credit" => Self::Raw,
            _ => Self::Unknown,
        }
    }
}

/// Which counter a quota quantity represents.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaCounterKind {
    #[default]
    Used,
    Remaining,
    Limit,
    DirectTotal,
}

impl QuotaCounterKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Used => "used",
            Self::Remaining => "remaining",
            Self::Limit => "limit",
            Self::DirectTotal => "direct_total",
        }
    }
}

/// Provider-declared shape of the quota accounting window.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindowKind {
    /// A civil calendar day. Only this variant permits user-facing "today" wording.
    CalendarDay,
    /// A duration measured backwards from the observation/reset boundary, such as rolling 24h.
    Rolling,
    /// A provider-defined fixed subscription or rate-limit window.
    Custom,
    /// A civil calendar month or provider-declared monthly subscription period.
    Monthly,
    /// A wallet/counter with no recurring reset.
    Resetless,
    #[default]
    Unknown,
}

/// Evidence for how a quota reset boundary is known.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaResetKind {
    /// The provider returned an absolute reset timestamp.
    ExplicitTimestamp,
    /// A configured IANA timezone supplies the civil-day boundary.
    ConfiguredCalendarBoundary,
    /// The provider contract explicitly has no recurring reset.
    NoReset,
    #[default]
    Unknown,
}

/// Typed window/reset semantics consumed by analytics without reparsing provider labels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(default)]
pub struct QuotaWindowSemantics {
    pub kind: QuotaWindowKind,
    pub reset: QuotaResetKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rolling_duration_ms: Option<u64>,
}

impl QuotaWindowSemantics {
    pub fn from_provider_hint(
        period: Option<&str>,
        reset_at_ms: Option<u64>,
        reset_timezone: Option<&str>,
        window_start_ms: Option<u64>,
        window_end_ms: Option<u64>,
    ) -> Self {
        let period = period.map(str::trim).filter(|value| !value.is_empty());
        let normalized = period.map(str::to_ascii_lowercase);
        let kind = match normalized.as_deref() {
            Some("daily" | "day" | "calendar_day") => QuotaWindowKind::CalendarDay,
            Some("monthly" | "month" | "calendar_month") => QuotaWindowKind::Monthly,
            Some("rolling" | "rolling_24h" | "24h" | "1d") => QuotaWindowKind::Rolling,
            Some("wallet" | "paygo" | "resetless" | "no_reset") => QuotaWindowKind::Resetless,
            Some(value) if value.starts_with("rolling:") || value.starts_with("rate_limit:") => {
                QuotaWindowKind::Rolling
            }
            Some("weekly" | "week" | "subscription" | "custom") => QuotaWindowKind::Custom,
            Some(_) => QuotaWindowKind::Unknown,
            None if window_start_ms.is_some() || window_end_ms.is_some() => QuotaWindowKind::Custom,
            None => QuotaWindowKind::Unknown,
        };
        let reset_timezone = reset_timezone
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let reset = if reset_at_ms.is_some() {
            QuotaResetKind::ExplicitTimestamp
        } else if kind == QuotaWindowKind::CalendarDay && reset_timezone.is_some() {
            QuotaResetKind::ConfiguredCalendarBoundary
        } else if kind == QuotaWindowKind::Resetless {
            QuotaResetKind::NoReset
        } else {
            QuotaResetKind::Unknown
        };
        let rolling_duration_ms = (kind == QuotaWindowKind::Rolling)
            .then(|| window_end_ms?.checked_sub(window_start_ms?))
            .flatten();
        Self {
            kind,
            reset,
            reset_timezone,
            rolling_duration_ms,
        }
    }

    pub fn allows_today_label(&self) -> bool {
        self.kind == QuotaWindowKind::CalendarDay
    }

    pub fn allows_midnight_label(&self) -> bool {
        self.kind == QuotaWindowKind::CalendarDay
            && matches!(
                self.reset,
                QuotaResetKind::ExplicitTimestamp | QuotaResetKind::ConfiguredCalendarBoundary
            )
    }
}

/// Resolves the next configured civil-time reset using a fixed UTC offset.
/// Provider-returned absolute reset timestamps should always take precedence.
pub fn next_configured_reset_at_ms(now_ms: u64, utc_offset: &str, reset_time: &str) -> Option<u64> {
    let offset_ms = parse_utc_offset_ms(utc_offset)?;
    let reset_ms = parse_hh_mm_ms(reset_time)?;
    let local_ms = i128::from(now_ms) + offset_ms;
    let local_day_start = div_floor(local_ms, i128::from(DAY_MS)) * i128::from(DAY_MS);
    let mut reset_local = local_day_start + i128::from(reset_ms);
    if reset_local <= local_ms {
        reset_local += i128::from(DAY_MS);
    }
    u64::try_from(reset_local - offset_ms).ok()
}

fn parse_hh_mm_ms(value: &str) -> Option<u64> {
    let (hour, minute) = value.trim().split_once(':')?;
    let hour = hour.parse::<u64>().ok()?;
    let minute = minute.parse::<u64>().ok()?;
    if hour >= 24 || minute >= 60 {
        return None;
    }
    Some(
        hour.saturating_mul(HOUR_MS)
            .saturating_add(minute.saturating_mul(MINUTE_MS)),
    )
}

fn parse_utc_offset_ms(value: &str) -> Option<i128> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("z") || value == "+00:00" || value == "-00:00" {
        return Some(0);
    }
    let sign = match value.as_bytes().first().copied()? {
        b'+' => 1_i128,
        b'-' => -1_i128,
        _ => return None,
    };
    let (hour, minute) = value[1..].split_once(':')?;
    let hour = hour.parse::<i128>().ok()?;
    let minute = minute.parse::<i128>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(sign * (hour * i128::from(HOUR_MS) + minute * i128::from(MINUTE_MS)))
}

fn div_floor(dividend: i128, divisor: i128) -> i128 {
    let quotient = dividend / divisor;
    let remainder = dividend % divisor;
    if remainder != 0 && ((remainder > 0) != (divisor > 0)) {
        quotient - 1
    } else {
        quotient
    }
}

/// Sampling cadence and absolute validity boundaries for one observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(default)]
pub struct QuotaSamplingSemantics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_interval_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fresh_until_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuity_deadline_ms: Option<u64>,
}

impl QuotaSamplingSemantics {
    pub fn is_fresh_at(&self, now_ms: u64) -> Option<bool> {
        self.fresh_until_ms.map(|deadline| now_ms <= deadline)
    }
}

/// Scope of a provider quota.  The string payload is intentionally descriptive rather than
/// a credential or account identifier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum QuotaScope {
    Account,
    ApiKey,
    Subscription,
    Organization,
    Endpoint,
    Custom(String),
    #[default]
    Unknown,
}

impl QuotaScope {
    pub fn as_key(&self) -> String {
        match self {
            Self::Account => "account".to_string(),
            Self::ApiKey => "api_key".to_string(),
            Self::Subscription => "subscription".to_string(),
            Self::Organization => "organization".to_string(),
            Self::Endpoint => "endpoint".to_string(),
            Self::Custom(value) => sanitize_component(value),
            Self::Unknown => "unknown".to_string(),
        }
    }
}

/// Capability flags advertised by an adapter.  A missing flag means "not known", not false;
/// the bools therefore remain optional in the wire shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct QuotaCapabilities {
    pub used: bool,
    pub remaining: bool,
    pub direct_total: bool,
    pub limit: bool,
    pub reset: bool,
    pub window: bool,
    pub conversion: bool,
    pub cumulative: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub unlimited: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub raw_unit: bool,
}

impl QuotaCapabilities {
    pub fn any(&self) -> bool {
        self.used
            || self.remaining
            || self.direct_total
            || self.limit
            || self.reset
            || self.window
            || self.conversion
            || self.cumulative
            || self.unlimited
            || self.raw_unit
    }
}

/// Evidence used to identify a shared remote quota pool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum IdentityEvidence {
    RemoteQuotaOwnerId,
    /// A stable subject/token/user ID which has not been proven to own the counter.
    RemoteStableId,
    ExplicitPoolId,
    CredentialFingerprint,
    EndpointOrigin,
    #[default]
    Unknown,
}

/// Adapter assertion about what a remote stable ID identifies.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum RemoteIdentityProof {
    QuotaOwner,
    StableSubject,
    #[default]
    Unverified,
}

/// Confidence controls whether multiple endpoint views may be summed.
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum IdentityConfidence {
    High,
    Medium,
    Low,
    #[default]
    Unknown,
}

impl IdentityConfidence {
    pub fn aggregation_eligible(self) -> bool {
        matches!(self, Self::High | Self::Medium)
    }
}

/// A stable identity for a quota pool that excludes credentials.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(default)]
pub struct PoolIdentity {
    pub key: String,
    pub origin: String,
    pub scope: QuotaScope,
    pub revision: u64,
    pub evidence: IdentityEvidence,
    pub confidence: IdentityConfidence,
    #[serde(skip_serializing_if = "is_false")]
    pub aggregation_eligible: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub conflicting_evidence: bool,
}

impl Default for PoolIdentity {
    fn default() -> Self {
        Self {
            key: "unknown".to_string(),
            origin: String::new(),
            scope: QuotaScope::Unknown,
            revision: 0,
            evidence: IdentityEvidence::Unknown,
            confidence: IdentityConfidence::Unknown,
            aggregation_eligible: false,
            conflicting_evidence: false,
        }
    }
}

impl PoolIdentity {
    /// Resolve identity according to the plan's evidence order.  `credential` and
    /// `install_key` are transient inputs and are never retained in this value.
    pub fn resolve(
        origin: impl AsRef<str>,
        scope: QuotaScope,
        remote_stable_id: Option<&str>,
        explicit_pool_id: Option<&str>,
        credential: Option<&[u8]>,
        install_key: Option<&[u8]>,
        revision: u64,
    ) -> Self {
        Self::resolve_with_proof(
            origin,
            scope,
            remote_stable_id,
            RemoteIdentityProof::QuotaOwner,
            explicit_pool_id,
            credential,
            install_key,
            revision,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resolve_with_proof(
        origin: impl AsRef<str>,
        scope: QuotaScope,
        remote_stable_id: Option<&str>,
        remote_proof: RemoteIdentityProof,
        explicit_pool_id: Option<&str>,
        credential: Option<&[u8]>,
        install_key: Option<&[u8]>,
        revision: u64,
        conflicting_evidence: bool,
    ) -> Self {
        let origin = normalize_issuer_authority(origin.as_ref());
        let scope_key = scope.as_key();
        if remote_proof == RemoteIdentityProof::QuotaOwner
            && let Some(id) = non_empty(remote_stable_id)
            && let Some(install_key) = install_key.filter(|key| !key.is_empty())
        {
            let Some(id) = opaque_identity_digest(
                install_key,
                b"remote-quota-owner",
                &origin,
                &scope_key,
                id.as_bytes(),
            ) else {
                return Self::default();
            };
            let confidence = if conflicting_evidence {
                IdentityConfidence::Low
            } else {
                IdentityConfidence::High
            };
            return Self {
                key: format!("remote:{origin}:{scope_key}:{id}"),
                origin,
                scope,
                revision,
                evidence: IdentityEvidence::RemoteQuotaOwnerId,
                confidence,
                aggregation_eligible: !conflicting_evidence,
                conflicting_evidence,
            };
        }
        if let Some(id) = non_empty(explicit_pool_id) {
            let id = sanitize_component(id);
            return Self {
                key: format!("explicit:{origin}:{scope_key}:{id}"),
                origin,
                scope,
                revision,
                evidence: IdentityEvidence::ExplicitPoolId,
                confidence: IdentityConfidence::Medium,
                aggregation_eligible: !conflicting_evidence,
                conflicting_evidence,
            };
        }
        if let (Some(credential), Some(install_key)) = (credential, install_key)
            && !credential.is_empty()
            && !install_key.is_empty()
        {
            let Some(digest) =
                opaque_identity_digest(install_key, b"credential", &origin, &scope_key, credential)
            else {
                return Self::default();
            };
            return Self {
                key: format!("fingerprint:{origin}:{scope_key}:{digest}"),
                origin,
                scope,
                revision,
                evidence: IdentityEvidence::CredentialFingerprint,
                confidence: IdentityConfidence::Medium,
                aggregation_eligible: !conflicting_evidence,
                conflicting_evidence,
            };
        }
        Self {
            key: format!("endpoint:{origin}:{scope_key}"),
            origin,
            scope,
            revision,
            evidence: IdentityEvidence::EndpointOrigin,
            confidence: IdentityConfidence::Low,
            aggregation_eligible: false,
            conflicting_evidence,
        }
    }

    pub fn is_ambiguous(&self) -> bool {
        !self.aggregation_eligible || self.confidence == IdentityConfidence::Unknown
    }
}

/// A fixed-point quantity tagged with its unit and conversion generation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(default)]
pub struct QuotaQuantity {
    #[serde(with = "i128_decimal_string")]
    pub value: i128,
    /// Number of decimal digits represented by `value`.
    pub scale: u32,
    pub unit: QuotaUnit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversion_generation: Option<u64>,
}

mod i128_decimal_string {
    use std::fmt;

    use serde::{
        Deserializer, Serializer,
        de::{self, Unexpected, Visitor},
    };

    pub fn serialize<S>(value: &i128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<i128, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(I128Visitor)
    }

    struct I128Visitor;

    impl<'de> Visitor<'de> for I128Visitor {
        type Value = i128;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a base-10 i128 string or JSON integer")
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(i128::from(value))
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(i128::from(value))
        }

        fn visit_i128<E>(self, value: i128) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(value)
        }

        fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            i128::try_from(value).map_err(|_| {
                E::invalid_value(Unexpected::Other("integer outside the i128 range"), &self)
            })
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            value
                .parse::<i128>()
                .map_err(|_| E::invalid_value(Unexpected::Str(value), &self))
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            self.visit_str(&value)
        }

        fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Err(E::invalid_type(Unexpected::Float(value), &self))
        }
    }
}

impl QuotaQuantity {
    pub fn new(value: i128, scale: u32, unit: QuotaUnit) -> Self {
        let (value, scale) = canonical_decimal_parts(value, scale);
        Self {
            value,
            scale,
            unit,
            conversion_generation: None,
        }
    }

    pub fn with_conversion_generation(mut self, generation: Option<u64>) -> Self {
        self.conversion_generation = generation;
        self
    }

    pub fn canonicalized(mut self) -> Self {
        (self.value, self.scale) = canonical_decimal_parts(self.value, self.scale);
        self
    }

    pub fn from_integer(value: i128, unit: QuotaUnit) -> Self {
        Self::new(value, 0, unit)
    }

    /// Parse a decimal without floating point rounding.  Scientific notation is intentionally
    /// rejected because provider quota values are expected to be ordinary decimal strings.
    pub fn from_decimal(value: &str, unit: QuotaUnit) -> Option<Self> {
        let text = value.trim();
        if text.is_empty() || text.contains(['e', 'E']) {
            return None;
        }
        let negative = text.starts_with('-');
        let unsigned = text.trim_start_matches(['+', '-']);
        let mut parts = unsigned.split('.');
        let whole = parts.next().unwrap_or_default();
        let fraction = parts.next().unwrap_or_default();
        if parts.next().is_some()
            || whole.is_empty() && fraction.is_empty()
            || !whole.chars().all(|c| c.is_ascii_digit())
            || !fraction.chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let digits = format!("{whole}{fraction}");
        let mut parsed = digits.parse::<i128>().ok()?;
        if negative {
            parsed = parsed.checked_neg()?;
        }
        Some(Self::new(parsed, fraction.len() as u32, unit))
    }

    pub fn checked_add(&self, other: &Self) -> Option<Self> {
        let (left, right, scale) = aligned_quantity_values(self, other)?;
        Some(
            Self::new(left.checked_add(right)?, scale, self.unit)
                .with_conversion_generation(self.conversion_generation),
        )
    }

    pub fn checked_sub(&self, other: &Self) -> Option<Self> {
        let (left, right, scale) = aligned_quantity_values(self, other)?;
        Some(
            Self::new(left.checked_sub(right)?, scale, self.unit)
                .with_conversion_generation(self.conversion_generation),
        )
    }

    pub fn is_zero(&self) -> bool {
        self.value == 0
    }
}

fn canonical_decimal_parts(mut value: i128, mut scale: u32) -> (i128, u32) {
    if value == 0 {
        return (0, 0);
    }
    while scale > 0 && value % 10 == 0 {
        value /= 10;
        scale -= 1;
    }
    (value, scale)
}

fn aligned_quantity_values(
    left: &QuotaQuantity,
    right: &QuotaQuantity,
) -> Option<(i128, i128, u32)> {
    if left.unit != right.unit || left.conversion_generation != right.conversion_generation {
        return None;
    }
    let scale = left.scale.max(right.scale);
    let left_multiplier = 10_i128.checked_pow(scale.checked_sub(left.scale)?)?;
    let right_multiplier = 10_i128.checked_pow(scale.checked_sub(right.scale)?)?;
    Some((
        left.value.checked_mul(left_multiplier)?,
        right.value.checked_mul(right_multiplier)?,
        scale,
    ))
}

/// Where a raw-unit conversion came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConversionSource {
    Remote,
    Configured,
    Bundled,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(default)]
pub struct QuotaConversion {
    pub source: ConversionSource,
    pub divisor: Option<u64>,
    pub generation: Option<u64>,
}

impl QuotaConversion {
    pub fn stable_generation(source: ConversionSource, divisor: u64) -> u64 {
        let source_revision = match source {
            ConversionSource::Remote => 1_u64,
            ConversionSource::Configured => 2,
            ConversionSource::Bundled => 3,
            ConversionSource::Unknown => 0,
        };
        source_revision.rotate_left(56) ^ divisor
    }
}

/// Complete normalization signature.  Any field change starts a new rate epoch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(default)]
pub struct NormalizationSignature {
    pub pool_key: String,
    pub pool_revision: u64,
    pub counter_kind: QuotaCounterKind,
    pub unit: QuotaUnit,
    pub conversion_generation: Option<u64>,
    pub scope: QuotaScope,
    pub window: QuotaWindowSemantics,
    pub window_start_ms: Option<u64>,
    pub window_end_ms: Option<u64>,
    pub reset_at_ms: Option<u64>,
    pub limit_key: Option<String>,
    pub plan_identity: Option<String>,
    pub limit_quantity: Option<QuotaQuantity>,
    pub unlimited: bool,
    pub adjustment_revision: u64,
    pub continuity_marker: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum QuotaAdjustmentKind {
    Discontinuity,
    CounterResetOrRollback,
    TopUp,
    LimitOrPlanChanged,
    NormalizationChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct QuotaObservation {
    pub pool: PoolIdentity,
    pub endpoint: Option<ProviderEndpointKey>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub observation_provider_id: String,
    pub observed_at_ms: u64,
    pub source: String,
    pub status: String,
    pub fresh: bool,
    pub used: Option<QuotaQuantity>,
    pub remaining: Option<QuotaQuantity>,
    pub direct_total: Option<QuotaQuantity>,
    pub limit: Option<QuotaQuantity>,
    pub window_start_ms: Option<u64>,
    pub window_end_ms: Option<u64>,
    pub reset_at_ms: Option<u64>,
    pub conversion: Option<QuotaConversion>,
    pub capabilities: QuotaCapabilities,
    pub window: QuotaWindowSemantics,
    pub sampling: QuotaSamplingSemantics,
    pub signature: NormalizationSignature,
    pub adjustment: Option<QuotaAdjustmentKind>,
}

impl QuotaObservation {
    pub fn has_amount(&self) -> bool {
        self.used.is_some()
            || self.remaining.is_some()
            || self.direct_total.is_some()
            || self.limit.is_some()
            || self.capabilities.unlimited
    }
}

/// Transient adapter context used to resolve a pool identity.  It is never serialized.
#[derive(Clone, Default)]
pub struct QuotaObservationContext {
    pub origin: String,
    pub scope: QuotaScope,
    pub remote_stable_id: Option<String>,
    pub remote_identity_proof: RemoteIdentityProof,
    pub identity_conflict: bool,
    pub explicit_pool_id: Option<String>,
    pub credential: Option<Vec<u8>>,
    pub install_key: Option<Vec<u8>>,
    pub conversion: Option<QuotaConversion>,
    pub counter_kind: QuotaCounterKind,
    pub unit: QuotaUnit,
    pub capabilities: QuotaCapabilities,
    pub window: Option<QuotaWindowSemantics>,
    pub expected_interval_ms: Option<u64>,
    pub fresh_until_ms: Option<u64>,
    pub continuity_deadline_ms: Option<u64>,
    pub used: Option<QuotaQuantity>,
    pub remaining: Option<QuotaQuantity>,
    pub direct_total: Option<QuotaQuantity>,
    pub limit: Option<QuotaQuantity>,
    pub continuity_marker: Option<String>,
    pub window_start_ms: Option<u64>,
    pub window_end_ms: Option<u64>,
}

impl fmt::Debug for QuotaObservationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QuotaObservationContext")
            .field("origin", &normalize_issuer_authority(&self.origin))
            .field("scope", &self.scope)
            .field(
                "remote_stable_id",
                &self.remote_stable_id.as_ref().map(|_| "<redacted>"),
            )
            .field("remote_identity_proof", &self.remote_identity_proof)
            .field("identity_conflict", &self.identity_conflict)
            .field("explicit_pool_id", &self.explicit_pool_id)
            .field("conversion", &self.conversion)
            .field("counter_kind", &self.counter_kind)
            .field("unit", &self.unit)
            .field("capabilities", &self.capabilities)
            .field("window", &self.window)
            .field("expected_interval_ms", &self.expected_interval_ms)
            .field("fresh_until_ms", &self.fresh_until_ms)
            .field("continuity_deadline_ms", &self.continuity_deadline_ms)
            .field("used", &self.used)
            .field("remaining", &self.remaining)
            .field("direct_total", &self.direct_total)
            .field("limit", &self.limit)
            .field("continuity_marker", &self.continuity_marker)
            .field("window_start_ms", &self.window_start_ms)
            .field("window_end_ms", &self.window_end_ms)
            .field("credential", &"<redacted>")
            .field("install_key", &"<redacted>")
            .finish()
    }
}

impl QuotaObservationContext {
    pub fn new(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            ..Self::default()
        }
    }

    pub fn identity(&self, revision: u64) -> PoolIdentity {
        PoolIdentity::resolve_with_proof(
            &self.origin,
            self.scope.clone(),
            self.remote_stable_id.as_deref(),
            self.remote_identity_proof,
            self.explicit_pool_id.as_deref(),
            self.credential.as_deref(),
            self.install_key.as_deref(),
            revision,
            self.identity_conflict,
        )
    }

    pub fn identity_for_endpoint(
        &self,
        revision: u64,
        endpoint: &ProviderEndpointKey,
    ) -> PoolIdentity {
        let mut identity = self.identity(revision);
        if identity.evidence == IdentityEvidence::EndpointOrigin {
            identity.key = format!(
                "endpoint:{}:{}:{}",
                identity.origin,
                identity.scope.as_key(),
                sanitize_component(&endpoint.stable_key())
            );
        }
        identity
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PoolMembership {
    pub pool: PoolIdentity,
    pub endpoint: ProviderEndpointKey,
    pub since_ms: u64,
}

impl Default for PoolMembership {
    fn default() -> Self {
        Self {
            pool: PoolIdentity::default(),
            endpoint: ProviderEndpointKey::new("", "", ""),
            since_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct RegistryUpdate {
    pub generation: u64,
    pub pool: Option<PoolIdentity>,
    pub membership: Option<PoolMembership>,
    pub appended: bool,
    pub duplicate: bool,
    pub out_of_order: bool,
    pub carried_forward: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct QuotaPoolState {
    pub identity: PoolIdentity,
    pub samples: VecDeque<QuotaObservation>,
    pub last_success_at_ms: Option<u64>,
    pub last_attempt_at_ms: Option<u64>,
    pub adjustment_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct QuotaRegistryCheckpoint {
    pub schema_version: u32,
    pub generation: u64,
    pub pools: BTreeMap<String, QuotaPoolState>,
    #[serde(default, with = "endpoint_membership_map")]
    pub memberships: BTreeMap<ProviderEndpointKey, PoolMembership>,
}

mod endpoint_membership_map {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

    use super::{PoolMembership, ProviderEndpointKey};

    pub(super) fn serialize<S>(
        memberships: &BTreeMap<ProviderEndpointKey, PoolMembership>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        memberships.iter().collect::<Vec<_>>().serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<BTreeMap<ProviderEndpointKey, PoolMembership>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Memberships {
            Entries(Vec<(ProviderEndpointKey, PoolMembership)>),
            LegacyObject(BTreeMap<String, serde_json::Value>),
        }

        match Memberships::deserialize(deserializer)? {
            Memberships::Entries(entries) => {
                let mut memberships = BTreeMap::new();
                for (endpoint, membership) in entries {
                    if memberships.insert(endpoint.clone(), membership).is_some() {
                        return Err(D::Error::custom(format!(
                            "duplicate quota membership for {}",
                            endpoint.stable_key()
                        )));
                    }
                }
                Ok(memberships)
            }
            Memberships::LegacyObject(entries) if entries.is_empty() => Ok(BTreeMap::new()),
            Memberships::LegacyObject(_) => Err(D::Error::custom(
                "legacy quota membership objects must be empty",
            )),
        }
    }
}

impl Default for QuotaRegistryCheckpoint {
    fn default() -> Self {
        Self {
            schema_version: QUOTA_CHECKPOINT_SCHEMA_VERSION,
            generation: 0,
            pools: BTreeMap::new(),
            memberships: BTreeMap::new(),
        }
    }
}

/// In-memory quota state machine whose exported checkpoint is committed through RuntimeStore.
/// This type intentionally exposes one mutating ingestion method.
#[derive(Debug, Clone)]
pub struct QuotaPoolRegistry {
    generation: u64,
    pools: BTreeMap<String, QuotaPoolState>,
    memberships: BTreeMap<ProviderEndpointKey, PoolMembership>,
    max_samples_per_pool: usize,
    retention_ms: u64,
    max_pools: usize,
}

impl Default for QuotaPoolRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_SAMPLES_PER_POOL, DEFAULT_SAMPLE_RETENTION_MS)
    }
}

impl QuotaPoolRegistry {
    pub fn new(max_samples_per_pool: usize, retention_ms: u64) -> Self {
        Self {
            generation: 0,
            pools: BTreeMap::new(),
            memberships: BTreeMap::new(),
            max_samples_per_pool: max_samples_per_pool.max(1),
            retention_ms: retention_ms.max(1),
            max_pools: DEFAULT_MAX_POOLS,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn record_snapshot(
        &mut self,
        endpoint: &ProviderEndpointKey,
        context: &QuotaObservationContext,
        snapshot: &ProviderBalanceSnapshot,
    ) -> RegistryUpdate {
        let now_ms = snapshot.fetched_at_ms;
        let previous = self.memberships.get(endpoint).cloned();
        self.generation = self.generation.saturating_add(1);
        let generation = self.generation;
        if snapshot.stale
            || matches!(
                snapshot.status,
                BalanceSnapshotStatus::Unknown
                    | BalanceSnapshotStatus::Stale
                    | BalanceSnapshotStatus::Error
            )
        {
            if let Some(membership) = previous.as_ref()
                && let Some(pool) = self.pools.get_mut(&membership.pool.key)
            {
                pool.last_attempt_at_ms = Some(
                    pool.last_attempt_at_ms
                        .map_or(now_ms, |previous| previous.max(now_ms)),
                );
            }
            return RegistryUpdate {
                generation,
                pool: previous.as_ref().map(|membership| membership.pool.clone()),
                membership: previous,
                carried_forward: true,
                ..RegistryUpdate::default()
            };
        }

        let identity = self.resolve_pool_identity(endpoint, context, previous.as_ref());
        let mut observation = observation_from_snapshot(endpoint, &identity, context, snapshot);
        let Some(observation) = observation.as_mut() else {
            return RegistryUpdate {
                generation,
                pool: previous.as_ref().map(|membership| membership.pool.clone()),
                membership: previous,
                carried_forward: true,
                ..RegistryUpdate::default()
            };
        };
        if !observation.has_amount() {
            return RegistryUpdate {
                generation,
                pool: previous.as_ref().map(|membership| membership.pool.clone()),
                membership: previous,
                carried_forward: true,
                ..RegistryUpdate::default()
            };
        }
        let membership = PoolMembership {
            pool: identity.clone(),
            endpoint: endpoint.clone(),
            since_ms: previous
                .as_ref()
                .filter(|old| old.pool.key == identity.key)
                .map(|old| old.since_ms)
                .unwrap_or(now_ms),
        };
        self.memberships
            .insert(endpoint.clone(), membership.clone());
        let prior_sample = self
            .pools
            .get(&identity.key)
            .and_then(|pool| pool.samples.back())
            .cloned();
        if let Some(last) = prior_sample.as_ref() {
            if observation.observed_at_ms < last.observed_at_ms {
                return RegistryUpdate {
                    generation,
                    pool: Some(identity),
                    membership: Some(membership),
                    out_of_order: true,
                    ..RegistryUpdate::default()
                };
            }
            if observation.observed_at_ms == last.observed_at_ms {
                return RegistryUpdate {
                    generation,
                    pool: Some(identity),
                    membership: Some(membership),
                    duplicate: true,
                    ..RegistryUpdate::default()
                };
            }
        }

        let previous_adjustment_revision = self
            .pools
            .get(&identity.key)
            .map_or(0, |pool| pool.adjustment_revision);
        let adjustment = prior_sample
            .as_ref()
            .and_then(|previous| adjustment_between(previous, observation));
        let adjustment_revision =
            previous_adjustment_revision.saturating_add(u64::from(adjustment.is_some()));
        observation.adjustment = adjustment;
        observation.signature.adjustment_revision = adjustment_revision;

        self.ensure_pool_capacity(&identity.key);
        let pool = self
            .pools
            .entry(identity.key.clone())
            .or_insert_with(|| QuotaPoolState {
                identity: identity.clone(),
                ..QuotaPoolState::default()
            });
        pool.last_attempt_at_ms = Some(
            pool.last_attempt_at_ms
                .map_or(now_ms, |previous| previous.max(now_ms)),
        );
        pool.adjustment_revision = adjustment_revision;
        pool.samples.push_back(observation.clone());
        pool.last_success_at_ms = Some(now_ms);
        while pool.samples.len() > self.max_samples_per_pool {
            pool.samples.pop_front();
        }
        let cutoff = now_ms.saturating_sub(self.retention_ms);
        while pool
            .samples
            .front()
            .is_some_and(|sample| sample.observed_at_ms < cutoff)
        {
            pool.samples.pop_front();
        }
        RegistryUpdate {
            generation,
            pool: Some(identity),
            membership: Some(membership),
            appended: true,
            ..RegistryUpdate::default()
        }
    }

    fn resolve_pool_identity(
        &self,
        endpoint: &ProviderEndpointKey,
        context: &QuotaObservationContext,
        previous: Option<&PoolMembership>,
    ) -> PoolIdentity {
        let previous_revision = previous.map_or(0, |membership| membership.pool.revision);
        let mut identity = context.identity_for_endpoint(previous_revision, endpoint);
        if let Some(pool) = self.pools.get(&identity.key) {
            return pool.identity.clone();
        }
        if previous.is_some_and(|membership| membership.pool.key != identity.key) {
            identity.revision = previous_revision.saturating_add(1);
        }
        identity
    }

    pub fn membership_for_endpoint(
        &self,
        endpoint: &ProviderEndpointKey,
    ) -> Option<PoolMembership> {
        self.memberships.get(endpoint).cloned()
    }

    pub fn pools(&self) -> Vec<QuotaPoolState> {
        self.pools.values().cloned().collect()
    }

    pub fn pool(&self, pool_key: &str) -> Option<QuotaPoolState> {
        self.pools.get(pool_key).cloned()
    }

    pub fn pool_identities(&self) -> Vec<PoolIdentity> {
        self.pools
            .values()
            .map(|pool| pool.identity.clone())
            .collect()
    }

    pub fn samples_for_pool(&self, pool_key: &str) -> Vec<QuotaObservation> {
        self.pools
            .get(pool_key)
            .map(|pool| pool.samples.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn checkpoint(&self) -> QuotaRegistryCheckpoint {
        QuotaRegistryCheckpoint {
            schema_version: QUOTA_CHECKPOINT_SCHEMA_VERSION,
            generation: self.generation,
            pools: self.pools.clone(),
            memberships: self.memberships.clone(),
        }
    }

    pub fn from_checkpoint(
        checkpoint: QuotaRegistryCheckpoint,
        max_samples_per_pool: usize,
        retention_ms: u64,
    ) -> Option<Self> {
        let reference_ms = checkpoint
            .pools
            .values()
            .filter_map(|pool| pool.last_attempt_at_ms)
            .max()
            .unwrap_or(0);
        Self::from_checkpoint_at(checkpoint, max_samples_per_pool, retention_ms, reference_ms)
    }

    pub fn from_checkpoint_at(
        checkpoint: QuotaRegistryCheckpoint,
        max_samples_per_pool: usize,
        retention_ms: u64,
        now_ms: u64,
    ) -> Option<Self> {
        if checkpoint.schema_version != QUOTA_CHECKPOINT_SCHEMA_VERSION {
            return None;
        }
        let mut registry = Self {
            generation: checkpoint.generation,
            pools: checkpoint.pools,
            memberships: checkpoint.memberships,
            max_samples_per_pool: max_samples_per_pool.max(1),
            retention_ms: retention_ms.max(1),
            max_pools: DEFAULT_MAX_POOLS,
        };
        let cutoff = now_ms.saturating_sub(registry.retention_ms);
        for pool in registry.pools.values_mut() {
            for sample in &mut pool.samples {
                canonicalize_observation_quantities(sample);
            }
            while pool.samples.len() > registry.max_samples_per_pool {
                pool.samples.pop_front();
            }
            while pool
                .samples
                .front()
                .is_some_and(|sample| sample.observed_at_ms < cutoff)
            {
                pool.samples.pop_front();
            }
        }
        registry
            .memberships
            .retain(|_, membership| registry.pools.contains_key(&membership.pool.key));
        while registry.pools.len() > registry.max_pools {
            if !registry.remove_oldest_inactive_pool(None) {
                break;
            }
        }
        Some(registry)
    }

    fn ensure_pool_capacity(&mut self, keep_key: &str) {
        if self.pools.contains_key(keep_key) {
            return;
        }
        while self.pools.len() >= self.max_pools {
            if !self.remove_oldest_inactive_pool(Some(keep_key)) {
                break;
            }
        }
    }

    fn remove_oldest_inactive_pool(&mut self, keep_key: Option<&str>) -> bool {
        let active = self
            .memberships
            .values()
            .map(|membership| membership.pool.key.as_str())
            .collect::<std::collections::HashSet<_>>();
        let candidate = self
            .pools
            .iter()
            .filter(|(key, _)| keep_key != Some(key.as_str()))
            .filter(|(key, _)| !active.contains(key.as_str()))
            .min_by_key(|(_, pool)| pool.last_attempt_at_ms.unwrap_or(0))
            .map(|(key, _)| key.clone());
        let Some(candidate) = candidate else {
            return false;
        };
        let removed = self.pools.remove(&candidate).is_some();
        if removed {
            self.memberships
                .retain(|_, membership| membership.pool.key != candidate);
        }
        removed
    }
}

fn canonicalize_observation_quantities(observation: &mut QuotaObservation) {
    for quantity in [
        &mut observation.used,
        &mut observation.remaining,
        &mut observation.direct_total,
        &mut observation.limit,
        &mut observation.signature.limit_quantity,
    ] {
        if let Some(value) = quantity.take() {
            *quantity = Some(value.canonicalized());
        }
    }
}

fn observation_from_snapshot(
    endpoint: &ProviderEndpointKey,
    identity: &PoolIdentity,
    context: &QuotaObservationContext,
    snapshot: &ProviderBalanceSnapshot,
) -> Option<QuotaObservation> {
    let unit = context.unit;
    let conversion_generation = context
        .conversion
        .as_ref()
        .and_then(|conversion| conversion.generation);
    let used = context
        .used
        .clone()
        .map(QuotaQuantity::canonicalized)
        .or_else(|| {
            quantity_from_snapshot(
                snapshot.quota_used_usd.as_deref(),
                unit,
                conversion_generation,
            )
        });
    let remaining = context
        .remaining
        .clone()
        .map(QuotaQuantity::canonicalized)
        .or_else(|| {
            quantity_from_snapshot(
                snapshot.quota_remaining_usd.as_deref(),
                unit,
                conversion_generation,
            )
        });
    let limit = context
        .limit
        .clone()
        .map(QuotaQuantity::canonicalized)
        .or_else(|| {
            quantity_from_snapshot(
                snapshot.quota_limit_usd.as_deref(),
                unit,
                conversion_generation,
            )
        });
    let direct_total = context
        .direct_total
        .clone()
        .map(QuotaQuantity::canonicalized)
        .or_else(|| {
            quantity_from_snapshot(
                snapshot.today_used_usd.as_deref(),
                unit,
                conversion_generation,
            )
        });
    let mut capabilities = context.capabilities.clone();
    capabilities.used |= used.is_some();
    capabilities.remaining |= remaining.is_some();
    capabilities.limit |= limit.is_some();
    capabilities.direct_total |= direct_total.is_some();
    capabilities.unlimited |= snapshot.unlimited_quota == Some(true);
    if unit == QuotaUnit::Raw {
        capabilities.raw_unit = true;
    }
    let window = context.window.clone().unwrap_or_else(|| {
        QuotaWindowSemantics::from_provider_hint(
            snapshot.quota_period.as_deref(),
            snapshot.quota_resets_at_ms,
            None,
            context.window_start_ms,
            context.window_end_ms,
        )
    });
    capabilities.window |= window.kind != QuotaWindowKind::Unknown
        || context.window_start_ms.is_some()
        || context.window_end_ms.is_some();
    capabilities.reset |= window.reset != QuotaResetKind::Unknown;
    let derived_deadline_ms = context.expected_interval_ms.and_then(|interval| {
        snapshot
            .fetched_at_ms
            .checked_add(interval.saturating_mul(3))
    });
    let fresh_until_ms = context
        .fresh_until_ms
        .or(snapshot.stale_after_ms)
        .or(derived_deadline_ms);
    let sampling = QuotaSamplingSemantics {
        expected_interval_ms: context.expected_interval_ms,
        fresh_until_ms,
        continuity_deadline_ms: context.continuity_deadline_ms.or(fresh_until_ms),
    };
    let signature = NormalizationSignature {
        pool_key: identity.key.clone(),
        pool_revision: identity.revision,
        counter_kind: context.counter_kind,
        unit,
        conversion_generation,
        scope: context.scope.clone(),
        window: window.clone(),
        window_start_ms: context.window_start_ms,
        window_end_ms: context.window_end_ms,
        reset_at_ms: snapshot.quota_resets_at_ms,
        limit_key: snapshot.quota_period.clone(),
        plan_identity: snapshot.plan_name.clone(),
        limit_quantity: limit.clone(),
        unlimited: capabilities.unlimited,
        adjustment_revision: 0,
        continuity_marker: context.continuity_marker.clone(),
    };
    Some(QuotaObservation {
        pool: identity.clone(),
        endpoint: Some(endpoint.clone()),
        observation_provider_id: snapshot.observation_provider_id.clone(),
        observed_at_ms: snapshot.fetched_at_ms,
        source: snapshot.source.clone(),
        status: snapshot.status.as_str().to_string(),
        fresh: !snapshot.stale,
        used,
        remaining,
        direct_total,
        limit,
        window_start_ms: context.window_start_ms,
        window_end_ms: context.window_end_ms,
        reset_at_ms: snapshot.quota_resets_at_ms,
        conversion: context.conversion.clone(),
        capabilities,
        window,
        sampling,
        signature,
        adjustment: None,
    })
}

fn adjustment_between(
    previous: &QuotaObservation,
    current: &QuotaObservation,
) -> Option<QuotaAdjustmentKind> {
    if previous
        .sampling
        .continuity_deadline_ms
        .is_some_and(|deadline| current.observed_at_ms > deadline)
    {
        return Some(QuotaAdjustmentKind::Discontinuity);
    }
    if quantity_decreased(previous.used.as_ref(), current.used.as_ref()) {
        return Some(QuotaAdjustmentKind::CounterResetOrRollback);
    }
    if quantity_increased(previous.remaining.as_ref(), current.remaining.as_ref()) {
        return Some(QuotaAdjustmentKind::TopUp);
    }
    if previous.signature.limit_key != current.signature.limit_key
        || previous.signature.plan_identity != current.signature.plan_identity
        || previous.signature.limit_quantity != current.signature.limit_quantity
        || previous.signature.unlimited != current.signature.unlimited
    {
        return Some(QuotaAdjustmentKind::LimitOrPlanChanged);
    }
    let previous_signature = &previous.signature;
    let current_signature = &current.signature;
    if previous_signature.pool_key != current_signature.pool_key
        || previous_signature.pool_revision != current_signature.pool_revision
        || previous_signature.counter_kind != current_signature.counter_kind
        || previous_signature.unit != current_signature.unit
        || previous_signature.conversion_generation != current_signature.conversion_generation
        || previous_signature.scope != current_signature.scope
        || previous_signature.window != current_signature.window
        || previous_signature.window_start_ms != current_signature.window_start_ms
        || previous_signature.window_end_ms != current_signature.window_end_ms
        || previous_signature.reset_at_ms != current_signature.reset_at_ms
        || previous_signature.continuity_marker != current_signature.continuity_marker
    {
        return Some(QuotaAdjustmentKind::NormalizationChanged);
    }
    None
}

fn quantity_decreased(previous: Option<&QuotaQuantity>, current: Option<&QuotaQuantity>) -> bool {
    comparable_quantity_values(previous, current)
        .is_some_and(|(previous, current)| current < previous)
}

fn quantity_increased(previous: Option<&QuotaQuantity>, current: Option<&QuotaQuantity>) -> bool {
    comparable_quantity_values(previous, current)
        .is_some_and(|(previous, current)| current > previous)
}

fn comparable_quantity_values(
    previous: Option<&QuotaQuantity>,
    current: Option<&QuotaQuantity>,
) -> Option<(i128, i128)> {
    let (previous, current) = (previous?, current?);
    let (previous, current, _) = aligned_quantity_values(previous, current)?;
    Some((previous, current))
}

fn quantity_from_snapshot(
    value: Option<&str>,
    unit: QuotaUnit,
    conversion_generation: Option<u64>,
) -> Option<QuotaQuantity> {
    QuotaQuantity::from_decimal(value?, unit)
        .map(|quantity| quantity.with_conversion_generation(conversion_generation))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn normalize_issuer_authority(value: &str) -> String {
    let trimmed = value.trim();
    if let Ok(url) = reqwest::Url::parse(trimmed)
        && let Some(host) = url.host_str()
    {
        let scheme = url.scheme().to_ascii_lowercase();
        let host = host.to_ascii_lowercase();
        let port = url
            .port()
            .filter(|port| !(scheme == "http" && *port == 80 || scheme == "https" && *port == 443));
        return match port {
            Some(port) => format!("{scheme}://{host}:{port}"),
            None => format!("{scheme}://{host}"),
        };
    }
    format!("issuer:unknown:{}", sanitize_component(trimmed))
}

fn sanitize_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(96));
    for ch in value.trim().chars().take(96) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/') {
            output.push(ch);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "unknown".to_string()
    } else {
        output
    }
}

fn opaque_identity_digest(
    key: &[u8],
    evidence_domain: &[u8],
    issuer: &str,
    scope: &str,
    secret_value: &[u8],
) -> Option<String> {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).ok()?;
    mac.update(b"codex-helper/quota-pool/hmac-sha256/v1\0");
    mac.update(evidence_domain);
    mac.update(b"\0");
    mac.update(issuer.as_bytes());
    mac.update(b"\0");
    mac.update(scope.as_bytes());
    mac.update(b"\0");
    mac.update(secret_value);
    let digest = mac.finalize().into_bytes();
    let opaque = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Some(format!("hmac-sha256-v1:{opaque}"))
}

impl fmt::Display for QuotaUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(name: &str) -> ProviderEndpointKey {
        ProviderEndpointKey::new("codex", name, "default")
    }

    fn snapshot(at: u64, used: &str) -> ProviderBalanceSnapshot {
        let mut snapshot =
            ProviderBalanceSnapshot::new("relay", endpoint("relay"), "test", at, None);
        snapshot.status = BalanceSnapshotStatus::Ok;
        snapshot.quota_used_usd = Some(used.to_string());
        snapshot
    }

    fn snapshot_with_capacity(
        at: u64,
        used: &str,
        remaining: &str,
        limit: &str,
        plan: &str,
    ) -> ProviderBalanceSnapshot {
        let mut snapshot = snapshot(at, used);
        snapshot.quota_remaining_usd = Some(remaining.to_string());
        snapshot.quota_limit_usd = Some(limit.to_string());
        snapshot.plan_name = Some(plan.to_string());
        snapshot
    }

    #[test]
    fn identity_precedence_is_remote_then_explicit_then_fingerprint() {
        let key = [7_u8; 32];
        let remote = PoolIdentity::resolve(
            "https://relay.example/v1",
            QuotaScope::Account,
            Some("remote-1"),
            Some("operator-1"),
            Some(b"secret"),
            Some(&key),
            0,
        );
        assert_eq!(remote.evidence, IdentityEvidence::RemoteQuotaOwnerId);
        let explicit = PoolIdentity::resolve(
            "https://relay.example/v1",
            QuotaScope::Account,
            None,
            Some("operator-1"),
            Some(b"secret"),
            Some(&key),
            0,
        );
        assert_eq!(explicit.evidence, IdentityEvidence::ExplicitPoolId);
        let fallback = PoolIdentity::resolve(
            "https://relay.example/v1",
            QuotaScope::Account,
            None,
            None,
            Some(b"secret"),
            Some(&key),
            0,
        );
        assert_eq!(fallback.evidence, IdentityEvidence::CredentialFingerprint);
        assert!(!fallback.key.contains("secret"));
    }

    #[test]
    fn unverified_remote_subject_does_not_override_explicit_pool() {
        let identity = PoolIdentity::resolve_with_proof(
            "https://relay.example",
            QuotaScope::Account,
            Some("user-42"),
            RemoteIdentityProof::StableSubject,
            Some("shared-monthly"),
            Some(b"secret"),
            Some(&[3_u8; 32]),
            0,
            false,
        );

        assert_eq!(identity.evidence, IdentityEvidence::ExplicitPoolId);
        assert!(identity.key.contains("shared-monthly"));
    }

    #[test]
    fn proven_quota_owner_conflict_is_not_aggregation_eligible() {
        let identity = PoolIdentity::resolve_with_proof(
            "https://relay.example",
            QuotaScope::Account,
            Some("quota-owner-42"),
            RemoteIdentityProof::QuotaOwner,
            Some("operator-pool"),
            None,
            Some(&[9_u8; 32]),
            0,
            true,
        );

        assert_eq!(identity.evidence, IdentityEvidence::RemoteQuotaOwnerId);
        assert_eq!(identity.confidence, IdentityConfidence::Low);
        assert!(identity.conflicting_evidence);
        assert!(!identity.aggregation_eligible);
    }

    #[test]
    fn remote_ids_are_namespaced_by_origin_and_scope() {
        let a = PoolIdentity::resolve(
            "https://a.example",
            QuotaScope::Account,
            Some("same"),
            None,
            None,
            Some(&[8_u8; 32]),
            0,
        );
        let b = PoolIdentity::resolve(
            "https://b.example",
            QuotaScope::Account,
            Some("same"),
            None,
            None,
            Some(&[8_u8; 32]),
            0,
        );
        assert_ne!(a.key, b.key);
    }

    #[test]
    fn registry_rejects_stale_error_duplicate_and_out_of_order_points() {
        let mut registry = QuotaPoolRegistry::new(8, 10_000);
        let mut context = QuotaObservationContext::new("https://relay.example/v1");
        context.scope = QuotaScope::Account;
        context.unit = QuotaUnit::Usd;
        context.remote_stable_id = Some("pool".to_string());
        let endpoint = endpoint("relay");
        assert!(
            registry
                .record_snapshot(&endpoint, &context, &snapshot(10, "1"))
                .appended
        );
        assert!(
            registry
                .record_snapshot(&endpoint, &context, &snapshot(10, "1"))
                .duplicate
        );
        assert!(
            registry
                .record_snapshot(&endpoint, &context, &snapshot(9, "1"))
                .out_of_order
        );
        let mut error = snapshot(11, "2");
        error.status = BalanceSnapshotStatus::Error;
        assert!(
            registry
                .record_snapshot(&endpoint, &context, &error)
                .carried_forward
        );
        assert_eq!(
            registry
                .samples_for_pool(&registry.pools()[0].identity.key)
                .len(),
            1
        );
    }

    #[test]
    fn duplicate_shared_pool_sample_still_registers_endpoint_membership() {
        let mut registry = QuotaPoolRegistry::new(8, 10_000);
        let mut context = QuotaObservationContext::new("https://relay.example/v1");
        context.explicit_pool_id = Some("shared".to_string());
        let first_endpoint = endpoint("relay-a");
        let second_endpoint = endpoint("relay-b");

        let first = registry.record_snapshot(&first_endpoint, &context, &snapshot(10, "1"));
        let duplicate = registry.record_snapshot(&second_endpoint, &context, &snapshot(10, "1"));

        assert!(first.appended);
        assert!(duplicate.duplicate);
        assert!(!duplicate.appended);
        let first_membership = registry
            .membership_for_endpoint(&first_endpoint)
            .expect("first endpoint membership");
        let second_membership = registry
            .membership_for_endpoint(&second_endpoint)
            .expect("second endpoint membership");
        assert_eq!(second_membership.pool, first_membership.pool);
        assert_eq!(duplicate.pool.as_ref(), Some(&first_membership.pool));
        assert_eq!(duplicate.membership.as_ref(), Some(&second_membership));
        assert_eq!(
            registry.samples_for_pool(&first_membership.pool.key).len(),
            1
        );
    }

    #[test]
    fn out_of_order_shared_pool_sample_still_registers_endpoint_membership() {
        let mut registry = QuotaPoolRegistry::new(8, 10_000);
        let mut context = QuotaObservationContext::new("https://relay.example/v1");
        context.explicit_pool_id = Some("shared".to_string());
        let fast_endpoint = endpoint("relay-fast");
        let slow_endpoint = endpoint("relay-slow");

        let latest = registry.record_snapshot(&fast_endpoint, &context, &snapshot(20, "2"));
        let out_of_order = registry.record_snapshot(&slow_endpoint, &context, &snapshot(10, "1"));

        assert!(latest.appended);
        assert!(out_of_order.out_of_order);
        assert!(!out_of_order.appended);
        let fast_membership = registry
            .membership_for_endpoint(&fast_endpoint)
            .expect("fast endpoint membership");
        let slow_membership = registry
            .membership_for_endpoint(&slow_endpoint)
            .expect("slow endpoint membership");
        assert_eq!(slow_membership.pool, fast_membership.pool);
        assert_eq!(out_of_order.pool.as_ref(), Some(&fast_membership.pool));
        assert_eq!(out_of_order.membership.as_ref(), Some(&slow_membership));
        assert_eq!(
            registry.samples_for_pool(&fast_membership.pool.key).len(),
            1
        );
    }

    #[test]
    fn converged_shared_pool_keeps_one_revision_across_endpoint_refreshes() {
        let mut registry = QuotaPoolRegistry::new(16, 10_000);
        let first_endpoint = endpoint("relay-a");
        let second_endpoint = endpoint("relay-b");
        let mut first_context = QuotaObservationContext::new("https://relay.example/v1");
        let mut second_context = QuotaObservationContext::new("https://relay.example/v1");

        first_context.explicit_pool_id = Some("first-old".to_string());
        registry.record_snapshot(&first_endpoint, &first_context, &snapshot(10, "1"));
        first_context.explicit_pool_id = Some("first-newer".to_string());
        registry.record_snapshot(&first_endpoint, &first_context, &snapshot(20, "2"));
        first_context.explicit_pool_id = Some("shared".to_string());
        let shared = registry
            .record_snapshot(&first_endpoint, &first_context, &snapshot(30, "3"))
            .pool
            .expect("shared pool");
        assert_eq!(shared.revision, 2);

        second_context.explicit_pool_id = Some("second-old".to_string());
        registry.record_snapshot(&second_endpoint, &second_context, &snapshot(40, "4"));
        second_context.explicit_pool_id = Some("shared".to_string());
        registry.record_snapshot(&second_endpoint, &second_context, &snapshot(50, "5"));
        registry.record_snapshot(&first_endpoint, &first_context, &snapshot(60, "6"));
        registry.record_snapshot(&second_endpoint, &second_context, &snapshot(70, "7"));

        let pool = registry.pool(&shared.key).expect("converged pool");
        assert_eq!(pool.identity.revision, shared.revision);
        assert!(
            pool.samples
                .iter()
                .all(|sample| sample.pool.revision == shared.revision
                    && sample.signature.pool_revision == shared.revision)
        );
        for endpoint in [&first_endpoint, &second_endpoint] {
            let membership = registry
                .membership_for_endpoint(endpoint)
                .expect("converged endpoint membership");
            assert_eq!(membership.pool.key, shared.key);
            assert_eq!(membership.pool.revision, shared.revision);
        }
    }

    #[test]
    fn stale_and_error_snapshots_preserve_existing_membership() {
        let mut registry = QuotaPoolRegistry::new(8, 10_000);
        let endpoint = endpoint("relay");
        let mut valid_context = QuotaObservationContext::new("https://relay.example/v1");
        valid_context.explicit_pool_id = Some("stable-pool".to_string());
        let first = registry.record_snapshot(&endpoint, &valid_context, &snapshot(10, "1"));
        let membership = first.membership.expect("initial membership");

        let mut unrelated_context = QuotaObservationContext::new("https://other.example/v1");
        unrelated_context.explicit_pool_id = Some("wrong-pool".to_string());
        let mut error = snapshot(20, "2");
        error.status = BalanceSnapshotStatus::Error;
        let error_update = registry.record_snapshot(&endpoint, &unrelated_context, &error);
        assert_eq!(error_update.membership.as_ref(), Some(&membership));

        let mut stale = snapshot(30, "3");
        stale.stale = true;
        let stale_update = registry.record_snapshot(&endpoint, &unrelated_context, &stale);
        assert_eq!(stale_update.membership.as_ref(), Some(&membership));
        assert_eq!(
            registry.membership_for_endpoint(&endpoint),
            Some(membership)
        );
        assert_eq!(registry.pools().len(), 1);
        assert_eq!(registry.pools()[0].last_attempt_at_ms, Some(30));
    }

    #[test]
    fn snapshot_attempt_timestamps_never_move_backward() {
        let mut registry = QuotaPoolRegistry::new(8, 10_000);
        let endpoint = endpoint("relay");
        let mut context = QuotaObservationContext::new("https://relay.example/v1");
        context.explicit_pool_id = Some("stable-pool".to_string());
        let pool = registry
            .record_snapshot(&endpoint, &context, &snapshot(10, "1"))
            .pool
            .expect("initial pool");
        let mut checkpoint = registry.checkpoint();
        checkpoint
            .pools
            .get_mut(&pool.key)
            .expect("checkpoint pool")
            .last_attempt_at_ms = Some(100);
        let mut restored = QuotaPoolRegistry::from_checkpoint(checkpoint, 8, 10_000)
            .expect("restore quota checkpoint");

        let appended = restored.record_snapshot(&endpoint, &context, &snapshot(20, "2"));
        assert!(appended.appended);
        assert_eq!(
            restored
                .pool(&pool.key)
                .expect("pool after successful refresh")
                .last_attempt_at_ms,
            Some(100)
        );

        let mut older_error = snapshot(5, "3");
        older_error.status = BalanceSnapshotStatus::Error;
        restored.record_snapshot(&endpoint, &context, &older_error);
        assert_eq!(
            restored
                .pool(&pool.key)
                .expect("pool after older error")
                .last_attempt_at_ms,
            Some(100)
        );
    }

    #[test]
    fn endpoints_without_identity_evidence_never_merge() {
        let mut registry = QuotaPoolRegistry::new(8, 10_000);
        let context = QuotaObservationContext::new("https://relay.example/v1");
        let first_endpoint = endpoint("relay-a");
        let second_endpoint = endpoint("relay-b");

        let first = registry.record_snapshot(&first_endpoint, &context, &snapshot(10, "1"));
        let second = registry.record_snapshot(&second_endpoint, &context, &snapshot(20, "2"));
        let first_pool = first.pool.expect("first pool");
        let second_pool = second.pool.expect("second pool");

        assert_ne!(first_pool.key, second_pool.key);
        assert!(!first_pool.aggregation_eligible);
        assert!(!second_pool.aggregation_eligible);
        assert_eq!(registry.pools().len(), 2);
    }

    #[test]
    fn resets_top_ups_rollbacks_and_limit_changes_advance_adjustment_revision() {
        let mut registry = QuotaPoolRegistry::new(8, 10_000);
        let endpoint = endpoint("relay");
        let mut context = QuotaObservationContext::new("https://relay.example/v1");
        context.explicit_pool_id = Some("monthly".to_string());

        let mut initial = snapshot_with_capacity(10, "100", "90", "190", "basic");
        initial.quota_resets_at_ms = Some(1_000);
        registry.record_snapshot(&endpoint, &context, &initial);

        let mut reset = snapshot_with_capacity(20, "0", "100", "100", "basic");
        reset.quota_resets_at_ms = Some(2_000);
        registry.record_snapshot(&endpoint, &context, &reset);

        let mut top_up = snapshot_with_capacity(30, "1", "110", "100", "basic");
        top_up.quota_resets_at_ms = Some(2_000);
        registry.record_snapshot(&endpoint, &context, &top_up);

        let mut rollback = snapshot_with_capacity(40, "0", "109", "100", "basic");
        rollback.quota_resets_at_ms = Some(2_000);
        registry.record_snapshot(&endpoint, &context, &rollback);

        let mut plan_change = snapshot_with_capacity(50, "1", "108", "200", "pro");
        plan_change.quota_resets_at_ms = Some(2_000);
        registry.record_snapshot(&endpoint, &context, &plan_change);

        let membership = registry
            .membership_for_endpoint(&endpoint)
            .expect("membership");
        let samples = registry.samples_for_pool(&membership.pool.key);
        assert_eq!(samples.len(), 5);
        assert_eq!(samples[0].signature.adjustment_revision, 0);
        assert_eq!(
            samples[1].adjustment,
            Some(QuotaAdjustmentKind::CounterResetOrRollback)
        );
        assert_eq!(samples[1].signature.adjustment_revision, 1);
        assert_eq!(samples[2].adjustment, Some(QuotaAdjustmentKind::TopUp));
        assert_eq!(samples[2].signature.adjustment_revision, 2);
        assert_eq!(
            samples[3].adjustment,
            Some(QuotaAdjustmentKind::CounterResetOrRollback)
        );
        assert_eq!(samples[3].signature.adjustment_revision, 3);
        assert_eq!(
            samples[4].adjustment,
            Some(QuotaAdjustmentKind::LimitOrPlanChanged)
        );
        assert_eq!(samples[4].signature.adjustment_revision, 4);
    }

    #[test]
    fn capacity_evicts_only_inactive_pools_and_keeps_memberships_valid() {
        let mut registry = QuotaPoolRegistry::new(8, 10_000);
        registry.max_pools = 2;
        let endpoint_a = endpoint("relay-a");
        let endpoint_b = endpoint("relay-b");

        let mut context_a = QuotaObservationContext::new("https://relay.example/v1");
        context_a.explicit_pool_id = Some("pool-a".to_string());
        let pool_a = registry
            .record_snapshot(&endpoint_a, &context_a, &snapshot(10, "1"))
            .pool
            .expect("pool a");

        let mut context_b = QuotaObservationContext::new("https://relay.example/v1");
        context_b.explicit_pool_id = Some("pool-b".to_string());
        let pool_b = registry
            .record_snapshot(&endpoint_b, &context_b, &snapshot(20, "1"))
            .pool
            .expect("pool b");

        context_a.explicit_pool_id = Some("pool-c".to_string());
        let pool_c = registry
            .record_snapshot(&endpoint_a, &context_a, &snapshot(30, "2"))
            .pool
            .expect("pool c");

        assert!(registry.pool(&pool_a.key).is_none());
        assert!(registry.pool(&pool_b.key).is_some());
        assert!(registry.pool(&pool_c.key).is_some());
        assert_eq!(registry.pools().len(), 2);
        assert!(
            registry
                .memberships
                .values()
                .all(|membership| { registry.pools.contains_key(&membership.pool.key) })
        );

        let mut checkpoint = registry.checkpoint();
        checkpoint.memberships.insert(
            endpoint("dangling"),
            PoolMembership {
                pool: pool_a,
                endpoint: endpoint("dangling"),
                since_ms: 0,
            },
        );
        let restored =
            QuotaPoolRegistry::from_checkpoint(checkpoint, 8, 10_000).expect("valid checkpoint");
        assert!(
            restored
                .membership_for_endpoint(&endpoint("dangling"))
                .is_none()
        );
        assert!(
            restored
                .memberships
                .values()
                .all(|membership| { restored.pools.contains_key(&membership.pool.key) })
        );
    }

    #[test]
    fn unlimited_quota_is_retained_without_fabricated_amounts() {
        let mut registry = QuotaPoolRegistry::new(8, 10_000);
        let endpoint = endpoint("unlimited");
        let mut context = QuotaObservationContext::new("https://relay.example/v1");
        context.explicit_pool_id = Some("unlimited-pool".to_string());
        context.capabilities.unlimited = true;
        let mut unlimited =
            ProviderBalanceSnapshot::new("relay", endpoint.clone(), "test", 100, None);
        unlimited.status = BalanceSnapshotStatus::Ok;
        unlimited.unlimited_quota = Some(true);

        let update = registry.record_snapshot(&endpoint, &context, &unlimited);

        assert!(update.appended);
        let sample = registry
            .samples_for_pool(&update.pool.expect("pool").key)
            .pop()
            .expect("sample");
        assert!(sample.capabilities.unlimited);
        assert!(sample.used.is_none());
        assert!(sample.remaining.is_none());
        assert!(sample.limit.is_none());
    }

    #[test]
    fn checkpoint_memberships_round_trip_through_json_entries() {
        let endpoint = endpoint("checkpoint-json");
        let mut checkpoint = QuotaRegistryCheckpoint::default();
        checkpoint.memberships.insert(
            endpoint.clone(),
            PoolMembership {
                pool: PoolIdentity::default(),
                endpoint,
                since_ms: 42,
            },
        );

        let json = serde_json::to_string(&checkpoint).expect("serialize checkpoint");
        let restored =
            serde_json::from_str::<QuotaRegistryCheckpoint>(&json).expect("restore checkpoint");

        assert_eq!(restored, checkpoint);
        assert!(json.contains("\"memberships\":[["));
    }

    #[test]
    fn checkpoint_accepts_legacy_empty_membership_object() {
        let restored = serde_json::from_str::<QuotaRegistryCheckpoint>(
            r#"{"schema_version":1,"generation":0,"pools":{},"memberships":{}}"#,
        )
        .expect("restore legacy empty checkpoint");

        assert!(restored.memberships.is_empty());
    }

    #[test]
    fn quantity_decimal_round_trips_as_an_exact_string() {
        let quantity =
            QuotaQuantity::from_decimal("9007199254740993.25", QuotaUnit::Usd).expect("decimal");
        assert_eq!(quantity.value, 900_719_925_474_099_325);
        assert_eq!(quantity.scale, 2);
        let json = serde_json::to_string(&quantity).expect("serialize");
        let restored: QuotaQuantity = serde_json::from_str(&json).expect("deserialize");

        assert!(json.contains(r#""value":"900719925474099325""#));
        assert_eq!(restored, quantity);
    }

    #[test]
    fn quantity_accepts_legacy_integer_json_without_losing_precision() {
        let quantity: QuotaQuantity =
            serde_json::from_str(r#"{"value":9007199254740993,"scale":0,"unit":"usd"}"#)
                .expect("deserialize legacy integer");

        assert_eq!(quantity.value, 9_007_199_254_740_993);
    }

    #[test]
    fn quantity_rejects_float_and_out_of_range_wire_values() {
        for value in [
            "1.5",
            r#""170141183460469231731687303715884105728""#,
            "170141183460469231731687303715884105728",
        ] {
            let json = format!(r#"{{"value":{value},"scale":0,"unit":"usd"}}"#);
            assert!(
                serde_json::from_str::<QuotaQuantity>(&json).is_err(),
                "wire value must be rejected: {value}"
            );
        }
    }

    #[test]
    fn quantities_canonicalize_scale_and_align_safe_arithmetic() {
        let whole = QuotaQuantity::from_decimal("10.000", QuotaUnit::Usd).expect("whole");
        let tenths = QuotaQuantity::from_decimal("0.5", QuotaUnit::Usd).expect("tenths");
        let hundredths = QuotaQuantity::from_decimal("0.25", QuotaUnit::Usd).expect("hundredths");

        assert_eq!(whole, QuotaQuantity::from_integer(10, QuotaUnit::Usd));
        assert_eq!(
            tenths.checked_add(&hundredths),
            QuotaQuantity::from_decimal("0.75", QuotaUnit::Usd)
        );
        assert_eq!(
            whole.checked_sub(&hundredths),
            QuotaQuantity::from_decimal("9.75", QuotaUnit::Usd)
        );
    }

    #[test]
    fn provider_window_hints_keep_calendar_and_rolling_semantics_distinct() {
        let calendar = QuotaWindowSemantics::from_provider_hint(
            Some("daily"),
            None,
            Some("Asia/Shanghai"),
            None,
            None,
        );
        assert_eq!(calendar.kind, QuotaWindowKind::CalendarDay);
        assert_eq!(calendar.reset, QuotaResetKind::ConfiguredCalendarBoundary);
        assert!(calendar.allows_today_label());
        assert!(calendar.allows_midnight_label());

        let rolling = QuotaWindowSemantics::from_provider_hint(
            Some("rolling_24h"),
            Some(86_400_000),
            Some("Asia/Shanghai"),
            Some(0),
            Some(86_400_000),
        );
        assert_eq!(rolling.kind, QuotaWindowKind::Rolling);
        assert_eq!(rolling.reset, QuotaResetKind::ExplicitTimestamp);
        assert_eq!(rolling.rolling_duration_ms, Some(86_400_000));
        assert!(!rolling.allows_today_label());
        assert!(!rolling.allows_midnight_label());

        let resetless =
            QuotaWindowSemantics::from_provider_hint(Some("wallet"), None, None, None, None);
        assert_eq!(resetless.kind, QuotaWindowKind::Resetless);
        assert_eq!(resetless.reset, QuotaResetKind::NoReset);
    }

    #[test]
    fn sampling_deadline_marks_long_gap_as_discontinuity() {
        let mut registry = QuotaPoolRegistry::new(8, 10_000);
        let endpoint = endpoint("relay");
        let mut context = QuotaObservationContext::new("https://relay.example/v1");
        context.explicit_pool_id = Some("pool".to_string());
        context.expected_interval_ms = Some(100);

        registry.record_snapshot(&endpoint, &context, &snapshot(100, "1.0"));
        registry.record_snapshot(&endpoint, &context, &snapshot(401, "2.00"));

        let membership = registry
            .membership_for_endpoint(&endpoint)
            .expect("membership");
        let samples = registry.samples_for_pool(&membership.pool.key);
        assert_eq!(samples[0].used.as_ref().map(|value| value.scale), Some(0));
        assert_eq!(samples[0].sampling.expected_interval_ms, Some(100));
        assert_eq!(samples[0].sampling.fresh_until_ms, Some(400));
        assert_eq!(samples[0].sampling.is_fresh_at(400), Some(true));
        assert_eq!(samples[0].sampling.is_fresh_at(401), Some(false));
        assert_eq!(
            samples[1].adjustment,
            Some(QuotaAdjustmentKind::Discontinuity)
        );
        assert_eq!(samples[1].signature.adjustment_revision, 1);
    }

    #[test]
    fn conversion_source_or_divisor_change_opens_a_new_generation() {
        let configured = QuotaConversion::stable_generation(ConversionSource::Configured, 500_000);
        let remote = QuotaConversion::stable_generation(ConversionSource::Remote, 500_000);
        let changed = QuotaConversion::stable_generation(ConversionSource::Remote, 1_000_000);

        assert_ne!(configured, remote);
        assert_ne!(remote, changed);
    }

    #[test]
    fn configured_reset_uses_fixed_offset_without_claiming_provider_authority() {
        let now_ms = 10 * HOUR_MS;

        assert_eq!(
            next_configured_reset_at_ms(now_ms, "+08:00", "00:00"),
            Some(16 * HOUR_MS)
        );
        assert_eq!(
            next_configured_reset_at_ms(now_ms, "invalid", "00:00"),
            None
        );
        assert_eq!(next_configured_reset_at_ms(now_ms, "+08:00", "24:00"), None);
    }
}
