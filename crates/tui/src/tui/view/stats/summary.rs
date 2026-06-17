use std::collections::HashMap;

use ratatui::prelude::Style;
use unicode_width::UnicodeWidthStr;

use crate::pricing::CostConfidence;
use crate::state::{BalanceSnapshotStatus, ProviderBalanceSnapshot, UsageBucket};
use crate::tui::Language;
use crate::tui::i18n;
use crate::tui::model::{
    Palette, Snapshot, UsageForecastSampleSource, duration_short, provider_balance_compact_lang,
    shorten_middle, station_balance_brief_lang,
};
use crate::tui::types::StatsFocus;
use crate::usage::UsageMetrics;
use crate::usage_balance::{
    UsageBalanceEndpointRow, UsageBalanceProviderRow, UsageBalanceStatus, UsageBalanceStatusCounts,
    UsageBalanceView,
};
use crate::usage_forecast::{
    QuotaPacingForecast, QuotaPacingStatus, UsageForecastConfidence, UsageSpendForecast,
    build_quota_pacing_forecast, build_usage_spend_forecast,
};

pub(super) const STATS_BALANCE_COLUMN_WIDTH: u16 = 14;
pub(super) const STATS_ENDPOINT_BALANCE_COLUMN_WIDTH: u16 = 28;

pub(super) fn stats_window_label(days: usize, lang: Language) -> String {
    match days {
        0 => i18n::label(lang, "loaded").to_string(),
        1 => i18n::label(lang, "today").to_string(),
        n => format!("{n}d"),
    }
}

pub(super) fn fmt_pct(num: u64, den: u64) -> String {
    if den == 0 {
        return "-".to_string();
    }
    format!("{:.1}%", (num as f64) * 100.0 / (den as f64))
}

pub(super) fn fmt_per_mille(value: Option<u16>) -> String {
    value
        .map(|value| format!("{:.1}%", f64::from(value) / 10.0))
        .unwrap_or_else(|| "-".to_string())
}

pub(super) fn fmt_success_pct(bucket: &UsageBucket) -> String {
    fmt_pct(
        bucket.requests_total.saturating_sub(bucket.requests_error),
        bucket.requests_total,
    )
}

pub(super) fn usage_balance_status_label(
    status: UsageBalanceStatus,
    lang: Language,
) -> &'static str {
    match status {
        UsageBalanceStatus::Ok => i18n::label(lang, "ok"),
        UsageBalanceStatus::Unlimited => i18n::label(lang, "unlimited"),
        UsageBalanceStatus::Exhausted => i18n::label(lang, "exhausted"),
        UsageBalanceStatus::Stale => i18n::label(lang, "stale"),
        UsageBalanceStatus::Error => i18n::label(lang, "error"),
        UsageBalanceStatus::Unknown => i18n::label(lang, "unknown"),
    }
}

pub(super) fn usage_balance_status_style(p: Palette, status: UsageBalanceStatus) -> Style {
    match status {
        UsageBalanceStatus::Ok => Style::default().fg(p.good),
        UsageBalanceStatus::Unlimited => Style::default().fg(p.accent),
        UsageBalanceStatus::Exhausted => Style::default().fg(p.bad),
        UsageBalanceStatus::Stale | UsageBalanceStatus::Error => Style::default().fg(p.warn),
        UsageBalanceStatus::Unknown => Style::default().fg(p.muted),
    }
}

pub(super) fn provider_balance_status_style(p: Palette, row: &UsageBalanceProviderRow) -> Style {
    if row
        .primary_balance
        .as_ref()
        .is_some_and(|balance| balance.routing_ignored_exhaustion)
    {
        Style::default().fg(p.warn)
    } else {
        usage_balance_status_style(p, row.balance_status)
    }
}

pub(super) fn provider_usage_balance_status_label(
    row: &UsageBalanceProviderRow,
    compact: bool,
    lang: Language,
) -> &'static str {
    if row
        .primary_balance
        .as_ref()
        .is_some_and(|balance| balance.routing_ignored_exhaustion)
    {
        if compact {
            i18n::label(lang, "lazy")
        } else {
            i18n::label(lang, "lazy reset")
        }
    } else {
        usage_balance_status_label(row.balance_status, lang)
    }
}

pub(super) fn endpoint_balance_status_style(p: Palette, row: &UsageBalanceEndpointRow) -> Style {
    if row
        .balance
        .as_ref()
        .is_some_and(|balance| balance.routing_ignored_exhaustion)
    {
        Style::default().fg(p.warn)
    } else {
        usage_balance_status_style(p, row.balance_status)
    }
}

