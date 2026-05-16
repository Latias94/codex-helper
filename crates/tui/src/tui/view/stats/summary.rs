use std::collections::HashMap;

use ratatui::prelude::{Line, Span, Style};
use unicode_width::UnicodeWidthStr;

use crate::dashboard_core::WindowStats;
use crate::pricing::CostConfidence;
use crate::state::{BalanceSnapshotStatus, ProviderBalanceSnapshot, UsageBucket};
use crate::tui::Language;
use crate::tui::i18n;
use crate::tui::model::{
    Palette, Snapshot, duration_short, provider_balance_compact_lang, shorten_middle,
    station_balance_brief_lang,
};
use crate::tui::types::StatsFocus;
use crate::usage::UsageMetrics;
use crate::usage_balance::{
    UsageBalanceEndpointRow, UsageBalanceProviderRow, UsageBalanceStatus, UsageBalanceStatusCounts,
    UsageBalanceView,
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

pub(super) fn stats_coverage_line(
    p: Palette,
    snapshot: &Snapshot,
    window_label: &str,
    lang: Language,
) -> Line<'static> {
    let c = &snapshot.usage_rollup.coverage;
    let loaded = day_range_label(c.loaded_first_day, c.loaded_last_day);
    let window = day_range_label(c.window_first_day, c.window_last_day);
    let warning = if c.window_exceeds_loaded_start {
        match lang {
            Language::Zh => "  部分覆盖：所选窗口早于已加载日志数据",
            Language::En => "  partial: selected window starts before loaded log data",
        }
    } else {
        ""
    };

    Line::from(vec![
        Span::styled(
            format!("{} {window_label} {window}", i18n::label(lang, "window")),
            Style::default().fg(p.text),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{} {loaded} days={} req={}",
                i18n::label(lang, "loaded"),
                c.loaded_days_with_data,
                c.loaded_requests
            ),
            Style::default().fg(p.muted),
        ),
        Span::styled(warning.to_string(), Style::default().fg(p.warn)),
    ])
}

pub(super) fn live_health_line(stats: &WindowStats, lang: Language) -> String {
    if stats.total == 0 {
        return "-".to_string();
    }
    format!(
        "{} {} p95 {} retry {} 429 {} 5xx {} n={}",
        i18n::label(lang, "ok"),
        fmt_pct(stats.ok_2xx as u64, stats.total as u64),
        stats
            .p95_ms
            .map(duration_short)
            .unwrap_or_else(|| "-".to_string()),
        stats
            .retry_rate
            .map(|rate| format!("{:.0}%", rate * 100.0))
            .unwrap_or_else(|| "-".to_string()),
        stats.err_429,
        stats.err_5xx,
        stats.total
    )
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
