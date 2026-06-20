use serde::{Deserialize, Serialize};

use crate::balance::ProviderBalanceSnapshot;
use crate::config::UsageForecastConfig;
use crate::pricing::UsdAmount;
use crate::state::FinishedRequest;

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageSpendForecast {
    pub enabled: bool,
    pub sample_window_ms: u64,
    pub sample_elapsed_ms: u64,
    pub projected_sample_elapsed_ms: u64,
    pub priced_requests: u64,
    pub unpriced_requests: u64,
    pub sample_cost_usd: Option<String>,
    pub rate_per_hour_usd: Option<String>,
    pub reset_at_ms: Option<u64>,
    pub reset_in_ms: Option<u64>,
    pub projected_until_reset_usd: Option<String>,
    pub primary_balance_remaining_usd: Option<String>,
    pub projected_balance_after_reset_usd: Option<String>,
    pub projected_exhaustion: bool,
    #[serde(default)]
    pub balance_calibrated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance_calibration_multiplier_pct: Option<u32>,
    #[serde(default)]
    pub balance_calibration_requests: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance_calibration_window_ms: Option<u64>,
    pub confidence: UsageForecastConfidence,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageBalanceCalibration {
    pub available: bool,
    pub provider_id: String,
    pub station_name: Option<String>,
    pub upstream_index: Option<usize>,
    pub window_start_ms: Option<u64>,
    pub window_end_ms: Option<u64>,
    pub window_ms: Option<u64>,
    pub matched_requests: u64,
    pub actual_delta_usd: Option<String>,
    pub estimated_delta_usd: Option<String>,
    pub multiplier_pct: Option<u32>,
    pub status: UsageBalanceCalibrationStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UsageBalanceCalibrationStatus {
    #[default]
    Unavailable,
    Calibrated,
    LowSample,
    OutOfRange,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct QuotaPacingForecast {
    pub available: bool,
    pub provider_id: String,
    pub station_name: Option<String>,
    pub upstream_index: Option<usize>,
    pub source: String,
    pub plan_name: Option<String>,
    pub period: Option<String>,
    pub unlimited: bool,
    pub remaining_usd: Option<String>,
    pub limit_usd: Option<String>,
    pub used_usd: Option<String>,
    pub rate_per_hour_usd: Option<String>,
    pub target_rate_per_hour_usd: Option<String>,
    pub reset_at_ms: Option<u64>,
    pub reset_in_ms: Option<u64>,
    pub estimated_exhaustion_in_ms: Option<u64>,
    pub pace_ratio_pct: Option<u32>,
    pub status: QuotaPacingStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPacingStatus {
    #[default]
    Unavailable,
    Unlimited,
    NoSpendRate,
    UnknownReset,
    OnTrack,
    Fast,
    Slow,
    Exhausted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UsageForecastConfidence {
    #[default]
    Disabled,
    NoData,
    LowSample,
    PartialPricing,
    Estimated,
}

pub trait UsageForecastRequestLike {
    fn id(&self) -> u64;
    fn trace_id(&self) -> Option<&str>;
    fn ended_at_ms(&self) -> u64;
    fn provider_id(&self) -> Option<&str>;
    fn station_name(&self) -> Option<&str>;
    fn total_cost_usd(&self) -> Option<&str>;
    fn has_usage(&self) -> bool;
}

pub trait UsageForecastBalanceHistoryLike {
    fn fetched_at_ms(&self) -> u64;
    fn provider_id(&self) -> &str;
    fn station_name(&self) -> Option<&str>;
    fn upstream_index(&self) -> Option<usize>;
    fn quota_remaining_usd(&self) -> Option<&str>;
    fn subscription_balance_usd(&self) -> Option<&str>;
    fn total_balance_usd(&self) -> Option<&str>;
    fn error(&self) -> Option<&str>;
    fn unlimited_quota(&self) -> bool;
}

impl<T: UsageForecastRequestLike + ?Sized> UsageForecastRequestLike for &T {
    fn id(&self) -> u64 {
        (**self).id()
    }

    fn trace_id(&self) -> Option<&str> {
        (**self).trace_id()
    }

    fn ended_at_ms(&self) -> u64 {
        (**self).ended_at_ms()
    }

    fn provider_id(&self) -> Option<&str> {
        (**self).provider_id()
    }

    fn station_name(&self) -> Option<&str> {
        (**self).station_name()
    }

    fn total_cost_usd(&self) -> Option<&str> {
        (**self).total_cost_usd()
    }

    fn has_usage(&self) -> bool {
        (**self).has_usage()
    }
}

impl<T: UsageForecastBalanceHistoryLike + ?Sized> UsageForecastBalanceHistoryLike for &T {
    fn fetched_at_ms(&self) -> u64 {
        (**self).fetched_at_ms()
    }

    fn provider_id(&self) -> &str {
        (**self).provider_id()
    }

    fn station_name(&self) -> Option<&str> {
        (**self).station_name()
    }

    fn upstream_index(&self) -> Option<usize> {
        (**self).upstream_index()
    }

    fn quota_remaining_usd(&self) -> Option<&str> {
        (**self).quota_remaining_usd()
    }

    fn subscription_balance_usd(&self) -> Option<&str> {
        (**self).subscription_balance_usd()
    }

    fn total_balance_usd(&self) -> Option<&str> {
        (**self).total_balance_usd()
    }

    fn error(&self) -> Option<&str> {
        (**self).error()
    }

    fn unlimited_quota(&self) -> bool {
        (**self).unlimited_quota()
    }
}

impl UsageForecastRequestLike for FinishedRequest {
    fn id(&self) -> u64 {
        self.id
    }

    fn trace_id(&self) -> Option<&str> {
        self.trace_id.as_deref()
    }

    fn ended_at_ms(&self) -> u64 {
        self.ended_at_ms
    }

    fn provider_id(&self) -> Option<&str> {
        self.provider_id.as_deref()
    }

    fn station_name(&self) -> Option<&str> {
        self.station_name.as_deref()
    }

    fn total_cost_usd(&self) -> Option<&str> {
        self.cost.total_cost_usd.as_deref()
    }

    fn has_usage(&self) -> bool {
        self.usage.is_some()
    }
}

impl UsageForecastBalanceHistoryLike for ProviderBalanceSnapshot {
    fn fetched_at_ms(&self) -> u64 {
        self.fetched_at_ms
    }

    fn provider_id(&self) -> &str {
        self.provider_id.as_str()
    }

    fn station_name(&self) -> Option<&str> {
        self.station_name.as_deref()
    }

    fn upstream_index(&self) -> Option<usize> {
        self.upstream_index
    }

    fn quota_remaining_usd(&self) -> Option<&str> {
        self.quota_remaining_usd.as_deref()
    }

    fn subscription_balance_usd(&self) -> Option<&str> {
        self.subscription_balance_usd.as_deref()
    }

    fn total_balance_usd(&self) -> Option<&str> {
        self.total_balance_usd.as_deref()
    }

    fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn unlimited_quota(&self) -> bool {
        self.unlimited_quota == Some(true)
    }
}

pub fn build_usage_spend_forecast<T>(
    config: &UsageForecastConfig,
    recent: &[T],
    provider_balances: &[ProviderBalanceSnapshot],
    now_ms: u64,
) -> UsageSpendForecast
where
    T: UsageForecastRequestLike,
{
    let empty_history: &[ProviderBalanceSnapshot] = &[];
    build_usage_spend_forecast_with_balance_history(
        config,
        recent,
        provider_balances,
        empty_history,
        now_ms,
    )
}

pub fn build_usage_spend_forecast_with_balance_history<T, H>(
    config: &UsageForecastConfig,
    recent: &[T],
    provider_balances: &[ProviderBalanceSnapshot],
    provider_balance_history: &[H],
    now_ms: u64,
) -> UsageSpendForecast
where
    T: UsageForecastRequestLike,
    H: UsageForecastBalanceHistoryLike,
{
    let calibration =
        build_usage_balance_calibration(recent, provider_balance_history, config, now_ms);
    let mut forecast = build_usage_spend_forecast_inner(config, recent, provider_balances, now_ms);
    apply_balance_calibration(&mut forecast, calibration);
    forecast
}

pub fn build_usage_balance_calibration<T, H>(
    recent: &[T],
    provider_balance_history: &[H],
    config: &UsageForecastConfig,
    now_ms: u64,
) -> UsageBalanceCalibration
where
    T: UsageForecastRequestLike,
    H: UsageForecastBalanceHistoryLike,
{
    let sample_window_ms = config.rate_window_minutes.max(1).saturating_mul(MINUTE_MS);
    let cutoff_ms = now_ms.saturating_sub(sample_window_ms.saturating_mul(2));
    let Some((previous, current, actual_delta)) =
        best_balance_delta_pair(provider_balance_history, cutoff_ms, now_ms)
    else {
        return UsageBalanceCalibration {
            reason: Some("no usable balance delta".to_string()),
            ..UsageBalanceCalibration::default()
        };
    };

    let mut estimated_delta = UsdAmount::ZERO;
    let mut matched_requests = 0_u64;
    for request in recent {
        if request.ended_at_ms() <= previous.fetched_at_ms()
            || request.ended_at_ms() > current.fetched_at_ms()
        {
            continue;
        }
        if !request_matches_balance_snapshot(request, current) {
            continue;
        }
        let Some(cost) = request
            .total_cost_usd()
            .and_then(UsdAmount::from_decimal_str)
        else {
            continue;
        };
        if cost.is_zero() {
            continue;
        }
        estimated_delta = estimated_delta.saturating_add(cost);
        matched_requests = matched_requests.saturating_add(1);
    }

    let window_ms = current
        .fetched_at_ms()
        .saturating_sub(previous.fetched_at_ms());
    let mut out = UsageBalanceCalibration {
        available: true,
        provider_id: current.provider_id().to_string(),
        station_name: current.station_name().map(ToOwned::to_owned),
        upstream_index: current.upstream_index(),
        window_start_ms: Some(previous.fetched_at_ms()),
        window_end_ms: Some(current.fetched_at_ms()),
        window_ms: Some(window_ms),
        matched_requests,
        actual_delta_usd: Some(actual_delta.format_usd()),
        estimated_delta_usd: Some(estimated_delta.format_usd()),
        ..UsageBalanceCalibration::default()
    };

    if matched_requests < config.min_priced_requests.max(1) || estimated_delta.is_zero() {
        out.status = UsageBalanceCalibrationStatus::LowSample;
        out.reason = Some("not enough matched priced requests".to_string());
        return out;
    }

    let multiplier_pct = amount_ratio_pct(actual_delta, estimated_delta);
    out.multiplier_pct = Some(multiplier_pct);
    if !(25..=400).contains(&multiplier_pct) {
        out.status = UsageBalanceCalibrationStatus::OutOfRange;
        out.reason = Some("balance delta multiplier outside guardrails".to_string());
        return out;
    }

    out.status = UsageBalanceCalibrationStatus::Calibrated;
    out
}

fn apply_balance_calibration(
    forecast: &mut UsageSpendForecast,
    calibration: UsageBalanceCalibration,
) {
    if calibration.status != UsageBalanceCalibrationStatus::Calibrated {
        return;
    }
    let Some(multiplier_pct) = calibration.multiplier_pct else {
        return;
    };

    forecast.balance_calibrated = true;
    forecast.balance_calibration_multiplier_pct = Some(multiplier_pct);
    forecast.balance_calibration_requests = calibration.matched_requests;
    forecast.balance_calibration_window_ms = calibration.window_ms;

    if let Some(value) = forecast
        .sample_cost_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str)
    {
        forecast.sample_cost_usd =
            Some(scale_usd_by_ratio(value, u64::from(multiplier_pct), 100).format_usd());
    }
    if let Some(value) = forecast
        .rate_per_hour_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str)
    {
        forecast.rate_per_hour_usd =
            Some(scale_usd_by_ratio(value, u64::from(multiplier_pct), 100).format_usd());
    }
    if let Some(value) = forecast
        .projected_until_reset_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str)
    {
        forecast.projected_until_reset_usd =
            Some(scale_usd_by_ratio(value, u64::from(multiplier_pct), 100).format_usd());
    }

    let balance = forecast
        .primary_balance_remaining_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str);
    let projected = forecast
        .projected_until_reset_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str);
    forecast.projected_balance_after_reset_usd = match (balance, projected) {
        (Some(balance), Some(projected)) => Some(balance.saturating_sub(projected).format_usd()),
        _ => None,
    };
    forecast.projected_exhaustion =
        matches!((balance, projected), (Some(balance), Some(projected)) if projected > balance);
}

