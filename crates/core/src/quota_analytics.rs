//! Pure quota-pool rate, pacing, and project-reconciliation analytics.

use std::collections::BTreeMap;

use chrono::{DateTime, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::quota_pool::{
    NormalizationSignature, PoolIdentity, QuotaAdjustmentKind, QuotaCapabilities, QuotaConversion,
    QuotaObservation, QuotaPoolState, QuotaQuantity, QuotaRegistryCheckpoint, QuotaUnit,
    QuotaWindowKind, QuotaWindowSemantics,
};
use crate::runtime_identity::ProviderEndpointKey;
use crate::sessions::{ProjectIdentity, ProjectIdentityKind};
use crate::state::{
    AttributionCoverage, AttributionPoolKey, AttributionQuery, AttributionQueryResult,
};

const FEMTO_USD_PER_USD: i128 = 1_000_000_000_000_000;

/// An exact signed reconciliation difference whose JSON form is a decimal string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct SignedUsdDelta {
    femto_usd: i128,
}

impl SignedUsdDelta {
    pub const ZERO: Self = Self { femto_usd: 0 };

    pub const fn from_femto_usd(femto_usd: i128) -> Self {
        Self { femto_usd }
    }

    pub fn from_decimal_str(value: &str) -> Option<Self> {
        parse_signed_usd_delta(value).map(Self::from_femto_usd)
    }

    pub const fn femto_usd(self) -> i128 {
        self.femto_usd
    }

    pub const fn is_zero(self) -> bool {
        self.femto_usd == 0
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.femto_usd
            .checked_sub(other.femto_usd)
            .map(Self::from_femto_usd)
    }

    pub fn format_usd(self) -> String {
        let negative = self.femto_usd < 0;
        let magnitude = self.femto_usd.unsigned_abs();
        let divisor = FEMTO_USD_PER_USD as u128;
        let whole = magnitude / divisor;
        let fraction = magnitude % divisor;
        let prefix = if negative { "-" } else { "" };
        if fraction == 0 {
            return format!("{prefix}{whole}");
        }
        let mut fraction = format!("{fraction:015}");
        while fraction.ends_with('0') {
            fraction.pop();
        }
        format!("{prefix}{whole}.{fraction}")
    }
}

impl Serialize for SignedUsdDelta {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.format_usd())
    }
}

impl<'de> Deserialize<'de> for SignedUsdDelta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_decimal_str(&value)
            .ok_or_else(|| serde::de::Error::custom("invalid signed USD decimal"))
    }
}

