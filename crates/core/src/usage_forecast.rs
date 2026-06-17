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
    pub confidence: UsageForecastConfidence,
    pub reason: Option<String>,
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

pub fn build_usage_spend_forecast(
    config: &UsageForecastConfig,
    recent: &[FinishedRequest],
    provider_balances: &[ProviderBalanceSnapshot],
    now_ms: u64,
) -> UsageSpendForecast {
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
        if request.ended_at_ms < cutoff_ms || request.ended_at_ms > now_ms {
            continue;
        }
        if request.cost.total_cost_usd.is_some() {
            if let Some(cost) = request
                .cost
                .total_cost_usd
                .as_deref()
                .and_then(UsdAmount::from_decimal_str)
            {
                sample_cost = sample_cost.saturating_add(cost);
                priced_requests = priced_requests.saturating_add(1);
                oldest_sample_ms = Some(
                    oldest_sample_ms
                        .map(|oldest| oldest.min(request.ended_at_ms))
                        .unwrap_or(request.ended_at_ms),
                );
            } else {
                unpriced_requests = unpriced_requests.saturating_add(1);
            }
        } else if request.usage.is_some() {
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