fn best_balance_delta_pair<H>(
    provider_balance_history: &[H],
    cutoff_ms: u64,
    now_ms: u64,
) -> Option<(&H, &H, UsdAmount)>
where
    H: UsageForecastBalanceHistoryLike,
{
    let mut best: Option<(&H, &H, UsdAmount)> = None;
    let mut histories = std::collections::BTreeMap::<String, Vec<&H>>::new();
    for snapshot in provider_balance_history {
        if snapshot.fetched_at_ms() < cutoff_ms || snapshot.fetched_at_ms() > now_ms {
            continue;
        }
        if snapshot
            .error()
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            continue;
        }
        if snapshot.unlimited_quota() {
            continue;
        }
        if balance_delta_amount(snapshot).is_none() {
            continue;
        }
        histories
            .entry(balance_history_key(snapshot))
            .or_default()
            .push(snapshot);
    }

    for history in histories.values_mut() {
        history.sort_by_key(|snapshot| snapshot.fetched_at_ms());
        for pair in history.windows(2) {
            let [previous, current] = pair else {
                continue;
            };
            let Some(previous_amount) = balance_delta_amount(previous) else {
                continue;
            };
            let Some(current_amount) = balance_delta_amount(current) else {
                continue;
            };
            if current.fetched_at_ms() <= previous.fetched_at_ms()
                || current_amount >= previous_amount
            {
                continue;
            }
            let delta = previous_amount.saturating_sub(current_amount);
            if delta.is_zero() {
                continue;
            }
            let replace = best.as_ref().is_none_or(|(_, best_current, _)| {
                current.fetched_at_ms() > best_current.fetched_at_ms()
            });
            if replace {
                best = Some((previous, current, delta));
            }
        }
    }

    best
}