pub(super) fn usage_balance_counts_line(
    counts: &UsageBalanceStatusCounts,
    lang: Language,
) -> String {
    let mut parts = Vec::new();
    if counts.ok > 0 {
        parts.push(format!("{} {}", i18n::label(lang, "ok"), counts.ok));
    }
    if counts.unlimited > 0 {
        parts.push(format!(
            "{} {}",
            i18n::label(lang, "unlimited"),
            counts.unlimited
        ));
    }
    if counts.exhausted > 0 {
        parts.push(format!("{} {}", i18n::label(lang, "exh"), counts.exhausted));
    }
    if counts.stale > 0 {
        parts.push(format!("{} {}", i18n::label(lang, "stale"), counts.stale));
    }
    if counts.error > 0 {
        parts.push(format!("{} {}", i18n::label(lang, "error"), counts.error));
    }
    if counts.unknown > 0 {
        parts.push(format!(
            "{} {}",
            i18n::label(lang, "unknown"),
            counts.unknown
        ));
    }
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
pub(super) fn usage_refresh_line(view: &UsageBalanceView, lang: Language) -> String {
    if view.refresh_status.refreshing {
        return match lang {
            Language::Zh => "刷新中".to_string(),
            Language::En => "refreshing".to_string(),
        };
    }
    if let Some(err) = view.refresh_status.last_error.as_deref() {
        return format!("{}: {err}", i18n::label(lang, "error"));
    }
    let mut parts = Vec::new();
    if let Some(summary) = view.refresh_status.last_provider_refresh.as_ref() {
        parts.push(balance_refresh_summary_line(summary, lang));
    }
    if let Some(err) = view.refresh_status.latest_error.as_deref() {
        let source = view
            .refresh_status
            .latest_error_provider_id
            .as_deref()
            .unwrap_or("-");
        parts.push(format!(
            "{} {}: {err}",
            i18n::label(lang, "latest error"),
            source
        ));
    } else if let Some(msg) = view.refresh_status.last_message.as_deref() {
        parts.push(msg.to_string());
    }
    if parts.is_empty() {
        format!(
            "{}={}",
            i18n::label(lang, "snapshots"),
            view.refresh_status.total_snapshots
        )
    } else {
        parts.join(" · ")
    }
}

pub(super) fn usage_refresh_brief_line(view: &UsageBalanceView, lang: Language) -> String {
    if view.refresh_status.refreshing {
        return match lang {
            Language::Zh => "刷新中".to_string(),
            Language::En => "refreshing".to_string(),
        };
    }
    if let Some(err) = view.refresh_status.last_error.as_deref() {
        return format!("{}: {err}", i18n::label(lang, "error"));
    }

    let mut parts = Vec::new();
    if let Some(summary) = view.refresh_status.last_provider_refresh.as_ref() {
        parts.push(balance_refresh_summary_brief_line(summary, lang));
    }
    if view.refresh_status.latest_error.is_some() {
        let source = view
            .refresh_status
            .latest_error_provider_id
            .as_deref()
            .unwrap_or("-");
        parts.push(format!("{} {source}", i18n::label(lang, "err")));
    } else if let Some(msg) = view.refresh_status.last_message.as_deref() {
        parts.push(msg.to_string());
    }

    if parts.is_empty() {
        format!(
            "{}={}",
            i18n::label(lang, "snapshots"),
            view.refresh_status.total_snapshots
        )
    } else {
        parts.join(" · ")
    }
}

fn balance_refresh_summary_brief_line(
    summary: &crate::usage_providers::UsageProviderRefreshSummary,
    lang: Language,
) -> String {
    if summary.deduplicated > 0 && summary.attempted == 0 {
        return match lang {
            Language::Zh => "刷新已在进行".to_string(),
            Language::En => "refresh requested".to_string(),
        };
    }

    match lang {
        Language::Zh => {
            let mut parts = vec![format!("成功 {}/{}", summary.refreshed, summary.attempted)];
            if summary.failed > 0 {
                parts.push(format!("失败 {}", summary.failed));
            }
            if summary.missing_token > 0 {
                parts.push(format!("缺 key {}", summary.missing_token));
            }
            parts.join(" ")
        }
        Language::En => {
            let mut parts = vec![format!("ok {}/{}", summary.refreshed, summary.attempted)];
            if summary.failed > 0 {
                parts.push(format!("fail {}", summary.failed));
            }
            if summary.missing_token > 0 {
                parts.push(format!("no-key {}", summary.missing_token));
            }
            parts.join(" ")
        }
    }
}

#[cfg(test)]
pub(super) fn balance_refresh_summary_line(
    summary: &crate::usage_providers::UsageProviderRefreshSummary,
    lang: Language,
) -> String {
    if summary.deduplicated > 0 && summary.attempted == 0 {
        return match lang {
            Language::Zh => "余额刷新已在进行中".to_string(),
            Language::En => "balance refresh already requested".to_string(),
        };
    }
    match lang {
        Language::Zh => {
            let mut parts = vec![format!("成功 {}/{}", summary.refreshed, summary.attempted)];
            if summary.failed > 0 {
                parts.push(format!("失败 {}", summary.failed));
            }
            if summary.missing_token > 0 {
                parts.push(format!("缺 key {}", summary.missing_token));
            }
            if summary.auto_attempted > 0 {
                parts.push(format!(
                    "自动 {}/{}",
                    summary.auto_refreshed, summary.auto_attempted
                ));
            }
            if summary.deduplicated > 0 {
                parts.push(format!("去重 {}", summary.deduplicated));
            }
            parts.join(" · ")
        }
        Language::En => {
            let mut parts = vec![format!("ok {}/{}", summary.refreshed, summary.attempted)];
            if summary.failed > 0 {
                parts.push(format!("failed {}", summary.failed));
            }
            if summary.missing_token > 0 {
                parts.push(format!("missing key {}", summary.missing_token));
            }
            if summary.auto_attempted > 0 {
                parts.push(format!(
                    "auto {}/{}",
                    summary.auto_refreshed, summary.auto_attempted
                ));
            }
            if summary.deduplicated > 0 {
                parts.push(format!("dedup {}", summary.deduplicated));
            }
            parts.join(" · ")
        }
    }
}

pub(super) fn fmt_avg_ms(total_ms: u64, n: u64) -> String {
    if n == 0 {
        return "-".to_string();
    }
    duration_short(total_ms / n)
}

pub(super) fn cost_confidence_label(confidence: CostConfidence, lang: Language) -> &'static str {
    match confidence {
        CostConfidence::Unknown => i18n::label(lang, "unknown"),
        CostConfidence::Partial => i18n::label(lang, "partial"),
        CostConfidence::Estimated => i18n::label(lang, "estimated"),
        CostConfidence::Exact => i18n::label(lang, "exact"),
    }
}

