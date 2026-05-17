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
    let projected_until_reset =
        reset_in_ms.map(|reset_in_ms| scale_usd_by_ratio(rate_per_hour, reset_in_ms, HOUR_MS));
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
    let confidence = if priced_requests < config.min_priced_requests.max(1) {
        UsageForecastConfidence::LowSample
    } else if unpriced_requests > 0 {
        UsageForecastConfidence::PartialPricing
    } else {
        UsageForecastConfidence::Estimated
    };

    UsageSpendForecast {
        enabled: true,
        sample_window_ms,
        sample_elapsed_ms,
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

fn balance_rank(snapshot: &ProviderBalanceSnapshot, now_ms: u64) -> u8 {
    match snapshot.status_at(now_ms) {
        crate::balance::BalanceSnapshotStatus::Ok => 0,
        crate::balance::BalanceSnapshotStatus::Stale => 1,
        crate::balance::BalanceSnapshotStatus::Unknown => 2,
        crate::balance::BalanceSnapshotStatus::Error => 3,
        crate::balance::BalanceSnapshotStatus::Exhausted => 4,
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
            min_priced_requests: 1,
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
        let config = UsageForecastConfig::default();
        let recent = vec![priced_request(now_ms - 60 * MINUTE_MS, "1")];
        let balances = vec![ProviderBalanceSnapshot {
            provider_id: "provider".to_string(),
            quota_remaining_usd: Some("2".to_string()),
            fetched_at_ms: now_ms,
            ..ProviderBalanceSnapshot::default()
        }];

        let forecast = build_usage_spend_forecast(&config, &recent, &balances, now_ms);

        assert_eq!(forecast.projected_until_reset_usd.as_deref(), Some("6"));
        assert_eq!(
            forecast.projected_balance_after_reset_usd.as_deref(),
            Some("0")
        );
        assert!(forecast.projected_exhaustion);
    }
}