fn balance_history_key<H: UsageForecastBalanceHistoryLike>(snapshot: &H) -> String {
    format!(
        "{}|{}|{}",
        snapshot.station_name().unwrap_or_default(),
        snapshot
            .upstream_index()
            .map(|idx| idx.to_string())
            .unwrap_or_default(),
        snapshot.provider_id()
    )
}

fn balance_delta_amount<H: UsageForecastBalanceHistoryLike>(snapshot: &H) -> Option<UsdAmount> {
    if let Some(amount) = snapshot
        .quota_remaining_usd()
        .and_then(UsdAmount::from_decimal_str)
    {
        return Some(amount);
    }
    if let Some(amount) = snapshot
        .subscription_balance_usd()
        .and_then(UsdAmount::from_decimal_str)
    {
        return Some(amount);
    }
    snapshot
        .total_balance_usd()
        .and_then(UsdAmount::from_decimal_str)
}

fn request_matches_balance_snapshot<T, H>(request: &T, snapshot: &H) -> bool
where
    T: UsageForecastRequestLike,
    H: UsageForecastBalanceHistoryLike,
{
    let provider_matches = match snapshot.provider_id().trim() {
        "" => true,
        provider_id => request.provider_id() == Some(provider_id),
    };
    let station_matches = snapshot
        .station_name()
        .is_none_or(|station_name| request.station_name() == Some(station_name));
    provider_matches && station_matches
}