pub(super) fn calc_output_rate_tok_s(bucket: &UsageBucket) -> Option<f64> {
    let output = bucket.usage.output_tokens.max(0) as f64;
    if output <= 0.0 || bucket.generation_ms_total == 0 {
        return None;
    }
    Some(output / (bucket.generation_ms_total as f64 / 1000.0))
}

pub(super) fn fmt_tok_s_0(rate: Option<f64>) -> String {
    let Some(v) = rate.filter(|v| v.is_finite() && *v > 0.0) else {
        return "-".to_string();
    };
    format!("{:.0}", v)
}

pub(super) fn fmt_avg_ttfb_ms(bucket: &UsageBucket) -> String {
    if bucket.ttfb_samples == 0 {
        return "-".to_string();
    }
    duration_short(bucket.ttfb_ms_total / bucket.ttfb_samples)
}

pub(super) fn fmt_avg_generation_ms(bucket: &UsageBucket) -> String {
    if bucket.requests_with_usage == 0 || bucket.generation_ms_total == 0 {
        return "-".to_string();
    }
    duration_short(bucket.generation_ms_total / bucket.requests_with_usage)
}

pub(super) fn cache_hit_rate(usage: &UsageMetrics) -> Option<f64> {
    usage.cache_hit_rate()
}

pub(super) fn fmt_cache_hit(usage: &UsageMetrics) -> String {
    cache_hit_rate(usage)
        .map(|rate| format!("{:.1}%", rate * 100.0))
        .unwrap_or_else(|| "-".to_string())
}

pub(super) fn cost_coverage_label(bucket: &UsageBucket, lang: Language) -> String {
    if bucket.requests_with_usage == 0 {
        return "-".to_string();
    }
    format!(
        "{}/{} {}",
        bucket.cost.priced_requests,
        bucket.requests_with_usage,
        cost_confidence_label(bucket.cost.confidence, lang)
    )
}

pub(super) fn day_to_ymd(day: i32) -> String {
    let z = i64::from(day) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    format!("{year:04}-{m:02}-{d:02}")
}