fn parse_signed_usd_delta(value: &str) -> Option<i128> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 || value.contains(['e', 'E']) {
        return None;
    }
    let negative = value.starts_with('-');
    let unsigned = value.strip_prefix(['-', '+']).unwrap_or(value);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if unsigned.matches('.').count() > 1
        || whole.is_empty() && fraction.is_empty()
        || fraction.len() > 15
        || !whole.chars().all(|character| character.is_ascii_digit())
        || !fraction.chars().all(|character| character.is_ascii_digit())
    {
        return None;
    }

    let mut scaled = String::with_capacity(whole.len().max(1) + 15);
    scaled.push_str(if whole.is_empty() { "0" } else { whole });
    scaled.push_str(fraction);
    scaled.extend(std::iter::repeat_n('0', 15 - fraction.len()));
    let scaled = scaled.trim_start_matches('0');
    if scaled.is_empty() {
        return Some(0);
    }
    if negative {
        format!("-{scaled}").parse().ok()
    } else {
        scaled.parse().ok()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaAnalyticsSupport {
    #[default]
    Unsupported,
    Supported,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaRateStatus {
    Available,
    #[default]
    InsufficientSamples,
    ShortSpan,
    Stale,
    Gap,
    Adjustment,
    NegativeDelta,
    Unordered,
    NoCounter,
    Overflow,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPaceStatus {
    Unlimited,
    Faster,
    OnPace,
    Slower,
    NoReset,
    ResetUnknown,
    LowSample,
    Stale,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaFreshnessStatus {
    Fresh,
    Stale,
    Offline,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaReconciliationStatus {
    Available,
    IncompleteCoverage,
    StaleRemote,
    IncompatibleUnit,
    IncompatibleGeneration,
    WindowMismatch,
    NoRemoteDelta,
    Overflow,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct QuotaRateWindow {
    pub status: QuotaRateStatus,
    pub rate_per_hour: Option<QuotaQuantity>,
    pub lower_bound: bool,
    pub sample_count: usize,
    pub span_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct QuotaPacingView {
    pub status: QuotaPaceStatus,
    pub required_rate_per_hour: Option<QuotaQuantity>,
    /// 10_000 means exactly on the required pace.
    pub pace_ratio_basis_points: Option<u32>,
    pub exhaustion_eta_ms: Option<u64>,
    pub projected_remaining_at_reset: Option<QuotaQuantity>,
    pub reset_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct QuotaProjectRow {
    pub project: ProjectIdentity,
    pub local_cost: QuotaQuantity,
    pub requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct QuotaReconciliationView {
    pub status: QuotaReconciliationStatus,
    pub remote_total: Option<QuotaQuantity>,
    pub local_known: Option<QuotaQuantity>,
    pub local_unknown: Option<QuotaQuantity>,
    pub external_unattributed: Option<QuotaQuantity>,
    pub signed_delta: Option<SignedUsdDelta>,
    pub projects: Vec<QuotaProjectRow>,
    pub omitted_projects: usize,
    pub omitted_local_known: Option<QuotaQuantity>,
    pub coverage: AttributionCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct PoolQuotaAnalytics {
    pub identity: PoolIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<ProviderEndpointKey>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub observation_provider_id: String,
    pub observed_at_ms: u64,
    pub last_success_at_ms: Option<u64>,
    pub last_attempt_at_ms: Option<u64>,
    pub freshness: QuotaFreshnessStatus,
    pub latest_adjustment: Option<QuotaAdjustmentKind>,
    pub source: String,
    pub unit: QuotaUnit,
    pub conversion: Option<QuotaConversion>,
    pub capabilities: QuotaCapabilities,
    pub window: QuotaWindowSemantics,
    pub epoch_start_ms: u64,
    pub epoch_end_ms: Option<u64>,
    pub remote_used: Option<QuotaQuantity>,
    pub remote_direct_total: Option<QuotaQuantity>,
    pub remote_remaining: Option<QuotaQuantity>,
    pub remote_limit: Option<QuotaQuantity>,
    pub observed_burn: Option<QuotaQuantity>,
    pub rate_15m: QuotaRateWindow,
    pub rate_60m: QuotaRateWindow,
    pub pacing: QuotaPacingView,
    pub reconciliation: QuotaReconciliationView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct QuotaAnalyticsView {
    pub support: QuotaAnalyticsSupport,
    pub generated_at_ms: u64,
    pub registry_generation: u64,
    pub pools: Vec<PoolQuotaAnalytics>,
    pub omitted_pools: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct PoolAttributionWindow {
    pub pool_key: String,
    pub pool_revision: u64,
    pub conversion_generation: Option<u64>,
    pub unit: QuotaUnit,
    pub query: AttributionQuery,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct PoolAttributionResult {
    pub window: PoolAttributionWindow,
    pub attribution: AttributionQueryResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReconciliationWindowBounds {
    start_ms: u64,
    end_ms: u64,
}

fn reconciliation_window_bounds(
    latest: &QuotaObservation,
    observed_start_ms: u64,
    now_ms: u64,
) -> Option<ReconciliationWindowBounds> {
    let start_ms = latest
        .direct_total
        .as_ref()
        .and(latest.window_start_ms)
        .unwrap_or(observed_start_ms);
    let end_ms = [
        Some(latest.observed_at_ms),
        Some(now_ms),
        latest.signature.window_end_ms,
        latest.signature.reset_at_ms,
    ]
    .into_iter()
    .flatten()
    .min()?;
    (start_ms < end_ms).then_some(ReconciliationWindowBounds { start_ms, end_ms })
}

pub fn plan_quota_attribution(
    checkpoint: &QuotaRegistryCheckpoint,
    now_ms: u64,
) -> Vec<PoolAttributionWindow> {
    checkpoint
        .pools
        .values()
        .filter(|pool| pool.identity.aggregation_eligible)
        .filter_map(|pool| {
            let latest = pool.samples.back()?;
            let epoch = latest_epoch_samples(pool.samples.iter(), &latest.signature);
            let observed_start_ms = epoch.first()?.observed_at_ms;
            let bounds = reconciliation_window_bounds(latest, observed_start_ms, now_ms)?;
            let endpoints = checkpoint
                .memberships
                .values()
                .filter(|membership| {
                    membership.pool.key == pool.identity.key
                        && membership.pool.revision == pool.identity.revision
                })
                .map(|membership| membership.endpoint.clone())
                .collect::<Vec<_>>();
            if endpoints.is_empty() {
                return None;
            }
            let query = AttributionQuery::new(bounds.start_ms, bounds.end_ms)
                .for_pool(AttributionPoolKey {
                    pool_key: pool.identity.key.clone(),
                    revision: pool.identity.revision,
                })
                .for_endpoints(endpoints);
            Some(PoolAttributionWindow {
                pool_key: pool.identity.key.clone(),
                pool_revision: pool.identity.revision,
                conversion_generation: latest.signature.conversion_generation,
                unit: latest.signature.unit,
                query,
            })
        })
        .collect()
}

pub fn build_quota_analytics(
    checkpoint: &QuotaRegistryCheckpoint,
    now_ms: u64,
    attribution: &[PoolAttributionResult],
) -> QuotaAnalyticsView {
    let mut pools = checkpoint
        .pools
        .values()
        .filter_map(|pool| build_pool_analytics(pool, now_ms, attribution))
        .collect::<Vec<_>>();
    pools.sort_by(|left, right| left.identity.key.cmp(&right.identity.key));
    let omitted_pools = pools.len().saturating_sub(MAX_POOL_ROWS);
    pools.truncate(MAX_POOL_ROWS);
    QuotaAnalyticsView {
        support: QuotaAnalyticsSupport::Supported,
        generated_at_ms: now_ms,
        registry_generation: checkpoint.generation,
        pools,
        omitted_pools,
    }
}

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const RATE_EXTRA_SCALE: u32 = 6;
const FEMTO_USD_SCALE: u32 = 15;
const PACE_RATIO_SCALE: i128 = 10_000;
const MAX_POOL_ROWS: usize = 128;
const MAX_PROJECT_ROWS: usize = 256;

fn build_pool_analytics(
    pool: &QuotaPoolState,
    now_ms: u64,
    attribution: &[PoolAttributionResult],
) -> Option<PoolQuotaAnalytics> {
    let latest = pool.samples.back()?;
    let epoch = latest_epoch_samples(pool.samples.iter(), &latest.signature);
    let observed_start_ms = epoch.first()?.observed_at_ms;
    let epoch_start_ms = latest
        .direct_total
        .as_ref()
        .and(latest.window_start_ms)
        .unwrap_or(observed_start_ms);
    let epoch_end_ms = [latest.signature.window_end_ms, latest.signature.reset_at_ms]
        .into_iter()
        .flatten()
        .min();
    let rate_15m = rate_window(&epoch, now_ms, 15 * MINUTE_MS, 10 * MINUTE_MS);
    let rate_60m = rate_window(&epoch, now_ms, 60 * MINUTE_MS, 30 * MINUTE_MS);
    let observed_burn = observed_burn(&epoch);
    let remote_for_reconciliation = latest
        .direct_total
        .clone()
        .filter(|_| latest.window_start_ms.is_some())
        .or_else(|| observed_burn.clone());
    let expected_window =
        reconciliation_window_bounds(latest, observed_start_ms, now_ms).map(|bounds| {
            PoolAttributionWindow {
                pool_key: pool.identity.key.clone(),
                pool_revision: pool.identity.revision,
                conversion_generation: latest.signature.conversion_generation,
                unit: latest.signature.unit,
                query: AttributionQuery::new(bounds.start_ms, bounds.end_ms).for_pool(
                    AttributionPoolKey {
                        pool_key: pool.identity.key.clone(),
                        revision: pool.identity.revision,
                    },
                ),
            }
        });
    let matching_attribution = expected_window.as_ref().and_then(|expected_window| {
        attribution
            .iter()
            .find(|candidate| attribution_result_compatible(expected_window, candidate))
            .or_else(|| {
                attribution.iter().find(|candidate| {
                    candidate.window.pool_key == pool.identity.key
                        && candidate.window.pool_revision == pool.identity.revision
                })
            })
    });
    let reconciliation = match expected_window.as_ref() {
        Some(expected_window) => build_reconciliation(
            remote_for_reconciliation,
            expected_window,
            matching_attribution,
            observation_is_fresh(latest, now_ms),
        ),
        None => QuotaReconciliationView {
            remote_total: remote_for_reconciliation,
            ..QuotaReconciliationView::default()
        },
    };
    Some(PoolQuotaAnalytics {
        identity: pool.identity.clone(),
        endpoint: latest.endpoint.clone(),
        observation_provider_id: latest.observation_provider_id.clone(),
        observed_at_ms: latest.observed_at_ms,
        last_success_at_ms: pool.last_success_at_ms,
        last_attempt_at_ms: pool.last_attempt_at_ms,
        freshness: quota_freshness(pool, latest, now_ms),
        latest_adjustment: latest.adjustment,
        source: latest.source.clone(),
        unit: latest.signature.unit,
        conversion: latest.conversion.clone(),
        capabilities: latest.capabilities.clone(),
        window: latest.window.clone(),
        epoch_start_ms,
        epoch_end_ms,
        remote_used: latest.used.clone(),
        remote_direct_total: latest.direct_total.clone(),
        remote_remaining: latest.remaining.clone(),
        remote_limit: latest.limit.clone(),
        observed_burn: observed_burn.clone(),
        pacing: build_pacing(latest, &rate_60m, now_ms),
        reconciliation,
        rate_15m,
        rate_60m,
    })
}

fn latest_epoch_samples<'a>(
    samples: impl DoubleEndedIterator<Item = &'a QuotaObservation>,
    signature: &NormalizationSignature,
) -> Vec<&'a QuotaObservation> {
    let mut epoch = samples
        .rev()
        .take_while(|sample| sample.signature == *signature)
        .collect::<Vec<_>>();
    epoch.reverse();
    epoch
}

fn rate_window(
    epoch: &[&QuotaObservation],
    now_ms: u64,
    window_ms: u64,
    minimum_span_ms: u64,
) -> QuotaRateWindow {
    let Some(epoch_latest) = epoch.last().copied() else {
        return QuotaRateWindow::default();
    };
    if epoch_latest.observed_at_ms > now_ms {
        return QuotaRateWindow {
            status: QuotaRateStatus::Unordered,
            ..QuotaRateWindow::default()
        };
    }
    if !observation_is_fresh(epoch_latest, now_ms) {
        return QuotaRateWindow {
            status: QuotaRateStatus::Stale,
            ..QuotaRateWindow::default()
        };
    }
    let start_ms = now_ms.saturating_sub(window_ms);
    let samples = epoch
        .iter()
        .copied()
        .filter(|sample| sample.observed_at_ms >= start_ms && sample.observed_at_ms <= now_ms)
        .collect::<Vec<_>>();
    let mut result = QuotaRateWindow {
        sample_count: samples.len(),
        ..QuotaRateWindow::default()
    };
    let Some(latest) = samples.last().copied() else {
        return result;
    };
    if samples.len() < 3 {
        return result;
    }
    for pair in samples.windows(2) {
        if pair[1].observed_at_ms <= pair[0].observed_at_ms {
            result.status = QuotaRateStatus::Unordered;
            return result;
        }
        if pair_exceeds_continuity(pair[0], pair[1]) {
            result.status = QuotaRateStatus::Gap;
            return result;
        }
    }
    if samples
        .iter()
        .skip(1)
        .any(|sample| sample.adjustment.is_some())
    {
        result.status = QuotaRateStatus::Adjustment;
        return result;
    }
    let first = samples[0];
    let span_ms = latest.observed_at_ms.saturating_sub(first.observed_at_ms);
    result.span_ms = span_ms;
    if span_ms < minimum_span_ms {
        result.status = QuotaRateStatus::ShortSpan;
        return result;
    }

    let (delta, lower_bound) = match counter_delta(&samples) {
        Ok(delta) => delta,
        Err(status) => {
            result.status = status;
            return result;
        }
    };
    let Some(rate_per_hour) = rate_for_span(&delta, span_ms) else {
        result.status = QuotaRateStatus::Overflow;
        return result;
    };
    result.status = QuotaRateStatus::Available;
    result.lower_bound = lower_bound;
    result.rate_per_hour = Some(rate_per_hour);
    result
}

fn observation_is_fresh(sample: &QuotaObservation, now_ms: u64) -> bool {
    if !sample.fresh {
        return false;
    }
    if let Some(fresh) = sample.sampling.is_fresh_at(now_ms) {
        return fresh;
    }
    sample
        .sampling
        .expected_interval_ms
        .and_then(|interval| interval.checked_mul(3))
        .and_then(|horizon| sample.observed_at_ms.checked_add(horizon))
        .is_none_or(|deadline| now_ms <= deadline)
}

fn pair_exceeds_continuity(previous: &QuotaObservation, current: &QuotaObservation) -> bool {
    let deadline = previous.sampling.continuity_deadline_ms.or_else(|| {
        previous
            .sampling
            .expected_interval_ms
            .and_then(|interval| interval.checked_mul(3))
            .and_then(|horizon| previous.observed_at_ms.checked_add(horizon))
    });
    deadline.is_some_and(|deadline| current.observed_at_ms > deadline)
}

fn counter_delta(samples: &[&QuotaObservation]) -> Result<(QuotaQuantity, bool), QuotaRateStatus> {
    let use_used = samples.iter().all(|sample| sample.used.is_some());
    let use_remaining = !use_used && samples.iter().all(|sample| sample.remaining.is_some());
    if !use_used && !use_remaining {
        return Err(QuotaRateStatus::NoCounter);
    }
    let quantities = samples
        .iter()
        .map(|sample| {
            if use_used {
                sample.used.as_ref()
            } else {
                sample.remaining.as_ref()
            }
            .expect("counter presence checked above")
        })
        .collect::<Vec<_>>();
    for pair in quantities.windows(2) {
        let Some((previous, current, _)) = aligned_values(pair[0], pair[1]) else {
            return Err(QuotaRateStatus::Overflow);
        };
        let monotonic = if use_used {
            current >= previous
        } else {
            current <= previous
        };
        if !monotonic {
            return Err(QuotaRateStatus::NegativeDelta);
        }
    }
    let first = quantities[0];
    let latest = quantities[quantities.len() - 1];
    let delta = if use_used {
        latest.checked_sub(first)
    } else {
        first.checked_sub(latest)
    }
    .ok_or(QuotaRateStatus::Overflow)?;
    Ok((delta, use_remaining))
}

fn observed_burn(epoch: &[&QuotaObservation]) -> Option<QuotaQuantity> {
    if epoch.len() < 2
        || epoch
            .windows(2)
            .any(|pair| pair[1].observed_at_ms <= pair[0].observed_at_ms)
        || epoch
            .iter()
            .skip(1)
            .any(|sample| sample.adjustment.is_some())
    {
        return None;
    }
    counter_delta(epoch).ok().map(|(delta, _)| delta)
}

fn rate_for_span(delta: &QuotaQuantity, span_ms: u64) -> Option<QuotaQuantity> {
    if span_ms == 0 {
        return None;
    }
    let precision = 10_i128.checked_pow(RATE_EXTRA_SCALE)?;
    let value = delta
        .value
        .checked_mul(precision)?
        .checked_mul(HOUR_MS as i128)?
        .checked_div(span_ms as i128)?;
    Some(
        QuotaQuantity::new(
            value,
            delta.scale.checked_add(RATE_EXTRA_SCALE)?,
            delta.unit,
        )
        .with_conversion_generation(delta.conversion_generation),
    )
}

fn aligned_values(left: &QuotaQuantity, right: &QuotaQuantity) -> Option<(i128, i128, u32)> {
    if left.unit != right.unit || left.conversion_generation != right.conversion_generation {
        return None;
    }
    let scale = left.scale.max(right.scale);
    let left = left
        .value
        .checked_mul(10_i128.checked_pow(scale.checked_sub(left.scale)?)?)?;
    let right = right
        .value
        .checked_mul(10_i128.checked_pow(scale.checked_sub(right.scale)?)?)?;
    Some((left, right, scale))
}

fn build_pacing(
    latest: &QuotaObservation,
    rate_60m: &QuotaRateWindow,
    now_ms: u64,
) -> QuotaPacingView {
    let mut pacing = QuotaPacingView {
        reset_at_ms: latest
            .reset_at_ms
            .or(latest.signature.reset_at_ms)
            .or_else(|| configured_calendar_reset_at_ms(&latest.window, now_ms)),
        ..QuotaPacingView::default()
    };
    if latest.capabilities.unlimited {
        pacing.status = QuotaPaceStatus::Unlimited;
        pacing.reset_at_ms = None;
        return pacing;
    }
    let Some(rate) = rate_60m
        .rate_per_hour
        .as_ref()
        .filter(|_| rate_60m.status == QuotaRateStatus::Available)
    else {
        pacing.status = match rate_60m.status {
            QuotaRateStatus::Stale => QuotaPaceStatus::Stale,
            QuotaRateStatus::InsufficientSamples | QuotaRateStatus::ShortSpan => {
                QuotaPaceStatus::LowSample
            }
            _ => QuotaPaceStatus::Unavailable,
        };
        return pacing;
    };
    let Some(remaining) = observation_remaining(latest) else {
        return pacing;
    };
    pacing.exhaustion_eta_ms = duration_for_quantity(&remaining, rate);

    if latest.window.kind == QuotaWindowKind::Resetless {
        pacing.status = QuotaPaceStatus::NoReset;
        pacing.reset_at_ms = None;
        return pacing;
    }
    let Some(reset_at_ms) = pacing.reset_at_ms else {
        pacing.status = QuotaPaceStatus::ResetUnknown;
        return pacing;
    };
    let Some(reset_in_ms) = reset_at_ms.checked_sub(now_ms).filter(|value| *value > 0) else {
        pacing.status = QuotaPaceStatus::Unavailable;
        return pacing;
    };
    let Some(required_rate) = rate_for_span(&remaining, reset_in_ms) else {
        return pacing;
    };
    let Some(ratio) = quantity_ratio_basis_points(rate, &required_rate) else {
        return pacing;
    };
    pacing.status = if ratio < 8_000 {
        QuotaPaceStatus::Slower
    } else if ratio > 12_000 {
        QuotaPaceStatus::Faster
    } else {
        QuotaPaceStatus::OnPace
    };
    pacing.required_rate_per_hour = Some(required_rate);
    pacing.pace_ratio_basis_points = Some(ratio);
    pacing.projected_remaining_at_reset = projected_remaining(&remaining, rate, reset_in_ms);
    pacing
}

fn quota_freshness(
    pool: &QuotaPoolState,
    latest: &QuotaObservation,
    now_ms: u64,
) -> QuotaFreshnessStatus {
    if observation_is_fresh(latest, now_ms) {
        return QuotaFreshnessStatus::Fresh;
    }
    if pool
        .last_attempt_at_ms
        .zip(pool.last_success_at_ms)
        .is_some_and(|(attempt, success)| attempt > success)
    {
        QuotaFreshnessStatus::Offline
    } else {
        QuotaFreshnessStatus::Stale
    }
}

fn configured_calendar_reset_at_ms(window: &QuotaWindowSemantics, now_ms: u64) -> Option<u64> {
    if window.kind != QuotaWindowKind::CalendarDay
        || window.reset != crate::quota_pool::QuotaResetKind::ConfiguredCalendarBoundary
    {
        return None;
    }
    let timezone = window.reset_timezone.as_deref()?.parse::<Tz>().ok()?;
    let now_timestamp_ms = i64::try_from(now_ms).ok()?;
    let now = DateTime::<Utc>::from_timestamp_millis(now_timestamp_ms)?;
    let next_date = now.with_timezone(&timezone).date_naive().succ_opt()?;
    let next_midnight = next_date.and_hms_opt(0, 0, 0)?;
    let reset = match timezone.from_local_datetime(&next_midnight) {
        LocalResult::Single(reset) => reset,
        LocalResult::Ambiguous(first, second) => first.min(second),
        LocalResult::None => return None,
    };
    let reset_at_ms = u64::try_from(reset.timestamp_millis()).ok()?;
    (reset_at_ms > now_ms).then_some(reset_at_ms)
}

fn observation_remaining(observation: &QuotaObservation) -> Option<QuotaQuantity> {
    if let Some(remaining) = observation.remaining.as_ref() {
        return (remaining.value >= 0).then(|| remaining.clone());
    }
    let remaining = observation
        .limit
        .as_ref()?
        .checked_sub(observation.used.as_ref()?)?;
    (remaining.value >= 0).then_some(remaining)
}

fn quantity_ratio_basis_points(
    numerator: &QuotaQuantity,
    denominator: &QuotaQuantity,
) -> Option<u32> {
    let (numerator, denominator, _) = aligned_values(numerator, denominator)?;
    if numerator < 0 || denominator <= 0 {
        return None;
    }
    let ratio = numerator
        .checked_mul(PACE_RATIO_SCALE)?
        .checked_div(denominator)?;
    u32::try_from(ratio).ok()
}

fn duration_for_quantity(remaining: &QuotaQuantity, rate_per_hour: &QuotaQuantity) -> Option<u64> {
    let (remaining, rate, _) = aligned_values(remaining, rate_per_hour)?;
    if remaining < 0 || rate <= 0 {
        return None;
    }
    let duration = remaining.checked_mul(HOUR_MS as i128)?.checked_div(rate)?;
    u64::try_from(duration).ok()
}

fn projected_remaining(
    remaining: &QuotaQuantity,
    rate_per_hour: &QuotaQuantity,
    duration_ms: u64,
) -> Option<QuotaQuantity> {
    let precision = 10_i128.checked_pow(RATE_EXTRA_SCALE)?;
    let projected_burn_value = rate_per_hour
        .value
        .checked_mul(precision)?
        .checked_mul(duration_ms as i128)?
        .checked_div(HOUR_MS as i128)?;
    let projected_burn = QuotaQuantity::new(
        projected_burn_value,
        rate_per_hour.scale.checked_add(RATE_EXTRA_SCALE)?,
        rate_per_hour.unit,
    )
    .with_conversion_generation(rate_per_hour.conversion_generation);
    let projected = remaining.checked_sub(&projected_burn)?;
    Some(if projected.value < 0 {
        QuotaQuantity::new(0, 0, remaining.unit)
            .with_conversion_generation(remaining.conversion_generation)
    } else {
        projected
    })
}

fn build_reconciliation(
    remote_total: Option<QuotaQuantity>,
    expected_window: &PoolAttributionWindow,
    result: Option<&PoolAttributionResult>,
    remote_fresh: bool,
) -> QuotaReconciliationView {
    let Some(result) = result else {
        return QuotaReconciliationView {
            remote_total,
            ..QuotaReconciliationView::default()
        };
    };
    let mut view = QuotaReconciliationView {
        remote_total: remote_total.clone(),
        coverage: result.attribution.coverage.clone(),
        ..QuotaReconciliationView::default()
    };
    if !attribution_result_compatible(expected_window, result) {
        view.status = QuotaReconciliationStatus::WindowMismatch;
        return view;
    }

    let Some((projects, local_known, local_unknown)) =
        aggregate_local_projects(&result.attribution)
    else {
        view.status = QuotaReconciliationStatus::Overflow;
        return view;
    };
    let mut projects = projects;
    projects.sort_by(|left, right| {
        quantity_to_femto_usd(&right.local_cost)
            .cmp(&quantity_to_femto_usd(&left.local_cost))
            .then_with(|| left.project.cmp(&right.project))
    });
    let omitted_projects = projects.len().saturating_sub(MAX_PROJECT_ROWS);
    let omitted_femto = projects
        .iter()
        .skip(MAX_PROJECT_ROWS)
        .try_fold(0_i128, |sum, row| {
            sum.checked_add(quantity_to_femto_usd(&row.local_cost)?)
        });
    projects.truncate(MAX_PROJECT_ROWS);
    view.projects = projects;
    view.omitted_projects = omitted_projects;
    view.omitted_local_known = omitted_femto
        .filter(|_| omitted_projects > 0)
        .map(femto_usd_quantity);
    view.local_known = Some(femto_usd_quantity(local_known));
    view.local_unknown = Some(femto_usd_quantity(local_unknown));

    if !remote_fresh {
        view.status = QuotaReconciliationStatus::StaleRemote;
        return view;
    }
    let Some(remote_total) = remote_total else {
        view.status = QuotaReconciliationStatus::NoRemoteDelta;
        return view;
    };
    if remote_total.unit != QuotaUnit::Usd || expected_window.unit != QuotaUnit::Usd {
        view.status = QuotaReconciliationStatus::IncompatibleUnit;
        return view;
    }
    if remote_total.conversion_generation != expected_window.conversion_generation {
        view.status = QuotaReconciliationStatus::IncompatibleGeneration;
        return view;
    }
    if !result.attribution.coverage.complete_for_reconciliation() {
        view.status = QuotaReconciliationStatus::IncompleteCoverage;
        return view;
    }
    let Some(local_total) = local_known.checked_add(local_unknown) else {
        view.status = QuotaReconciliationStatus::Overflow;
        return view;
    };
    let Some(remote_femto) = quantity_to_femto_usd(&remote_total) else {
        view.status = QuotaReconciliationStatus::Overflow;
        return view;
    };
    let Some(delta) = remote_femto
        .checked_sub(local_total)
        .map(SignedUsdDelta::from_femto_usd)
    else {
        view.status = QuotaReconciliationStatus::Overflow;
        return view;
    };
    view.status = QuotaReconciliationStatus::Available;
    view.external_unattributed = Some(femto_usd_quantity(delta.femto_usd().max(0)));
    view.signed_delta = Some(delta);
    view
}

fn attribution_windows_compatible(
    expected: &PoolAttributionWindow,
    actual: &PoolAttributionWindow,
) -> bool {
    expected.pool_key == actual.pool_key
        && expected.pool_revision == actual.pool_revision
        && expected.conversion_generation == actual.conversion_generation
        && expected.unit == actual.unit
        && expected.query.start_ms == actual.query.start_ms
        && expected.query.end_ms == actual.query.end_ms
        && actual.query.pool
            == Some(AttributionPoolKey {
                pool_key: expected.pool_key.clone(),
                revision: expected.pool_revision,
            })
}

fn attribution_result_compatible(
    expected: &PoolAttributionWindow,
    actual: &PoolAttributionResult,
) -> bool {
    attribution_windows_compatible(expected, &actual.window)
        && actual.attribution.start_ms == expected.query.start_ms
        && actual.attribution.end_ms == expected.query.end_ms
}

fn aggregate_local_projects(
    attribution: &AttributionQueryResult,
) -> Option<(Vec<QuotaProjectRow>, i128, i128)> {
    let mut projects = BTreeMap::<ProjectIdentity, (i128, u64)>::new();
    for row in &attribution.rows {
        let cost = row.aggregate.checked_cost_femto_usd()?;
        if cost < 0 {
            return None;
        }
        let aggregate = projects.entry(row.key.project.clone()).or_default();
        aggregate.0 = aggregate.0.checked_add(cost)?;
        aggregate.1 = aggregate.1.checked_add(row.aggregate.requests)?;
    }
    let mut local_known = 0_i128;
    let mut local_unknown = 0_i128;
    let mut rows = Vec::new();
    for (project, (cost, requests)) in projects {
        if project.kind == ProjectIdentityKind::Unknown {
            local_unknown = local_unknown.checked_add(cost)?;
        } else {
            local_known = local_known.checked_add(cost)?;
            rows.push(QuotaProjectRow {
                project,
                local_cost: femto_usd_quantity(cost),
                requests,
            });
        }
    }
    if attribution.checked_total_femto_usd()? != local_known.checked_add(local_unknown)? {
        return None;
    }
    Some((rows, local_known, local_unknown))
}

fn quantity_to_femto_usd(quantity: &QuotaQuantity) -> Option<i128> {
    if quantity.unit != QuotaUnit::Usd || quantity.value < 0 {
        return None;
    }
    if quantity.scale <= FEMTO_USD_SCALE {
        return quantity
            .value
            .checked_mul(10_i128.checked_pow(FEMTO_USD_SCALE - quantity.scale)?);
    }
    let divisor = 10_i128.checked_pow(quantity.scale - FEMTO_USD_SCALE)?;
    (quantity.value % divisor == 0).then(|| quantity.value / divisor)
}

fn femto_usd_quantity(value: i128) -> QuotaQuantity {
    QuotaQuantity::new(value.max(0), FEMTO_USD_SCALE, QuotaUnit::Usd)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, VecDeque};

    use chrono::DateTime;

    use super::*;
    use crate::quota_pool::{
        IdentityConfidence, IdentityEvidence, NormalizationSignature, PoolIdentity,
        QuotaCounterKind, QuotaObservation, QuotaPoolState, QuotaScope,
    };
    use crate::sessions::{ProjectIdentity, ProjectIdentityKind};
    use crate::state::{
        AccountingPriceCoverage, AttributionAggregate, AttributionBucket, AttributionBucketKey,
    };

    const MINUTE_MS: u64 = 60_000;

    #[test]
    fn configured_calendar_timezone_handles_dst_day_lengths() {
        let rate = QuotaRateWindow {
            status: QuotaRateStatus::Available,
            rate_per_hour: Some(QuotaQuantity::from_integer(1, QuotaUnit::Usd)),
            ..QuotaRateWindow::default()
        };
        for (now, expected_reset, expected_duration_hours) in [
            ("2026-03-08T05:00:00Z", "2026-03-09T04:00:00Z", 23_u64),
            ("2026-11-01T04:00:00Z", "2026-11-02T05:00:00Z", 25_u64),
        ] {
            let now_ms = DateTime::parse_from_rfc3339(now)
                .expect("valid now")
                .timestamp_millis() as u64;
            let expected_reset_ms = DateTime::parse_from_rfc3339(expected_reset)
                .expect("valid reset")
                .timestamp_millis() as u64;
            let mut latest = used_sample(now_ms, 0, expected_reset_ms);
            latest.remaining = Some(QuotaQuantity::from_integer(100, QuotaUnit::Usd));
            latest.window = QuotaWindowSemantics {
                kind: QuotaWindowKind::CalendarDay,
                reset: crate::quota_pool::QuotaResetKind::ConfiguredCalendarBoundary,
                reset_timezone: Some("America/New_York".to_string()),
                ..QuotaWindowSemantics::default()
            };
            latest.signature.window = latest.window.clone();

            let pacing = build_pacing(&latest, &rate, now_ms);

            assert_eq!(pacing.reset_at_ms, Some(expected_reset_ms));
            assert_eq!(
                pacing.reset_at_ms.expect("reset") - now_ms,
                expected_duration_hours * HOUR_MS
            );
        }
    }

    #[test]
    fn explicit_remote_reset_outranks_configured_timezone() {
        let now_ms = DateTime::parse_from_rfc3339("2026-03-08T05:00:00Z")
            .expect("valid now")
            .timestamp_millis() as u64;
        let explicit_reset_ms = now_ms + 2 * HOUR_MS;
        let rate = QuotaRateWindow {
            status: QuotaRateStatus::Available,
            rate_per_hour: Some(QuotaQuantity::from_integer(1, QuotaUnit::Usd)),
            ..QuotaRateWindow::default()
        };
        let mut latest = used_sample(now_ms, 0, explicit_reset_ms);
        latest.remaining = Some(QuotaQuantity::from_integer(100, QuotaUnit::Usd));
        latest.reset_at_ms = Some(explicit_reset_ms);
        latest.signature.reset_at_ms = Some(explicit_reset_ms);
        latest.window = QuotaWindowSemantics {
            kind: QuotaWindowKind::CalendarDay,
            reset: crate::quota_pool::QuotaResetKind::ExplicitTimestamp,
            reset_timezone: Some("America/New_York".to_string()),
            ..QuotaWindowSemantics::default()
        };
        latest.signature.window = latest.window.clone();

        let pacing = build_pacing(&latest, &rate, now_ms);

        assert_eq!(pacing.reset_at_ms, Some(explicit_reset_ms));
    }

    #[test]
    fn unlimited_pool_has_explicit_pacing_status_without_a_target() {
        let now_ms = 60 * MINUTE_MS;
        let mut sample = used_sample(now_ms, 0, now_ms);
        sample.used = None;
        sample.capabilities.unlimited = true;
        let checkpoint = checkpoint_with_samples([sample]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[]);
        let pool = &view.pools[0];

        assert_eq!(pool.pacing.status, QuotaPaceStatus::Unlimited);
        assert_eq!(pool.pacing.required_rate_per_hour, None);
        assert_eq!(pool.pacing.reset_at_ms, None);
    }

    #[test]
    fn failed_attempt_after_cached_sample_is_offline_not_zero() {
        let now_ms = 60 * MINUTE_MS;
        let mut checkpoint =
            checkpoint_with_samples([used_sample(now_ms - 10 * MINUTE_MS, 10, now_ms - MINUTE_MS)]);
        let pool = checkpoint.pools.get_mut("pool-a").expect("pool");
        pool.last_success_at_ms = Some(now_ms - 10 * MINUTE_MS);
        pool.last_attempt_at_ms = Some(now_ms);

        let view = build_quota_analytics(&checkpoint, now_ms, &[]);

        assert_eq!(view.pools[0].freshness, QuotaFreshnessStatus::Offline);
        assert_eq!(
            view.pools[0].remote_used,
            Some(QuotaQuantity::from_integer(10, QuotaUnit::Usd))
        );
    }

    #[test]
    fn same_epoch_points_produce_exact_fifteen_and_sixty_minute_rates() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = checkpoint_with_samples([
            used_sample(0, 0, now_ms),
            used_sample(30 * MINUTE_MS, 30, now_ms),
            used_sample(45 * MINUTE_MS, 45, now_ms),
            used_sample(50 * MINUTE_MS, 50, now_ms),
            used_sample(now_ms, 60, now_ms),
        ]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[]);

        assert_eq!(view.support, QuotaAnalyticsSupport::Supported);
        assert_eq!(view.pools.len(), 1);
        let pool = &view.pools[0];
        assert_eq!(pool.rate_15m.status, QuotaRateStatus::Available);
        assert_eq!(pool.rate_60m.status, QuotaRateStatus::Available);
        assert_eq!(
            pool.rate_15m.rate_per_hour,
            Some(QuotaQuantity::from_integer(60, QuotaUnit::Usd))
        );
        assert_eq!(pool.rate_15m.rate_per_hour, pool.rate_60m.rate_per_hour);
    }

    #[test]
    fn analytics_preserves_latest_provider_endpoint_for_operator_matching() {
        let now_ms = 60 * MINUTE_MS;
        let endpoint = ProviderEndpointKey::new("codex", "provider-a", "endpoint-a");
        let mut sample = used_sample(now_ms, 60, now_ms);
        sample.endpoint = Some(endpoint.clone());
        sample.observation_provider_id = "usage-provider-a".to_string();

        let view = build_quota_analytics(&checkpoint_with_samples([sample]), now_ms, &[]);
        let serialized = serde_json::to_value(&view.pools[0]).expect("serialize quota analytics");

        assert_eq!(
            serialized.get("endpoint"),
            Some(&serde_json::to_value(endpoint).expect("serialize endpoint"))
        );
        assert_eq!(
            serialized
                .get("observation_provider_id")
                .and_then(serde_json::Value::as_str),
            Some("usage-provider-a")
        );
    }

    #[test]
    fn two_direct_total_points_stay_visible_without_becoming_a_rate() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = checkpoint_with_samples([
            direct_sample(0, 4, now_ms),
            direct_sample(now_ms, 12, now_ms),
        ]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[]);
        let pool = &view.pools[0];

        assert_eq!(pool.remote_used, None);
        assert_eq!(
            pool.remote_direct_total,
            Some(QuotaQuantity::from_integer(12, QuotaUnit::Usd))
        );
        assert_eq!(pool.rate_60m.status, QuotaRateStatus::InsufficientSamples);
        assert_eq!(pool.rate_60m.rate_per_hour, None);
    }

    #[test]
    fn three_samples_below_minimum_span_report_short_span() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = checkpoint_with_samples([
            used_sample(40 * MINUTE_MS, 0, now_ms),
            used_sample(50 * MINUTE_MS, 5, now_ms),
            used_sample(now_ms, 10, now_ms),
        ]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[]);

        assert_eq!(view.pools[0].rate_60m.status, QuotaRateStatus::ShortSpan);
    }

    #[test]
    fn future_latest_sample_reports_unordered_rate() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = checkpoint_with_samples([used_sample(now_ms + MINUTE_MS, 10, now_ms)]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[]);

        assert_eq!(view.pools[0].rate_60m.status, QuotaRateStatus::Unordered);
    }

    #[test]
    fn adjustment_inside_rate_window_reports_adjustment() {
        let now_ms = 60 * MINUTE_MS;
        let first = used_sample(0, 0, now_ms);
        let mut adjusted = used_sample(30 * MINUTE_MS, 5, now_ms);
        adjusted.adjustment = Some(QuotaAdjustmentKind::TopUp);
        let latest = used_sample(now_ms, 10, now_ms);
        let checkpoint = checkpoint_with_samples([first, adjusted, latest]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[]);

        assert_eq!(view.pools[0].rate_60m.status, QuotaRateStatus::Adjustment);
    }

    #[test]
    fn direct_totals_without_a_monotonic_counter_report_no_counter() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = checkpoint_with_samples([
            direct_sample(0, 0, now_ms),
            direct_sample(30 * MINUTE_MS, 5, now_ms),
            direct_sample(now_ms, 10, now_ms),
        ]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[]);

        assert_eq!(view.pools[0].rate_60m.status, QuotaRateStatus::NoCounter);
    }

    #[test]
    fn rate_arithmetic_overflow_is_explicit() {
        let now_ms = 60 * MINUTE_MS;
        let first = used_sample(0, 0, now_ms);
        let mut middle = used_sample(30 * MINUTE_MS, 0, now_ms);
        middle.used = Some(QuotaQuantity::from_integer(i128::MAX / 2, QuotaUnit::Usd));
        let mut latest = used_sample(now_ms, 0, now_ms);
        latest.used = Some(QuotaQuantity::from_integer(i128::MAX, QuotaUnit::Usd));
        let checkpoint = checkpoint_with_samples([first, middle, latest]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[]);

        assert_eq!(view.pools[0].rate_60m.status, QuotaRateStatus::Overflow);
    }

    #[test]
    fn attribution_plan_matches_latest_normalization_epoch_bounds() {
        let now_ms = 70 * MINUTE_MS;
        let mut old = used_sample(0, 10, now_ms);
        old.signature.adjustment_revision = 0;
        let mut current_start = used_sample(30 * MINUTE_MS, 20, now_ms);
        current_start.signature.adjustment_revision = 1;
        current_start.signature.conversion_generation = Some(44);
        current_start.used = current_start
            .used
            .map(|value| value.with_conversion_generation(Some(44)));
        let mut current_end = used_sample(60 * MINUTE_MS, 50, now_ms);
        current_end.signature = current_start.signature.clone();
        current_end.used = current_end
            .used
            .map(|value| value.with_conversion_generation(Some(44)));
        let checkpoint = checkpoint_with_samples([old, current_start, current_end]);

        let plans = plan_quota_attribution(&checkpoint, now_ms);

        assert_eq!(plans.len(), 1);
        let plan = &plans[0];
        assert_eq!(plan.pool_key, "pool-a");
        assert_eq!(plan.pool_revision, 7);
        assert_eq!(plan.conversion_generation, Some(44));
        assert_eq!(plan.unit, QuotaUnit::Usd);
        assert_eq!(plan.query.start_ms, 30 * MINUTE_MS);
        assert_eq!(plan.query.end_ms, 60 * MINUTE_MS);
        assert_eq!(
            plan.query.pool,
            Some(crate::state::AttributionPoolKey {
                pool_key: "pool-a".to_string(),
                revision: 7,
            })
        );
    }

    #[test]
    fn wall_clock_freshness_and_fractional_rates_are_preserved() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = checkpoint_with_samples([
            used_sample(20 * MINUTE_MS, 0, now_ms),
            used_sample(40 * MINUTE_MS, 0, now_ms),
            used_sample(now_ms, 1, now_ms),
        ]);

        let fresh = build_quota_analytics(&checkpoint, now_ms, &[]);
        assert_eq!(
            fresh.pools[0].rate_60m.rate_per_hour,
            Some(QuotaQuantity::new(15, 1, QuotaUnit::Usd))
        );

        let stale = build_quota_analytics(&checkpoint, now_ms + MINUTE_MS, &[]);
        assert_eq!(stale.pools[0].rate_60m.status, QuotaRateStatus::Stale);
    }

    #[test]
    fn first_adjustment_marks_a_new_epoch_without_permanent_rate_failure() {
        let now_ms = 60 * MINUTE_MS;
        let mut first = used_sample(30 * MINUTE_MS, 10, now_ms);
        first.signature.adjustment_revision = 1;
        first.adjustment = Some(crate::quota_pool::QuotaAdjustmentKind::TopUp);
        let mut second = used_sample(45 * MINUTE_MS, 25, now_ms);
        second.signature = first.signature.clone();
        let mut third = used_sample(now_ms, 40, now_ms);
        third.signature = first.signature.clone();
        let checkpoint = checkpoint_with_samples([first, second, third]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[]);

        assert_eq!(view.pools[0].rate_60m.status, QuotaRateStatus::Available);
        assert_eq!(
            view.pools[0].rate_60m.rate_per_hour,
            Some(QuotaQuantity::from_integer(60, QuotaUnit::Usd))
        );
    }

    #[test]
    fn attribution_plan_requires_current_endpoints_and_stops_at_epoch_end() {
        let now_ms = 70 * MINUTE_MS;
        let mut first = used_sample(30 * MINUTE_MS, 20, now_ms);
        first.signature.window_end_ms = Some(65 * MINUTE_MS);
        let mut latest = used_sample(now_ms, 50, now_ms);
        latest.signature = first.signature.clone();
        let mut checkpoint = checkpoint_with_samples([first, latest]);

        let plans = plan_quota_attribution(&checkpoint, now_ms);
        assert_eq!(plans[0].query.end_ms, 65 * MINUTE_MS);

        checkpoint.memberships.clear();
        assert!(plan_quota_attribution(&checkpoint, now_ms).is_empty());
    }

    #[test]
    fn attribution_plan_stops_at_earliest_reset_boundary() {
        let now_ms = 80 * MINUTE_MS;
        let mut first = used_sample(30 * MINUTE_MS, 20, now_ms);
        first.signature.window_end_ms = Some(75 * MINUTE_MS);
        first.signature.reset_at_ms = Some(65 * MINUTE_MS);
        let mut latest = used_sample(70 * MINUTE_MS, 50, now_ms);
        latest.signature = first.signature.clone();
        let checkpoint = checkpoint_with_samples([first, latest]);

        let plans = plan_quota_attribution(&checkpoint, now_ms);

        assert_eq!(plans[0].query.end_ms, 65 * MINUTE_MS);
    }

    #[test]
    fn attribution_plan_omits_empty_or_reversed_windows() {
        let now_ms = 60 * MINUTE_MS;
        let observed_at_ms = 50 * MINUTE_MS;

        for window_start_ms in [observed_at_ms, observed_at_ms + MINUTE_MS] {
            let mut sample = direct_sample(observed_at_ms, 100, now_ms);
            sample.window_start_ms = Some(window_start_ms);
            sample.signature.window_start_ms = Some(window_start_ms);
            let checkpoint = checkpoint_with_samples([sample]);

            assert!(plan_quota_attribution(&checkpoint, now_ms).is_empty());
        }
    }

    #[test]
    fn lagging_direct_total_reconciles_only_through_remote_observation() {
        let now_ms = 60 * MINUTE_MS;
        let observed_at_ms = 50 * MINUTE_MS;
        let mut sample = direct_sample(observed_at_ms, 100, now_ms);
        sample.window_start_ms = Some(0);
        sample.signature.window_start_ms = Some(0);
        let checkpoint = checkpoint_with_samples([sample]);
        let attribution = attribution_result(&checkpoint, now_ms, [(git_project("C:/src/a"), 55)]);

        assert_eq!(attribution.window.query.end_ms, observed_at_ms);

        let view = build_quota_analytics(&checkpoint, now_ms, &[attribution]);
        let reconciliation = &view.pools[0].reconciliation;

        assert_eq!(reconciliation.status, QuotaReconciliationStatus::Available);
        assert_eq!(
            reconciliation.external_unattributed,
            Some(QuotaQuantity::from_integer(45, QuotaUnit::Usd))
        );
    }

    #[test]
    fn now_ended_local_result_with_tail_is_rejected_as_window_mismatch() {
        let now_ms = 60 * MINUTE_MS;
        let observed_at_ms = 50 * MINUTE_MS;
        let mut sample = direct_sample(observed_at_ms, 100, now_ms);
        sample.window_start_ms = Some(0);
        sample.signature.window_start_ms = Some(0);
        let checkpoint = checkpoint_with_samples([sample]);
        let mut attribution = attribution_result(
            &checkpoint,
            now_ms,
            [
                (git_project("C:/src/prefix"), 55),
                (git_project("C:/src/local-tail"), 20),
            ],
        );
        attribution.attribution.end_ms = now_ms;
        attribution.attribution.covered_end_ms = now_ms;

        let view = build_quota_analytics(&checkpoint, now_ms, &[attribution]);
        let reconciliation = &view.pools[0].reconciliation;

        assert_eq!(
            reconciliation.status,
            QuotaReconciliationStatus::WindowMismatch
        );
        assert_eq!(reconciliation.local_known, None);
        assert_eq!(reconciliation.external_unattributed, None);
        assert_eq!(reconciliation.signed_delta, None);
    }

    #[test]
    fn now_ended_attribution_window_is_rejected_as_window_mismatch() {
        let now_ms = 60 * MINUTE_MS;
        let observed_at_ms = 50 * MINUTE_MS;
        let mut sample = direct_sample(observed_at_ms, 100, now_ms);
        sample.window_start_ms = Some(0);
        sample.signature.window_start_ms = Some(0);
        let checkpoint = checkpoint_with_samples([sample]);
        let mut attribution =
            attribution_result(&checkpoint, now_ms, [(git_project("C:/src/a"), 55)]);
        attribution.window.query.end_ms = now_ms;
        attribution.attribution.end_ms = now_ms;
        attribution.attribution.covered_end_ms = now_ms;

        let view = build_quota_analytics(&checkpoint, now_ms, &[attribution]);

        assert_eq!(
            view.pools[0].reconciliation.status,
            QuotaReconciliationStatus::WindowMismatch
        );
    }

    #[test]
    fn remote_100_local_55_unknown_5_yields_external_40() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = direct_window_checkpoint(100, QuotaUnit::Usd, now_ms);
        let attribution = attribution_result(
            &checkpoint,
            now_ms,
            [
                (git_project("C:/src/a"), 55),
                (ProjectIdentity::default(), 5),
            ],
        );

        let view = build_quota_analytics(&checkpoint, now_ms, &[attribution]);
        let reconciliation = &view.pools[0].reconciliation;

        assert_eq!(reconciliation.status, QuotaReconciliationStatus::Available);
        assert_eq!(
            reconciliation.local_known,
            Some(QuotaQuantity::from_integer(55, QuotaUnit::Usd))
        );
        assert_eq!(
            reconciliation.local_unknown,
            Some(QuotaQuantity::from_integer(5, QuotaUnit::Usd))
        );
        assert_eq!(
            reconciliation.external_unattributed,
            Some(QuotaQuantity::from_integer(40, QuotaUnit::Usd))
        );
        assert_eq!(
            reconciliation.signed_delta,
            Some(SignedUsdDelta::from_femto_usd(40 * 10_i128.pow(15)))
        );
        assert_eq!(reconciliation.projects.len(), 1);
    }

    #[test]
    fn remote_50_local_60_keeps_negative_gap_and_zero_external() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = direct_window_checkpoint(50, QuotaUnit::Usd, now_ms);
        let attribution = attribution_result(&checkpoint, now_ms, [(git_project("C:/src/a"), 60)]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[attribution]);
        let reconciliation = &view.pools[0].reconciliation;

        assert_eq!(reconciliation.status, QuotaReconciliationStatus::Available);
        assert_eq!(
            reconciliation.external_unattributed,
            Some(QuotaQuantity::from_integer(0, QuotaUnit::Usd))
        );
        assert_eq!(
            reconciliation.signed_delta,
            Some(SignedUsdDelta::from_femto_usd(-10 * 10_i128.pow(15)))
        );
    }

    #[test]
    fn raw_remote_total_keeps_local_side_but_refuses_reconciliation() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = direct_window_checkpoint(100, QuotaUnit::Raw, now_ms);
        let attribution = attribution_result(&checkpoint, now_ms, [(git_project("C:/src/a"), 55)]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[attribution]);
        let reconciliation = &view.pools[0].reconciliation;

        assert_eq!(
            reconciliation.status,
            QuotaReconciliationStatus::IncompatibleUnit
        );
        assert_eq!(
            reconciliation.local_known,
            Some(QuotaQuantity::from_integer(55, QuotaUnit::Usd))
        );
        assert_eq!(reconciliation.external_unattributed, None);
        assert_eq!(reconciliation.signed_delta, None);
    }

    #[test]
    fn pace_deadband_includes_exact_08_and_12_boundaries() {
        for (burn, expected_ratio) in [(8, 8_000), (12, 12_000)] {
            let now_ms = 60 * MINUTE_MS;
            let mut samples = [
                used_sample(0, 0, now_ms),
                used_sample(30 * MINUTE_MS, burn / 2, now_ms),
                used_sample(now_ms, burn, now_ms),
            ];
            let reset_at_ms = now_ms + HOUR_MS;
            for sample in &mut samples {
                sample.signature.reset_at_ms = Some(reset_at_ms);
            }
            let latest = samples.last_mut().expect("latest");
            latest.remaining = Some(QuotaQuantity::from_integer(10, QuotaUnit::Usd));
            latest.reset_at_ms = Some(reset_at_ms);
            let checkpoint = checkpoint_with_samples(samples);

            let view = build_quota_analytics(&checkpoint, now_ms, &[]);
            assert_eq!(view.pools[0].pacing.status, QuotaPaceStatus::OnPace);
            assert_eq!(
                view.pools[0].pacing.pace_ratio_basis_points,
                Some(expected_ratio)
            );
        }
    }

    #[test]
    fn incomplete_coverage_never_labels_the_gap_as_external() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = direct_window_checkpoint(100, QuotaUnit::Usd, now_ms);
        let mut attribution =
            attribution_result(&checkpoint, now_ms, [(git_project("C:/src/a"), 55)]);
        attribution.attribution.coverage.unpriced_requests = 1;

        let view = build_quota_analytics(&checkpoint, now_ms, &[attribution]);
        let reconciliation = &view.pools[0].reconciliation;

        assert_eq!(
            reconciliation.status,
            QuotaReconciliationStatus::IncompleteCoverage
        );
        assert_eq!(reconciliation.external_unattributed, None);
        assert_eq!(reconciliation.signed_delta, None);
    }

    #[test]
    fn partial_captured_cost_stays_visible_without_a_reconciliation_delta() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = direct_window_checkpoint(100, QuotaUnit::Usd, now_ms);
        let project = git_project("C:/src/partial");
        let mut attribution = attribution_result(&checkpoint, now_ms, [(project.clone(), 55)]);
        attribution
            .attribution
            .coverage
            .partial_captured_price_requests = 1;

        let view = build_quota_analytics(&checkpoint, now_ms, &[attribution]);
        let reconciliation = &view.pools[0].reconciliation;

        assert_eq!(
            reconciliation.status,
            QuotaReconciliationStatus::IncompleteCoverage
        );
        assert_eq!(
            reconciliation.local_known,
            Some(QuotaQuantity::from_integer(55, QuotaUnit::Usd))
        );
        assert_eq!(reconciliation.projects.len(), 1);
        assert_eq!(reconciliation.projects[0].project, project);
        assert_eq!(
            reconciliation.projects[0].local_cost,
            QuotaQuantity::from_integer(55, QuotaUnit::Usd)
        );
        assert_eq!(reconciliation.external_unattributed, None);
        assert_eq!(reconciliation.signed_delta, None);
    }

    #[test]
    fn conversion_generation_mismatch_refuses_usd_reconciliation() {
        let now_ms = 60 * MINUTE_MS;
        let mut checkpoint = direct_window_checkpoint(100, QuotaUnit::Usd, now_ms);
        let sample = checkpoint
            .pools
            .get_mut("pool-a")
            .and_then(|pool| pool.samples.back_mut())
            .expect("sample");
        sample.signature.conversion_generation = Some(1);
        sample.direct_total = sample
            .direct_total
            .take()
            .map(|value| value.with_conversion_generation(Some(2)));
        let attribution = attribution_result(&checkpoint, now_ms, [(git_project("C:/src/a"), 55)]);

        let view = build_quota_analytics(&checkpoint, now_ms, &[attribution]);

        assert_eq!(
            view.pools[0].reconciliation.status,
            QuotaReconciliationStatus::IncompatibleGeneration
        );
        assert_eq!(view.pools[0].reconciliation.signed_delta, None);
    }

    #[test]
    fn remaining_decrease_is_an_explicit_lower_bound_rate() {
        let now_ms = 60 * MINUTE_MS;
        let mut samples = [
            used_sample(0, 0, now_ms),
            used_sample(30 * MINUTE_MS, 0, now_ms),
            used_sample(now_ms, 0, now_ms),
        ];
        for (sample, remaining) in samples.iter_mut().zip([60, 50, 40]) {
            sample.used = None;
            sample.remaining = Some(QuotaQuantity::from_integer(remaining, QuotaUnit::Usd));
            sample.signature.counter_kind = QuotaCounterKind::Remaining;
        }

        let view = build_quota_analytics(&checkpoint_with_samples(samples), now_ms, &[]);

        assert_eq!(view.pools[0].rate_60m.status, QuotaRateStatus::Available);
        assert!(view.pools[0].rate_60m.lower_bound);
        assert_eq!(
            view.pools[0].rate_60m.rate_per_hour,
            Some(QuotaQuantity::from_integer(20, QuotaUnit::Usd))
        );
    }

    #[test]
    fn long_gap_and_counter_rollback_suppress_rates() {
        let now_ms = 60 * MINUTE_MS;
        let mut gap_samples = [
            used_sample(0, 0, now_ms),
            used_sample(30 * MINUTE_MS, 30, now_ms),
            used_sample(now_ms, 60, now_ms),
        ];
        gap_samples[0].sampling.continuity_deadline_ms = Some(10 * MINUTE_MS);
        let gap = build_quota_analytics(&checkpoint_with_samples(gap_samples), now_ms, &[]);
        assert_eq!(gap.pools[0].rate_60m.status, QuotaRateStatus::Gap);

        let rollback = build_quota_analytics(
            &checkpoint_with_samples([
                used_sample(0, 10, now_ms),
                used_sample(30 * MINUTE_MS, 5, now_ms),
                used_sample(now_ms, 20, now_ms),
            ]),
            now_ms,
            &[],
        );
        assert_eq!(
            rollback.pools[0].rate_60m.status,
            QuotaRateStatus::NegativeDelta
        );
    }

    #[test]
    fn pace_outside_deadband_is_faster_or_slower_and_resetless_omits_target() {
        for (burn, expected) in [(7, QuotaPaceStatus::Slower), (13, QuotaPaceStatus::Faster)] {
            let now_ms = 60 * MINUTE_MS;
            let reset_at_ms = now_ms + HOUR_MS;
            let mut samples = [
                used_sample(0, 0, now_ms),
                used_sample(30 * MINUTE_MS, burn / 2, now_ms),
                used_sample(now_ms, burn, now_ms),
            ];
            for sample in &mut samples {
                sample.signature.reset_at_ms = Some(reset_at_ms);
            }
            let latest = samples.last_mut().expect("latest");
            latest.remaining = Some(QuotaQuantity::from_integer(10, QuotaUnit::Usd));
            latest.reset_at_ms = Some(reset_at_ms);
            let view = build_quota_analytics(&checkpoint_with_samples(samples), now_ms, &[]);
            assert_eq!(view.pools[0].pacing.status, expected);
        }

        let now_ms = 60 * MINUTE_MS;
        let mut samples = [
            used_sample(0, 0, now_ms),
            used_sample(30 * MINUTE_MS, 5, now_ms),
            used_sample(now_ms, 10, now_ms),
        ];
        for sample in &mut samples {
            sample.window.kind = QuotaWindowKind::Resetless;
            sample.signature.window.kind = QuotaWindowKind::Resetless;
        }
        samples.last_mut().expect("latest").remaining =
            Some(QuotaQuantity::from_integer(10, QuotaUnit::Usd));
        let view = build_quota_analytics(&checkpoint_with_samples(samples), now_ms, &[]);
        assert_eq!(view.pools[0].pacing.status, QuotaPaceStatus::NoReset);
        assert_eq!(view.pools[0].pacing.required_rate_per_hour, None);
        assert!(view.pools[0].pacing.exhaustion_eta_ms.is_some());
    }

    #[test]
    fn project_rows_are_bounded_after_full_local_sum() {
        let now_ms = 60 * MINUTE_MS;
        let checkpoint = direct_window_checkpoint(300, QuotaUnit::Usd, now_ms);
        let rows = (0..=MAX_PROJECT_ROWS)
            .map(|index| (git_project(&format!("C:/src/{index}")), 1))
            .collect::<Vec<_>>();
        let attribution = attribution_result(&checkpoint, now_ms, rows);

        let view = build_quota_analytics(&checkpoint, now_ms, &[attribution]);
        let reconciliation = &view.pools[0].reconciliation;

        assert_eq!(reconciliation.projects.len(), MAX_PROJECT_ROWS);
        assert_eq!(reconciliation.omitted_projects, 1);
        assert_eq!(
            reconciliation.local_known,
            Some(QuotaQuantity::from_integer(257, QuotaUnit::Usd))
        );
        assert_eq!(
            reconciliation.omitted_local_known,
            Some(QuotaQuantity::from_integer(1, QuotaUnit::Usd))
        );
    }

    fn direct_window_checkpoint(
        total: i128,
        unit: QuotaUnit,
        now_ms: u64,
    ) -> QuotaRegistryCheckpoint {
        let mut sample = direct_sample(now_ms, total, now_ms);
        sample.direct_total = Some(QuotaQuantity::from_integer(total, unit));
        sample.window_start_ms = Some(0);
        sample.signature.window_start_ms = Some(0);
        sample.signature.unit = unit;
        checkpoint_with_samples([sample])
    }

    fn attribution_result(
        checkpoint: &QuotaRegistryCheckpoint,
        now_ms: u64,
        rows: impl IntoIterator<Item = (ProjectIdentity, i128)>,
    ) -> PoolAttributionResult {
        let window = plan_quota_attribution(checkpoint, now_ms)
            .into_iter()
            .next()
            .expect("attribution plan");
        let rows = rows
            .into_iter()
            .map(|(project, cost_usd)| {
                let mut aggregate = AttributionAggregate::default();
                aggregate.requests = 1;
                aggregate.record_cost_femto_for_test(cost_usd * 10_i128.pow(15));
                AttributionBucket {
                    key: AttributionBucketKey {
                        bucket_start_ms: 0,
                        pool: Some(AttributionPoolKey {
                            pool_key: window.pool_key.clone(),
                            revision: window.pool_revision,
                        }),
                        project,
                        price_coverage: AccountingPriceCoverage::Captured,
                        ..AttributionBucketKey::default()
                    },
                    aggregate,
                }
            })
            .collect();
        PoolAttributionResult {
            attribution: AttributionQueryResult {
                start_ms: window.query.start_ms,
                end_ms: window.query.end_ms,
                covered_start_ms: window.query.start_ms,
                covered_end_ms: window.query.end_ms,
                rows,
                ..AttributionQueryResult::default()
            },
            window,
        }
    }

    fn git_project(path: &str) -> ProjectIdentity {
        ProjectIdentity {
            kind: ProjectIdentityKind::GitRoot,
            path: Some(path.to_string()),
        }
    }

    fn checkpoint_with_samples(
        samples: impl IntoIterator<Item = QuotaObservation>,
    ) -> QuotaRegistryCheckpoint {
        let identity = pool_identity();
        let endpoint =
            crate::runtime_identity::ProviderEndpointKey::new("codex", "provider-a", "endpoint-a");
        QuotaRegistryCheckpoint {
            pools: BTreeMap::from([(
                identity.key.clone(),
                QuotaPoolState {
                    identity,
                    samples: samples.into_iter().collect::<VecDeque<_>>(),
                    last_success_at_ms: None,
                    last_attempt_at_ms: None,
                    adjustment_revision: 0,
                },
            )]),
            memberships: BTreeMap::from([(
                endpoint.clone(),
                crate::quota_pool::PoolMembership {
                    pool: pool_identity(),
                    endpoint,
                    since_ms: 0,
                },
            )]),
            ..QuotaRegistryCheckpoint::default()
        }
    }

    fn pool_identity() -> PoolIdentity {
        PoolIdentity {
            key: "pool-a".to_string(),
            origin: "https://relay.example".to_string(),
            scope: QuotaScope::Account,
            revision: 7,
            evidence: IdentityEvidence::ExplicitPoolId,
            confidence: IdentityConfidence::High,
            aggregation_eligible: true,
            conflicting_evidence: false,
        }
    }

    fn used_sample(observed_at_ms: u64, used: i128, fresh_until_ms: u64) -> QuotaObservation {
        let pool = pool_identity();
        let quantity = QuotaQuantity::from_integer(used, QuotaUnit::Usd);
        QuotaObservation {
            pool: pool.clone(),
            observed_at_ms,
            source: "test".to_string(),
            status: "ok".to_string(),
            fresh: true,
            used: Some(quantity),
            sampling: crate::quota_pool::QuotaSamplingSemantics {
                expected_interval_ms: Some(5 * MINUTE_MS),
                fresh_until_ms: Some(fresh_until_ms),
                continuity_deadline_ms: Some(fresh_until_ms),
            },
            signature: NormalizationSignature {
                pool_key: pool.key,
                pool_revision: pool.revision,
                counter_kind: QuotaCounterKind::Used,
                unit: QuotaUnit::Usd,
                scope: QuotaScope::Account,
                ..NormalizationSignature::default()
            },
            ..QuotaObservation::default()
        }
    }

    fn direct_sample(observed_at_ms: u64, total: i128, fresh_until_ms: u64) -> QuotaObservation {
        let mut sample = used_sample(observed_at_ms, total, fresh_until_ms);
        sample.used = None;
        sample.direct_total = Some(QuotaQuantity::from_integer(total, QuotaUnit::Usd));
        sample.signature.counter_kind = QuotaCounterKind::DirectTotal;
        sample
    }
}