fn build_usage_spend_forecast_inner<T>(
    config: &UsageForecastConfig,
    recent: &[T],
    provider_balances: &[ProviderBalanceSnapshot],
    now_ms: u64,
) -> UsageSpendForecast
where
    T: UsageForecastRequestLike,
{
    if !config.enabled {
        return UsageSpendForecast {
            enabled: false,
            reason: Some("disabled".to_string()),
            ..UsageSpendForecast::default()
        };
    }

    let sample_window_ms = config.rate_window_minutes.max(1).saturating_mul(MINUTE_MS);
    let reset_at_ms = next_reset_at_ms(
        now_ms,
        config.reset_utc_offset.as_str(),
        config.reset_time.as_str(),
    );
    let reset_in_ms = reset_at_ms.map(|reset| reset.saturating_sub(now_ms));

    let cutoff_ms = now_ms.saturating_sub(sample_window_ms);
    let mut priced_requests = 0_u64;
    let mut unpriced_requests = 0_u64;
    let mut sample_cost = UsdAmount::ZERO;
    let mut oldest_sample_ms: Option<u64> = None;

    for request in recent {
        if request.ended_at_ms() < cutoff_ms || request.ended_at_ms() > now_ms {
            continue;
        }
        if request.total_cost_usd().is_some() {
            if let Some(cost) = request
                .total_cost_usd()
                .and_then(UsdAmount::from_decimal_str)
            {
                sample_cost = sample_cost.saturating_add(cost);
                priced_requests = priced_requests.saturating_add(1);
                oldest_sample_ms = Some(
                    oldest_sample_ms
                        .map(|oldest| oldest.min(request.ended_at_ms()))
                        .unwrap_or(request.ended_at_ms()),
                );
            } else {
                unpriced_requests = unpriced_requests.saturating_add(1);
            }
        } else if request.has_usage() {
            unpriced_requests = unpriced_requests.saturating_add(1);
        }
    }

    let sample_elapsed_ms = oldest_sample_ms
        .map(|oldest| {
            now_ms
                .saturating_sub(oldest)
                .clamp(MINUTE_MS, sample_window_ms)
        })
        .unwrap_or(0);

    if priced_requests == 0 || sample_elapsed_ms == 0 {
        return UsageSpendForecast {
            enabled: true,
            sample_window_ms,
            sample_elapsed_ms,
            priced_requests,
            unpriced_requests,
            reset_at_ms,
            reset_in_ms,
            confidence: UsageForecastConfidence::NoData,
            reason: Some("no priced requests in forecast window".to_string()),
            ..UsageSpendForecast::default()
        };
    }

    let rate_per_hour = scale_usd_by_ratio(sample_cost, HOUR_MS, sample_elapsed_ms);
    let confidence = if priced_requests < config.min_priced_requests.max(1) {
        UsageForecastConfidence::LowSample
    } else if unpriced_requests > 0 {
        UsageForecastConfidence::PartialPricing
    } else {
        UsageForecastConfidence::Estimated
    };
    let projected_sample_elapsed_ms = if confidence == UsageForecastConfidence::LowSample {
        0
    } else {
        sample_elapsed_ms
    };
    let projected_until_reset = if confidence == UsageForecastConfidence::LowSample {
        None
    } else {
        reset_in_ms.map(|reset_in_ms| scale_usd_by_ratio(rate_per_hour, reset_in_ms, HOUR_MS))
    };
    let primary_balance_remaining =
        primary_remaining_balance(provider_balances, now_ms).map(|(_, amount)| amount);
    let projected_balance_after_reset = match (primary_balance_remaining, projected_until_reset) {
        (Some(balance), Some(projected)) => Some(balance.saturating_sub(projected)),
        _ => None,
    };
    let projected_exhaustion = matches!(
        (primary_balance_remaining, projected_until_reset),
        (Some(balance), Some(projected)) if projected > balance
    );

    UsageSpendForecast {
        enabled: true,
        sample_window_ms,
        sample_elapsed_ms,
        projected_sample_elapsed_ms,
        priced_requests,
        unpriced_requests,
        sample_cost_usd: Some(sample_cost.format_usd()),
        rate_per_hour_usd: Some(rate_per_hour.format_usd()),
        reset_at_ms,
        reset_in_ms,
        projected_until_reset_usd: projected_until_reset.map(UsdAmount::format_usd),
        primary_balance_remaining_usd: primary_balance_remaining.map(UsdAmount::format_usd),
        projected_balance_after_reset_usd: projected_balance_after_reset.map(UsdAmount::format_usd),
        projected_exhaustion,
        confidence,
        reason: None,
        ..UsageSpendForecast::default()
    }
}