pub(super) fn day_range_label(first: Option<i32>, last: Option<i32>) -> String {
    match (first, last) {
        (Some(first), Some(last)) if first == last => day_to_ymd(first),
        (Some(first), Some(last)) => format!("{}..{}", day_to_ymd(first), day_to_ymd(last)),
        _ => "-".to_string(),
    }
}

pub(super) fn usage_spend_forecast(
    snapshot: &Snapshot,
    config: &crate::config::UsageForecastConfig,
    now_ms: u64,
) -> UsageSpendForecast {
    let balances = snapshot
        .provider_balances
        .values()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let recent = match snapshot.forecast_recent_source {
        UsageForecastSampleSource::RuntimeOnly => &snapshot.recent,
        UsageForecastSampleSource::RuntimeAndRequestLedger => &snapshot.forecast_recent,
    };
    build_usage_spend_forecast(config, recent, &balances, now_ms)
}

pub(super) fn quota_pacing_forecast(
    snapshot: &Snapshot,
    spend: &UsageSpendForecast,
    now_ms: u64,
) -> QuotaPacingForecast {
    let balances = snapshot
        .provider_balances
        .values()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    build_quota_pacing_forecast(spend, &balances, now_ms)
}

pub(super) fn spend_forecast_rate_line(forecast: &UsageSpendForecast, lang: Language) -> String {
    if !forecast.enabled {
        return match lang {
            Language::Zh => "预测已关闭".to_string(),
            Language::En => "forecast disabled".to_string(),
        };
    }
    if forecast.rate_per_hour_usd.is_none() {
        return forecast.reason.clone().unwrap_or_else(|| match lang {
            Language::Zh => "暂无可计价样本".to_string(),
            Language::En => "no priced sample".to_string(),
        });
    }

    let rate = fmt_usd_compact(forecast.rate_per_hour_usd.as_deref());
    let reset = forecast
        .reset_in_ms
        .map(duration_short)
        .unwrap_or_else(|| "-".to_string());
    let confidence = spend_forecast_confidence_label(forecast.confidence, lang);

    if forecast.confidence == UsageForecastConfidence::LowSample {
        return match lang {
            Language::Zh => format!("速率 {rate}/h  样本少，不外推到0点 ({reset})"),
            Language::En => format!("rate {rate}/h  low sample, not projecting to reset ({reset})"),
        };
    }

    let projected = fmt_usd_compact(forecast.projected_until_reset_usd.as_deref());
    match lang {
        Language::Zh => {
            format!("速率 {rate}/h  按当前速率到0点≈{projected} ({reset}, {confidence})")
        }
        Language::En => {
            format!("rate {rate}/h  at current rate to reset≈{projected} ({reset}, {confidence})")
        }
    }
}

pub(super) fn spend_forecast_balance_line(forecast: &UsageSpendForecast, lang: Language) -> String {
    if forecast.rate_per_hour_usd.is_none() {
        return "-".to_string();
    }
    let sample = match lang {
        Language::Zh => "样本",
        Language::En => "sample",
    };
    let confidence = spend_forecast_confidence_label(forecast.confidence, lang);
    let sample_prefix = format!("{sample} {} req {confidence}", forecast.priced_requests);

    match (
        forecast.primary_balance_remaining_usd.as_deref(),
        forecast.projected_balance_after_reset_usd.as_deref(),
    ) {
        (Some(left), Some(after)) if forecast.projected_exhaustion => match lang {
            Language::Zh => format!(
                "{sample_prefix}  可能耗尽: {} -> {}",
                fmt_usd_compact(Some(left)),
                fmt_usd_compact(Some(after))
            ),
            Language::En => format!(
                "{sample_prefix}  may exhaust: {} -> {}",
                fmt_usd_compact(Some(left)),
                fmt_usd_compact(Some(after))
            ),
        },
        (Some(left), Some(after)) => match lang {
            Language::Zh => format!(
                "{sample_prefix}  余额 {} -> {}",
                fmt_usd_compact(Some(left)),
                fmt_usd_compact(Some(after))
            ),
            Language::En => format!(
                "{sample_prefix}  left {} -> {}",
                fmt_usd_compact(Some(left)),
                fmt_usd_compact(Some(after))
            ),
        },
        _ => sample_prefix,
    }
}