pub fn build_quota_pacing_forecast(
    spend: &UsageSpendForecast,
    provider_balances: &[ProviderBalanceSnapshot],
    now_ms: u64,
) -> QuotaPacingForecast {
    let Some(snapshot) = primary_quota_snapshot(provider_balances, now_ms) else {
        return QuotaPacingForecast {
            reason: Some("no package quota snapshot".to_string()),
            ..QuotaPacingForecast::default()
        };
    };

    let rate_per_hour = spend
        .rate_per_hour_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str);
    let remaining = quota_remaining(snapshot);
    let limit = snapshot
        .quota_limit_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str);
    let used = snapshot
        .quota_used_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str);

    if snapshot.unlimited_quota == Some(true) {
        return quota_pacing_from_snapshot(snapshot, rate_per_hour, remaining, limit, used, None)
            .with_status(QuotaPacingStatus::Unlimited, Some("unlimited quota"));
    }

    let Some(remaining) = remaining else {
        return quota_pacing_from_snapshot(snapshot, rate_per_hour, None, limit, used, None)
            .with_status(
                QuotaPacingStatus::Unavailable,
                Some("quota remaining unavailable"),
            );
    };

    if remaining.is_zero() || snapshot.exhausted == Some(true) {
        return quota_pacing_from_snapshot(
            snapshot,
            rate_per_hour,
            Some(remaining),
            limit,
            used,
            None,
        )
        .with_status(QuotaPacingStatus::Exhausted, Some("quota exhausted"));
    }

    let estimated_exhaustion_in_ms =
        rate_per_hour.and_then(|rate| estimated_exhaustion_in_ms(remaining, rate));
    let mut pacing = quota_pacing_from_snapshot(
        snapshot,
        rate_per_hour,
        Some(remaining),
        limit,
        used,
        estimated_exhaustion_in_ms,
    );

    let Some(rate_per_hour) = rate_per_hour.filter(|rate| !rate.is_zero()) else {
        return pacing.with_status(QuotaPacingStatus::NoSpendRate, Some("no spend rate"));
    };

    let Some(reset_in_ms) = quota_reset_in_ms(spend, snapshot) else {
        return pacing.with_status(
            QuotaPacingStatus::UnknownReset,
            Some("reset time unavailable"),
        );
    };

    let target_rate = scale_usd_by_ratio(remaining, HOUR_MS, reset_in_ms);
    pacing.target_rate_per_hour_usd = Some(target_rate.format_usd());
    pacing.reset_at_ms = spend.reset_at_ms;
    pacing.reset_in_ms = Some(reset_in_ms);

    if target_rate.is_zero() {
        return pacing.with_status(QuotaPacingStatus::Fast, Some("target rate is zero"));
    }

    let ratio_pct = amount_ratio_pct(rate_per_hour, target_rate);
    pacing.pace_ratio_pct = Some(ratio_pct);
    pacing.status = if ratio_pct > 120 {
        QuotaPacingStatus::Fast
    } else if ratio_pct < 80 {
        QuotaPacingStatus::Slow
    } else {
        QuotaPacingStatus::OnTrack
    };
    pacing
}

fn scale_usd_by_ratio(amount: UsdAmount, numerator: u64, denominator: u64) -> UsdAmount {
    if denominator == 0 || numerator == 0 || amount.is_zero() {
        return UsdAmount::ZERO;
    }
    UsdAmount::from_femto_usd(
        amount
            .femto_usd()
            .saturating_mul(i128::from(numerator))
            .saturating_div(i128::from(denominator)),
    )
}

fn amount_ratio_pct(numerator: UsdAmount, denominator: UsdAmount) -> u32 {
    if denominator.is_zero() {
        return u32::MAX;
    }
    let value = numerator
        .femto_usd()
        .saturating_mul(100)
        .saturating_div(denominator.femto_usd());
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

fn estimated_exhaustion_in_ms(remaining: UsdAmount, rate_per_hour: UsdAmount) -> Option<u64> {
    if remaining.is_zero() || rate_per_hour.is_zero() {
        return None;
    }
    let ms = remaining
        .femto_usd()
        .saturating_mul(i128::from(HOUR_MS))
        .saturating_div(rate_per_hour.femto_usd());
    u64::try_from(ms).ok().filter(|value| *value > 0)
}

fn primary_remaining_balance(
    provider_balances: &[ProviderBalanceSnapshot],
    now_ms: u64,
) -> Option<(&ProviderBalanceSnapshot, UsdAmount)> {
    provider_balances
        .iter()
        .filter(|snapshot| snapshot.error.as_deref().is_none_or(str::is_empty))
        .filter_map(|snapshot| remaining_balance(snapshot).map(|amount| (snapshot, amount)))
        .min_by(|(left, _), (right, _)| {
            balance_rank(left, now_ms)
                .cmp(&balance_rank(right, now_ms))
                .then_with(|| left.upstream_index.cmp(&right.upstream_index))
                .then_with(|| right.fetched_at_ms.cmp(&left.fetched_at_ms))
        })
}

fn primary_quota_snapshot(
    provider_balances: &[ProviderBalanceSnapshot],
    now_ms: u64,
) -> Option<&ProviderBalanceSnapshot> {
    provider_balances
        .iter()
        .filter(|snapshot| snapshot.error.as_deref().is_none_or(str::is_empty))
        .filter(|snapshot| is_package_quota_snapshot(snapshot))
        .min_by(|left, right| {
            balance_rank(left, now_ms)
                .cmp(&balance_rank(right, now_ms))
                .then_with(|| quota_rank(left).cmp(&quota_rank(right)))
                .then_with(|| left.upstream_index.cmp(&right.upstream_index))
                .then_with(|| right.fetched_at_ms.cmp(&left.fetched_at_ms))
        })
}

fn remaining_balance(snapshot: &ProviderBalanceSnapshot) -> Option<UsdAmount> {
    snapshot
        .quota_remaining_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str)
        .or_else(|| {
            snapshot
                .total_balance_usd
                .as_deref()
                .and_then(UsdAmount::from_decimal_str)
        })
}