pub(super) fn quota_pacing_plan_line(pacing: &QuotaPacingForecast, lang: Language) -> String {
    if !pacing.available {
        return match lang {
            Language::Zh => "未识别到套餐额度".to_string(),
            Language::En => "no package quota detected".to_string(),
        };
    }
    if pacing.unlimited {
        return match lang {
            Language::Zh => "套餐 unlimited，仅显示当前速度".to_string(),
            Language::En => "unlimited package, showing burn only".to_string(),
        };
    }

    let period = quota_period_label(pacing.period.as_deref(), lang);
    let remaining = fmt_usd_compact(pacing.remaining_usd.as_deref());
    let amount = match pacing.limit_usd.as_deref() {
        Some(limit) => format!("{remaining}/{}", fmt_usd_compact(Some(limit))),
        None => remaining,
    };
    match lang {
        Language::Zh => format!("{period} 套餐 剩余 {amount}"),
        Language::En => format!("{period} package left {amount}"),
    }
}

pub(super) fn quota_pacing_status_line(pacing: &QuotaPacingForecast, lang: Language) -> String {
    if !pacing.available {
        return pacing.reason.clone().unwrap_or_else(|| match lang {
            Language::Zh => "暂无套餐节奏数据".to_string(),
            Language::En => "no package pacing data".to_string(),
        });
    }

    let rate = fmt_usd_compact(pacing.rate_per_hour_usd.as_deref());
    match pacing.status {
        QuotaPacingStatus::Unlimited => match lang {
            Language::Zh => format!("当前 {rate}/h，无耗尽预估"),
            Language::En => format!("current {rate}/h, no exhaustion ETA"),
        },
        QuotaPacingStatus::Exhausted => match lang {
            Language::Zh => "套餐已耗尽".to_string(),
            Language::En => "package exhausted".to_string(),
        },
        QuotaPacingStatus::NoSpendRate => match lang {
            Language::Zh => "暂无可计价样本，无法估算节奏".to_string(),
            Language::En => "no priced sample, pacing unknown".to_string(),
        },
        QuotaPacingStatus::UnknownReset => {
            let eta = pacing
                .estimated_exhaustion_in_ms
                .map(duration_short)
                .unwrap_or_else(|| "-".to_string());
            match lang {
                Language::Zh => format!("当前 {rate}/h，预计还能 {eta}"),
                Language::En => format!("current {rate}/h, ETA {eta}"),
            }
        }
        QuotaPacingStatus::OnTrack | QuotaPacingStatus::Fast | QuotaPacingStatus::Slow => {
            let target = fmt_usd_compact(pacing.target_rate_per_hour_usd.as_deref());
            let reset = pacing
                .reset_in_ms
                .map(duration_short)
                .unwrap_or_else(|| "-".to_string());
            let status = quota_pacing_status_label(pacing.status, pacing.pace_ratio_pct, lang);
            match lang {
                Language::Zh => format!("当前 {rate}/h 目标 {target}/h {status} ({reset})"),
                Language::En => format!("current {rate}/h target {target}/h {status} ({reset})"),
            }
        }
        QuotaPacingStatus::Unavailable => pacing.reason.clone().unwrap_or_else(|| match lang {
            Language::Zh => "套餐额度不可用".to_string(),
            Language::En => "package quota unavailable".to_string(),
        }),
    }
}

fn fmt_usd_compact(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "-".to_string();
    };
    let Ok(parsed) = value.trim().parse::<f64>() else {
        return format!("${value}");
    };
    if parsed == 0.0 {
        "$0".to_string()
    } else if parsed.abs() < 0.01 {
        format!("${parsed:.4}")
    } else if parsed.abs() < 100.0 {
        format!("${parsed:.2}")
    } else {
        format!("${parsed:.0}")
    }
}

fn quota_period_label(period: Option<&str>, lang: Language) -> String {
    let Some(period) = period.map(str::trim).filter(|value| !value.is_empty()) else {
        return match lang {
            Language::Zh => "quota".to_string(),
            Language::En => "quota".to_string(),
        };
    };
    match (period, lang) {
        ("daily", Language::Zh) => "每日".to_string(),
        ("daily", Language::En) => "daily".to_string(),
        ("weekly", Language::Zh) => "每周".to_string(),
        ("weekly", Language::En) => "weekly".to_string(),
        ("monthly", Language::Zh) => "每月".to_string(),
        ("monthly", Language::En) => "monthly".to_string(),
        ("quota", _) => "quota".to_string(),
        (period, _) => period.to_string(),
    }
}