fn is_package_quota_snapshot(snapshot: &ProviderBalanceSnapshot) -> bool {
    snapshot.unlimited_quota == Some(true)
        || snapshot.quota_period.is_some()
        || snapshot.quota_remaining_usd.is_some()
        || snapshot.quota_limit_usd.is_some()
        || snapshot.quota_used_usd.is_some()
        || (snapshot.monthly_budget_usd.is_some() && snapshot.monthly_spent_usd.is_some())
}

fn quota_remaining(snapshot: &ProviderBalanceSnapshot) -> Option<UsdAmount> {
    snapshot
        .quota_remaining_usd
        .as_deref()
        .and_then(UsdAmount::from_decimal_str)
        .or_else(|| {
            let budget = snapshot
                .monthly_budget_usd
                .as_deref()
                .and_then(UsdAmount::from_decimal_str)?;
            let spent = snapshot
                .monthly_spent_usd
                .as_deref()
                .and_then(UsdAmount::from_decimal_str)?;
            Some(budget.saturating_sub(spent))
        })
}

fn quota_rank(snapshot: &ProviderBalanceSnapshot) -> u8 {
    if snapshot.unlimited_quota == Some(true) {
        return 3;
    }
    if snapshot.quota_remaining_usd.is_some() {
        return 0;
    }
    if snapshot.monthly_budget_usd.is_some() && snapshot.monthly_spent_usd.is_some() {
        return 1;
    }
    2
}

fn quota_reset_in_ms(
    spend: &UsageSpendForecast,
    snapshot: &ProviderBalanceSnapshot,
) -> Option<u64> {
    let period = snapshot
        .quota_period
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    period
        .eq_ignore_ascii_case("daily")
        .then_some(spend.reset_in_ms)
        .flatten()
        .filter(|value| *value > 0)
}

fn balance_rank(snapshot: &ProviderBalanceSnapshot, now_ms: u64) -> u8 {
    match snapshot.status_at(now_ms) {
        crate::balance::BalanceSnapshotStatus::Ok => 0,
        crate::balance::BalanceSnapshotStatus::Stale => 1,
        crate::balance::BalanceSnapshotStatus::Unknown => 2,
        crate::balance::BalanceSnapshotStatus::Error => 3,
        crate::balance::BalanceSnapshotStatus::Exhausted => 4,
    }
}

fn quota_pacing_from_snapshot(
    snapshot: &ProviderBalanceSnapshot,
    rate_per_hour: Option<UsdAmount>,
    remaining: Option<UsdAmount>,
    limit: Option<UsdAmount>,
    used: Option<UsdAmount>,
    estimated_exhaustion_in_ms: Option<u64>,
) -> QuotaPacingForecast {
    QuotaPacingForecast {
        available: true,
        provider_id: snapshot.provider_id.clone(),
        station_name: snapshot.station_name.clone(),
        upstream_index: snapshot.upstream_index,
        source: snapshot.source.clone(),
        plan_name: snapshot.plan_name.clone(),
        period: snapshot
            .quota_period
            .clone()
            .or_else(|| monthly_budget_period(snapshot)),
        unlimited: snapshot.unlimited_quota == Some(true),
        remaining_usd: remaining.map(UsdAmount::format_usd),
        limit_usd: limit.map(UsdAmount::format_usd),
        used_usd: used.map(UsdAmount::format_usd),
        rate_per_hour_usd: rate_per_hour.map(UsdAmount::format_usd),
        estimated_exhaustion_in_ms,
        ..QuotaPacingForecast::default()
    }
}

fn monthly_budget_period(snapshot: &ProviderBalanceSnapshot) -> Option<String> {
    (snapshot.monthly_budget_usd.is_some() && snapshot.monthly_spent_usd.is_some())
        .then(|| "monthly".to_string())
}

impl QuotaPacingForecast {
    fn with_status(mut self, status: QuotaPacingStatus, reason: Option<&str>) -> Self {
        self.status = status;
        self.reason = reason.map(str::to_string);
        self
    }
}

pub fn next_reset_at_ms(now_ms: u64, utc_offset: &str, reset_time: &str) -> Option<u64> {
    let offset_ms = parse_utc_offset_ms(utc_offset)?;
    let reset_ms = parse_hh_mm_ms(reset_time)?;
    let local_ms = i128::from(now_ms) + offset_ms;
    let local_day_start = div_floor(local_ms, i128::from(DAY_MS)) * i128::from(DAY_MS);
    let mut reset_local = local_day_start + i128::from(reset_ms);
    if reset_local <= local_ms {
        reset_local += i128::from(DAY_MS);
    }
    let reset_utc = reset_local - offset_ms;
    u64::try_from(reset_utc).ok()
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
    let rest = &value[1..];
    let (hour, minute) = rest.split_once(':')?;
    let hour = hour.parse::<i128>().ok()?;
    let minute = minute.parse::<i128>().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some(sign * (hour * i128::from(HOUR_MS) + minute * i128::from(MINUTE_MS)))
}

fn div_floor(a: i128, b: i128) -> i128 {
    let q = a / b;
    let r = a % b;
    if r != 0 && ((r > 0) != (b > 0)) {
        q - 1
    } else {
        q
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{CostAdjustments, ModelPrice, estimate_usage_cost_with_accounting};
    use crate::state::RequestObservability;
    use crate::usage::{CacheInputAccounting, UsageMetrics};

    fn priced_request(ended_at_ms: u64, usd: &str) -> FinishedRequest {
        let usage = UsageMetrics {
            input_tokens: 1_000_000,
            total_tokens: 1_000_000,
            ..Default::default()
        };
        let price = ModelPrice::from_per_million_usd(
            "gpt-test",
            None,
            usd,
            "0",
            Some("0"),
            Some("0"),
            "test",
        )
        .expect("test price");
        let cost = estimate_usage_cost_with_accounting(
            &usage,
            &price,
            CostAdjustments::default(),
            CacheInputAccounting::default(),
        );

        FinishedRequest {
            id: ended_at_ms,
            trace_id: None,
            session_id: None,
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: Some("gpt-test".to_string()),
            reasoning_effort: None,
            service_tier: None,
            station_name: Some("station".to_string()),
            provider_id: Some("provider".to_string()),
            upstream_base_url: None,
            route_decision: None,
            usage: Some(usage),
            cost,
            retry: None,
            observability: RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 100,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms,
        }
    }

    #[test]
    fn next_reset_uses_configured_fixed_offset_midnight() {
        let now_ms = 10 * HOUR_MS;

        let reset = next_reset_at_ms(now_ms, "+08:00", "00:00").expect("reset");

        assert_eq!(reset, 16 * HOUR_MS);
    }

    #[test]
    fn forecast_projects_current_hourly_spend_until_reset() {
        let now_ms = 10 * HOUR_MS;
        let config = UsageForecastConfig {
            rate_window_minutes: 60,
            min_priced_requests: 2,
            ..UsageForecastConfig::default()
        };
        let recent = vec![
            priced_request(now_ms - 60 * MINUTE_MS, "1"),
            priced_request(now_ms - 30 * MINUTE_MS, "1"),
        ];
        let balances = vec![ProviderBalanceSnapshot {
            provider_id: "provider".to_string(),
            quota_remaining_usd: Some("20".to_string()),
            fetched_at_ms: now_ms,
            ..ProviderBalanceSnapshot::default()
        }];

        let forecast = build_usage_spend_forecast(&config, &recent, &balances, now_ms);

        assert_eq!(forecast.rate_per_hour_usd.as_deref(), Some("2"));
        assert_eq!(forecast.reset_in_ms, Some(6 * HOUR_MS));
        assert_eq!(forecast.projected_until_reset_usd.as_deref(), Some("12"));
        assert_eq!(
            forecast.projected_balance_after_reset_usd.as_deref(),
            Some("8")
        );
        assert!(!forecast.projected_exhaustion);
        assert_eq!(forecast.confidence, UsageForecastConfidence::Estimated);
    }

    #[test]
    fn forecast_marks_exhaustion_when_projection_exceeds_remaining_balance() {
        let now_ms = 10 * HOUR_MS;
        let config = UsageForecastConfig {
            min_priced_requests: 2,
            ..UsageForecastConfig::default()
        };
        let recent = vec![
            priced_request(now_ms - 60 * MINUTE_MS, "1"),
            priced_request(now_ms - 30 * MINUTE_MS, "1"),
        ];
        let balances = vec![ProviderBalanceSnapshot {
            provider_id: "provider".to_string(),
            quota_remaining_usd: Some("2".to_string()),
            fetched_at_ms: now_ms,
            ..ProviderBalanceSnapshot::default()
        }];

        let forecast = build_usage_spend_forecast(&config, &recent, &balances, now_ms);

        assert_eq!(forecast.projected_until_reset_usd.as_deref(), Some("12"));
        assert_eq!(
            forecast.projected_balance_after_reset_usd.as_deref(),
            Some("0")
        );
        assert!(forecast.projected_exhaustion);
    }

    #[test]
    fn quota_pacing_marks_package_burn_as_fast_against_reset_target() {
        let now_ms = 10 * HOUR_MS;
        let config = UsageForecastConfig {
            rate_window_minutes: 60,
            min_priced_requests: 2,
            ..UsageForecastConfig::default()
        };
        let recent = vec![
            priced_request(now_ms - 60 * MINUTE_MS, "1"),
            priced_request(now_ms - 30 * MINUTE_MS, "1"),
        ];
        let balances = vec![ProviderBalanceSnapshot {
            provider_id: "provider".to_string(),
            station_name: Some("station".to_string()),
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("6".to_string()),
            quota_limit_usd: Some("20".to_string()),
            quota_used_usd: Some("14".to_string()),
            fetched_at_ms: now_ms,
            ..ProviderBalanceSnapshot::default()
        }];
        let spend = build_usage_spend_forecast(&config, &recent, &balances, now_ms);

        let pacing = build_quota_pacing_forecast(&spend, &balances, now_ms);

        assert!(pacing.available);
        assert_eq!(pacing.period.as_deref(), Some("daily"));
        assert_eq!(pacing.remaining_usd.as_deref(), Some("6"));
        assert_eq!(pacing.limit_usd.as_deref(), Some("20"));
        assert_eq!(pacing.rate_per_hour_usd.as_deref(), Some("2"));
        assert_eq!(pacing.target_rate_per_hour_usd.as_deref(), Some("1"));
        assert_eq!(pacing.pace_ratio_pct, Some(200));
        assert_eq!(pacing.status, QuotaPacingStatus::Fast);
    }

    #[test]
    fn spend_forecast_applies_balance_delta_calibration() {
        let now_ms = 10 * HOUR_MS;
        let config = UsageForecastConfig {
            rate_window_minutes: 60,
            min_priced_requests: 1,
            ..UsageForecastConfig::default()
        };
        let recent = vec![priced_request(now_ms - 30 * MINUTE_MS, "1")];
        let balances = vec![ProviderBalanceSnapshot {
            provider_id: "provider".to_string(),
            station_name: Some("station".to_string()),
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("8".to_string()),
            fetched_at_ms: now_ms,
            ..ProviderBalanceSnapshot::default()
        }];
        let history = vec![
            ProviderBalanceSnapshot {
                provider_id: "provider".to_string(),
                station_name: Some("station".to_string()),
                upstream_index: Some(0),
                quota_period: Some("daily".to_string()),
                quota_remaining_usd: Some("10".to_string()),
                fetched_at_ms: now_ms - 60 * MINUTE_MS,
                ..ProviderBalanceSnapshot::default()
            },
            ProviderBalanceSnapshot {
                provider_id: "provider".to_string(),
                station_name: Some("station".to_string()),
                upstream_index: Some(0),
                quota_period: Some("daily".to_string()),
                quota_remaining_usd: Some("8".to_string()),
                fetched_at_ms: now_ms,
                ..ProviderBalanceSnapshot::default()
            },
        ];

        let forecast = build_usage_spend_forecast_with_balance_history(
            &config, &recent, &balances, &history, now_ms,
        );

        assert!(forecast.balance_calibrated);
        assert_eq!(forecast.balance_calibration_multiplier_pct, Some(200));
        assert_eq!(forecast.rate_per_hour_usd.as_deref(), Some("4"));
        assert_eq!(forecast.sample_cost_usd.as_deref(), Some("2"));
    }

    #[test]
    fn quota_pacing_uses_remaining_eta_when_reset_is_unknown() {
        let now_ms = 10 * HOUR_MS;
        let spend = UsageSpendForecast {
            enabled: true,
            rate_per_hour_usd: Some("2".to_string()),
            ..UsageSpendForecast::default()
        };
        let balances = vec![ProviderBalanceSnapshot {
            provider_id: "provider".to_string(),
            quota_period: Some("daily".to_string()),
            quota_remaining_usd: Some("6".to_string()),
            quota_limit_usd: Some("20".to_string()),
            fetched_at_ms: now_ms,
            ..ProviderBalanceSnapshot::default()
        }];

        let pacing = build_quota_pacing_forecast(&spend, &balances, now_ms);

        assert_eq!(pacing.status, QuotaPacingStatus::UnknownReset);
        assert_eq!(pacing.estimated_exhaustion_in_ms, Some(3 * HOUR_MS));
        assert_eq!(pacing.target_rate_per_hour_usd, None);
    }

    #[test]
    fn quota_pacing_does_not_apply_daily_reset_to_monthly_budget() {
        let now_ms = 10 * HOUR_MS;
        let spend = UsageSpendForecast {
            enabled: true,
            rate_per_hour_usd: Some("2".to_string()),
            reset_in_ms: Some(6 * HOUR_MS),
            ..UsageSpendForecast::default()
        };
        let balances = vec![ProviderBalanceSnapshot {
            provider_id: "provider".to_string(),
            monthly_budget_usd: Some("20".to_string()),
            monthly_spent_usd: Some("14".to_string()),
            fetched_at_ms: now_ms,
            ..ProviderBalanceSnapshot::default()
        }];

        let pacing = build_quota_pacing_forecast(&spend, &balances, now_ms);

        assert_eq!(pacing.status, QuotaPacingStatus::UnknownReset);
        assert_eq!(pacing.period.as_deref(), Some("monthly"));
        assert_eq!(pacing.remaining_usd.as_deref(), Some("6"));
        assert_eq!(pacing.estimated_exhaustion_in_ms, Some(3 * HOUR_MS));
        assert_eq!(pacing.target_rate_per_hour_usd, None);
    }

    #[test]
    fn forecast_keeps_burn_rate_but_suppresses_projection_for_low_sample() {
        let now_ms = 10 * HOUR_MS;
        let config = UsageForecastConfig {
            min_priced_requests: 2,
            ..UsageForecastConfig::default()
        };
        let recent = vec![priced_request(now_ms - 30 * MINUTE_MS, "1")];

        let forecast = build_usage_spend_forecast(&config, &recent, &[], now_ms);

        assert_eq!(forecast.rate_per_hour_usd.as_deref(), Some("2"));
        assert_eq!(forecast.confidence, UsageForecastConfidence::LowSample);
        assert_eq!(forecast.projected_until_reset_usd, None);
        assert_eq!(forecast.projected_sample_elapsed_ms, 0);
        assert!(!forecast.projected_exhaustion);
    }
}