fn quota_pacing_status_label(
    status: QuotaPacingStatus,
    pace_ratio_pct: Option<u32>,
    lang: Language,
) -> String {
    let ratio = pace_ratio_pct
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "-".to_string());
    match (status, lang) {
        (QuotaPacingStatus::Fast, Language::Zh) => format!("偏快 {ratio}"),
        (QuotaPacingStatus::Fast, Language::En) => format!("fast {ratio}"),
        (QuotaPacingStatus::Slow, Language::Zh) => format!("偏慢 {ratio}"),
        (QuotaPacingStatus::Slow, Language::En) => format!("slow {ratio}"),
        (QuotaPacingStatus::OnTrack, Language::Zh) => format!("正常 {ratio}"),
        (QuotaPacingStatus::OnTrack, Language::En) => format!("on-track {ratio}"),
        (_, Language::Zh) => "未知".to_string(),
        (_, Language::En) => "unknown".to_string(),
    }
}

fn spend_forecast_confidence_label(
    confidence: UsageForecastConfidence,
    lang: Language,
) -> &'static str {
    match confidence {
        UsageForecastConfidence::Disabled => match lang {
            Language::Zh => "关闭",
            Language::En => "disabled",
        },
        UsageForecastConfidence::NoData => match lang {
            Language::Zh => "无数据",
            Language::En => "no-data",
        },
        UsageForecastConfidence::LowSample => match lang {
            Language::Zh => "样本少",
            Language::En => "low-sample",
        },
        UsageForecastConfidence::PartialPricing => match lang {
            Language::Zh => "部分计价",
            Language::En => "partial",
        },
        UsageForecastConfidence::Estimated => match lang {
            Language::Zh => "估算",
            Language::En => "estimated",
        },
    }
}

pub(super) fn balance_status_rank(status: BalanceSnapshotStatus) -> u8 {
    match status {
        BalanceSnapshotStatus::Ok => 0,
        BalanceSnapshotStatus::Stale => 1,
        BalanceSnapshotStatus::Unknown | BalanceSnapshotStatus::Error => 2,
        BalanceSnapshotStatus::Exhausted => 3,
    }
}

pub(super) fn provider_primary_balance<'a>(
    provider_balances: &'a HashMap<String, Vec<ProviderBalanceSnapshot>>,
    provider_id: &str,
) -> Option<&'a ProviderBalanceSnapshot> {
    let mut matches = provider_balances
        .iter()
        .flat_map(|(key, balances)| {
            balances.iter().filter(move |snapshot| {
                snapshot.provider_id == provider_id
                    || (snapshot.provider_id.trim().is_empty() && key == provider_id)
            })
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        balance_status_rank(left.status)
            .cmp(&balance_status_rank(right.status))
            .then_with(|| left.upstream_index.cmp(&right.upstream_index))
            .then_with(|| right.fetched_at_ms.cmp(&left.fetched_at_ms))
    });
    matches.into_iter().next()
}

pub(super) fn provider_balance_brief(
    snapshot: &Snapshot,
    provider_id: &str,
    max_width: usize,
    lang: Language,
) -> String {
    provider_primary_balance(&snapshot.provider_balances, provider_id)
        .map(|balance| provider_balance_compact_lang(balance, max_width, lang))
        .unwrap_or_else(|| "-".to_string())
}

pub(super) fn filter_suffix(attention_only: bool, lang: Language) -> String {
    if attention_only {
        format!(" · {}", i18n::label(lang, "attention only"))
    } else {
        String::new()
    }
}

pub(super) fn atomic_summary_or_status(
    summary: &str,
    status: UsageBalanceStatus,
    max_width: usize,
    lang: Language,
) -> String {
    let summary = summary.trim();
    if !summary.is_empty() && UnicodeWidthStr::width(summary) <= max_width {
        return summary.to_string();
    }
    let status = usage_balance_status_label(status, lang);
    if UnicodeWidthStr::width(status) <= max_width {
        return status.to_string();
    }
    shorten_middle(status, max_width)
}

pub(super) fn table_balance_brief(
    snapshot: &Snapshot,
    focus: StatsFocus,
    name: &str,
    lang: Language,
) -> String {
    match focus {
        StatsFocus::Stations => station_balance_brief_lang(
            &snapshot.provider_balances,
            name,
            usize::from(STATS_BALANCE_COLUMN_WIDTH),
            lang,
        ),
        StatsFocus::Providers => provider_balance_brief(
            snapshot,
            name,
            usize::from(STATS_BALANCE_COLUMN_WIDTH),
            lang,
        ),
    }
}
