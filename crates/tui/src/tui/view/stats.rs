use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Sparkline, Table, Wrap};

use crate::dashboard_core::WindowStats;
use crate::pricing::CostConfidence;
use crate::state::{BalanceSnapshotStatus, ProviderBalanceSnapshot, UsageBucket};
use crate::tui::Language;
use crate::tui::ProviderOption;
use crate::tui::i18n;
use crate::tui::model::{
    Palette, Snapshot, duration_short, now_ms, provider_balance_compact_lang, shorten,
    shorten_middle, station_balance_brief_lang, tokens_short,
};
use crate::tui::state::UiState;
use crate::tui::types::StatsFocus;
use crate::usage::UsageMetrics;
use crate::usage_balance::{
    UsageBalanceBuildInput, UsageBalanceEndpointRow, UsageBalanceProviderRow,
    UsageBalanceRefreshInput, UsageBalanceStatus, UsageBalanceStatusCounts, UsageBalanceView,
};

const STATS_BALANCE_COLUMN_WIDTH: u16 = 14;

fn build_usage_balance_view(ui: &UiState, snapshot: &Snapshot) -> UsageBalanceView {
    UsageBalanceView::build(UsageBalanceBuildInput {
        service_name: ui.service_name,
        window_days: ui.stats_days,
        generated_at_ms: now_ms(),
        usage_rollup: &snapshot.usage_rollup,
        provider_balances: &snapshot.provider_balances,
        recent: &snapshot.recent,
        routing_explain: ui.routing_explain.as_ref(),
        refresh: UsageBalanceRefreshInput {
            refreshing: ui.balance_refresh_in_flight,
            last_message: ui.last_balance_refresh_message.clone(),
            last_error: ui.last_balance_refresh_error.clone(),
        },
    })
}

fn stats_window_label(days: usize, lang: Language) -> String {
    match days {
        0 => i18n::label(lang, "loaded").to_string(),
        1 => i18n::label(lang, "today").to_string(),
        n => format!("{n}d"),
    }
}

fn fmt_pct(num: u64, den: u64) -> String {
    if den == 0 {
        return "-".to_string();
    }
    format!("{:.1}%", (num as f64) * 100.0 / (den as f64))
}

fn fmt_per_mille(value: Option<u16>) -> String {
    value
        .map(|value| format!("{:.1}%", f64::from(value) / 10.0))
        .unwrap_or_else(|| "-".to_string())
}

fn fmt_success_pct(bucket: &UsageBucket) -> String {
    fmt_pct(
        bucket.requests_total.saturating_sub(bucket.requests_error),
        bucket.requests_total,
    )
}

fn usage_balance_status_label(status: UsageBalanceStatus, lang: Language) -> &'static str {
    match status {
        UsageBalanceStatus::Ok => i18n::label(lang, "ok"),
        UsageBalanceStatus::Unlimited => i18n::label(lang, "unlimited"),
        UsageBalanceStatus::Exhausted => i18n::label(lang, "exhausted"),
        UsageBalanceStatus::Stale => i18n::label(lang, "stale"),
        UsageBalanceStatus::Error => i18n::label(lang, "error"),
        UsageBalanceStatus::Unknown => i18n::label(lang, "unknown"),
    }
}

fn usage_balance_counts_line(counts: &UsageBalanceStatusCounts, lang: Language) -> String {
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

fn usage_refresh_line(view: &UsageBalanceView, lang: Language) -> String {
    if view.refresh_status.refreshing {
        return match lang {
            Language::Zh => "刷新中".to_string(),
            Language::En => "refreshing".to_string(),
        };
    }
    if let Some(err) = view.refresh_status.last_error.as_deref() {
        return format!("{}: {err}", i18n::label(lang, "error"));
    }
    if let Some(err) = view.refresh_status.latest_error.as_deref() {
        return format!("{}: {err}", i18n::label(lang, "latest error"));
    }
    if let Some(msg) = view.refresh_status.last_message.as_deref() {
        return msg.to_string();
    }
    format!(
        "{}={}",
        i18n::label(lang, "snapshots"),
        view.refresh_status.total_snapshots
    )
}

fn fmt_avg_ms(total_ms: u64, n: u64) -> String {
    if n == 0 {
        return "-".to_string();
    }
    duration_short(total_ms / n)
}

fn cost_confidence_label(confidence: CostConfidence, lang: Language) -> &'static str {
    match confidence {
        CostConfidence::Unknown => i18n::label(lang, "unknown"),
        CostConfidence::Partial => i18n::label(lang, "partial"),
        CostConfidence::Estimated => i18n::label(lang, "estimated"),
        CostConfidence::Exact => i18n::label(lang, "exact"),
    }
}

fn calc_output_rate_tok_s(bucket: &UsageBucket) -> Option<f64> {
    let output = bucket.usage.output_tokens.max(0) as f64;
    if output <= 0.0 || bucket.generation_ms_total == 0 {
        return None;
    }
    Some(output / (bucket.generation_ms_total as f64 / 1000.0))
}

fn fmt_tok_s_0(rate: Option<f64>) -> String {
    let Some(v) = rate.filter(|v| v.is_finite() && *v > 0.0) else {
        return "-".to_string();
    };
    format!("{:.0}", v)
}

fn fmt_avg_ttfb_ms(bucket: &UsageBucket) -> String {
    if bucket.ttfb_samples == 0 {
        return "-".to_string();
    }
    duration_short(bucket.ttfb_ms_total / bucket.ttfb_samples)
}

fn fmt_avg_generation_ms(bucket: &UsageBucket) -> String {
    if bucket.requests_with_usage == 0 || bucket.generation_ms_total == 0 {
        return "-".to_string();
    }
    duration_short(bucket.generation_ms_total / bucket.requests_with_usage)
}

fn cache_hit_rate(usage: &UsageMetrics) -> Option<f64> {
    usage.cache_hit_rate()
}

fn fmt_cache_hit(usage: &UsageMetrics) -> String {
    cache_hit_rate(usage)
        .map(|rate| format!("{:.1}%", rate * 100.0))
        .unwrap_or_else(|| "-".to_string())
}

fn cost_coverage_label(bucket: &UsageBucket, lang: Language) -> String {
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

fn day_to_ymd(day: i32) -> String {
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

fn day_range_label(first: Option<i32>, last: Option<i32>) -> String {
    match (first, last) {
        (Some(first), Some(last)) if first == last => day_to_ymd(first),
        (Some(first), Some(last)) => format!("{}..{}", day_to_ymd(first), day_to_ymd(last)),
        _ => "-".to_string(),
    }
}

fn stats_coverage_line(
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

fn live_health_line(stats: &WindowStats, lang: Language) -> String {
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

fn balance_status_rank(status: BalanceSnapshotStatus) -> u8 {
    match status {
        BalanceSnapshotStatus::Ok => 0,
        BalanceSnapshotStatus::Stale => 1,
        BalanceSnapshotStatus::Unknown | BalanceSnapshotStatus::Error => 2,
        BalanceSnapshotStatus::Exhausted => 3,
    }
}

fn provider_primary_balance<'a>(
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

fn provider_balance_brief(
    snapshot: &Snapshot,
    provider_id: &str,
    max_width: usize,
    lang: Language,
) -> String {
    provider_primary_balance(&snapshot.provider_balances, provider_id)
        .map(|balance| provider_balance_compact_lang(balance, max_width, lang))
        .unwrap_or_else(|| "-".to_string())
}

fn table_balance_brief(
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

pub(super) fn render_stats_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    _providers: &[ProviderOption],
    area: Rect,
) {
    let usage_balance = build_usage_balance_view(ui, snapshot);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(area);

    let lang = ui.language;
    let window_label = stats_window_label(ui.stats_days, lang);
    render_kpis(f, p, snapshot, &usage_balance, &window_label, rows[0], lang);
    render_sparkline(f, p, snapshot, &window_label, rows[1], lang);
    render_tables(
        f,
        p,
        ui,
        snapshot,
        &usage_balance,
        &window_label,
        rows[2],
        lang,
    );
}

fn render_kpis(
    f: &mut Frame<'_>,
    p: Palette,
    snapshot: &Snapshot,
    usage_balance: &UsageBalanceView,
    window_label: &str,
    area: Rect,
    lang: Language,
) {
    let l = |text| i18n::label(lang, text);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    let s = &snapshot.usage_rollup.window;
    let tokens = &s.usage;
    let ok = s.requests_total.saturating_sub(s.requests_error);

    let req_block = Block::default()
        .title(format!("{} ({window_label})", l("Requests")))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let req_text = Text::from(vec![
        Line::from(vec![
            Span::styled(format!("{} ", l("total")), Style::default().fg(p.muted)),
            Span::styled(s.requests_total.to_string(), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled(format!("{} ", l("ok")), Style::default().fg(p.muted)),
            Span::styled(ok.to_string(), Style::default().fg(p.good)),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("success")), Style::default().fg(p.muted)),
            Span::styled(fmt_success_pct(s), Style::default().fg(p.good)),
            Span::raw("  "),
            Span::styled(format!("{} ", l("err")), Style::default().fg(p.muted)),
            Span::styled(s.requests_error.to_string(), Style::default().fg(p.warn)),
        ]),
    ]);
    f.render_widget(Paragraph::new(req_text).block(req_block), cols[0]);

    let spend_block = Block::default()
        .title(l("Spend & tokens"))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let spend_text = Text::from(vec![
        Line::from(vec![
            Span::styled(format!("{} ", l("cost")), Style::default().fg(p.muted)),
            Span::styled(s.cost.display_total(), Style::default().fg(p.accent)),
            Span::raw("  "),
            Span::styled(format!("{} ", l("tok")), Style::default().fg(p.muted)),
            Span::styled(
                tokens_short(tokens.total_tokens),
                Style::default().fg(p.text),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("in/out")), Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "{}/{}",
                    tokens_short(tokens.input_tokens),
                    tokens_short(tokens.output_tokens)
                ),
                Style::default().fg(p.muted),
            ),
        ]),
    ]);
    f.render_widget(Paragraph::new(spend_text).block(spend_block), cols[1]);

    let perf_block = Block::default()
        .title(l("Cache & speed"))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let perf_text = Text::from(vec![
        Line::from(vec![
            Span::styled(format!("{} ", l("cache")), Style::default().fg(p.muted)),
            Span::styled(fmt_cache_hit(tokens), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled(format!("{} ", l("tok/s")), Style::default().fg(p.muted)),
            Span::styled(
                fmt_tok_s_0(calc_output_rate_tok_s(s)),
                Style::default().fg(p.text),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("ttfb")), Style::default().fg(p.muted)),
            Span::styled(fmt_avg_ttfb_ms(s), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled(format!("{} ", l("avg")), Style::default().fg(p.muted)),
            Span::styled(
                fmt_avg_ms(s.duration_ms_total, s.requests_total),
                Style::default().fg(p.text),
            ),
        ]),
    ]);
    f.render_widget(Paragraph::new(perf_text).block(perf_block), cols[2]);

    let live_block = Block::default()
        .title(l("Usage / Balance"))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let live_text = Text::from(vec![
        Line::from(vec![
            Span::styled("bal ", Style::default().fg(p.muted)),
            Span::styled(
                usage_balance_counts_line(&usage_balance.totals.balance_status_counts, lang),
                Style::default().fg(p.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("ref ", Style::default().fg(p.muted)),
            Span::styled(
                shorten(&usage_refresh_line(usage_balance, lang), 48),
                Style::default().fg(p.muted),
            ),
        ]),
        Line::from(vec![
            Span::styled("5m ", Style::default().fg(p.muted)),
            Span::styled(
                live_health_line(&snapshot.stats_5m, lang),
                Style::default().fg(p.muted),
            ),
        ]),
    ]);
    f.render_widget(
        Paragraph::new(live_text)
            .block(live_block)
            .wrap(Wrap { trim: true }),
        cols[3],
    );
}

fn render_sparkline(
    f: &mut Frame<'_>,
    p: Palette,
    snapshot: &Snapshot,
    window_label: &str,
    area: Rect,
    lang: Language,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let coverage = stats_coverage_line(p, snapshot, window_label, lang);
    f.render_widget(Paragraph::new(Text::from(coverage)), rows[0]);

    let values = snapshot
        .usage_rollup
        .by_day
        .iter()
        .map(|(_, b)| b.usage.total_tokens.max(0) as u64)
        .collect::<Vec<_>>();
    let block = Block::default()
        .title(i18n::label(lang, "Tokens / day (window, zero-filled)"))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));

    let widget = Sparkline::default()
        .block(block)
        .style(Style::default().fg(p.accent))
        .data(&values);
    f.render_widget(widget, rows[1]);
}

#[allow(clippy::too_many_arguments)]
fn render_tables(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    usage_balance: &UsageBalanceView,
    window_label: &str,
    area: Rect,
    lang: Language,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(cols[0]);

    render_bucket_table_stateful(
        f,
        p,
        ui.stats_focus == StatsFocus::Stations,
        &format!(
            "{} scorecard ({window_label})",
            i18n::label(lang, "Stations")
        ),
        &snapshot.usage_rollup.by_config,
        snapshot,
        StatsFocus::Stations,
        left[0],
        &mut ui.stats_stations_table,
        lang,
    );
    render_provider_usage_balance_table_stateful(
        f,
        p,
        ui.stats_focus == StatsFocus::Providers,
        &format!(
            "{} / {} ({window_label})",
            i18n::label(lang, "Provider"),
            i18n::label(lang, "Balance")
        ),
        &usage_balance.provider_rows,
        snapshot,
        left[1],
        &mut ui.stats_providers_table,
        lang,
    );

    render_detail_panel(
        f,
        p,
        ui,
        snapshot,
        usage_balance,
        window_label,
        cols[1],
        lang,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_bucket_table_stateful(
    f: &mut Frame<'_>,
    p: Palette,
    focused: bool,
    title: &str,
    items: &[(String, UsageBucket)],
    snapshot: &Snapshot,
    focus: StatsFocus,
    area: Rect,
    state: &mut ratatui::widgets::TableState,
    lang: Language,
) {
    let l = |text| i18n::label(lang, text);
    let header = Row::new(vec![
        Cell::from(Span::styled(l("name"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(
            l("balance/quota"),
            Style::default().fg(p.muted),
        )),
        Cell::from(Span::styled(l("req"), Style::default().fg(p.muted))),
        Cell::from(Span::styled("ok%", Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("ttfb"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("tok/s"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("tok"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("usd"), Style::default().fg(p.muted))),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = items
        .iter()
        .map(|(name, b)| {
            let cost = b
                .cost
                .total_cost_usd
                .clone()
                .unwrap_or_else(|| "-".to_string());
            let rate = fmt_tok_s_0(calc_output_rate_tok_s(b));
            Row::new(vec![
                Cell::from(shorten_middle(name, 24)),
                Cell::from(table_balance_brief(snapshot, focus, name, lang)),
                Cell::from(b.requests_total.to_string()),
                Cell::from(fmt_success_pct(b)),
                Cell::from(fmt_avg_ttfb_ms(b)),
                Cell::from(rate),
                Cell::from(tokens_short(b.usage.total_tokens)),
                Cell::from(cost),
            ])
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        rows,
        [
            Constraint::Min(14),
            Constraint::Length(STATS_BALANCE_COLUMN_WIDTH),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused { p.focus } else { p.border })),
    )
    .row_highlight_style(Style::default().bg(p.panel).fg(p.text))
    .highlight_symbol("  ");

    f.render_stateful_widget(table, area, state);
}

#[allow(clippy::too_many_arguments)]
fn render_provider_usage_balance_table_stateful(
    f: &mut Frame<'_>,
    p: Palette,
    focused: bool,
    title: &str,
    rows: &[UsageBalanceProviderRow],
    snapshot: &Snapshot,
    area: Rect,
    state: &mut ratatui::widgets::TableState,
    lang: Language,
) {
    let l = |text| i18n::label(lang, text);
    let header = Row::new(vec![
        Cell::from(Span::styled(l("provider"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("status"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(
            l("balance/quota"),
            Style::default().fg(p.muted),
        )),
        Cell::from(Span::styled(l("req"), Style::default().fg(p.muted))),
        Cell::from(Span::styled("ok%", Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("tok"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("usd"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("route"), Style::default().fg(p.muted))),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let table_rows = rows
        .iter()
        .map(|row| {
            Row::new(vec![
                Cell::from(shorten_middle(&row.provider_id, 22)),
                Cell::from(usage_balance_status_label(row.balance_status, lang)),
                Cell::from(provider_balance_brief(
                    snapshot,
                    &row.provider_id,
                    usize::from(STATS_BALANCE_COLUMN_WIDTH),
                    lang,
                )),
                Cell::from(row.usage.requests_total.to_string()),
                Cell::from(fmt_per_mille(row.success_per_mille)),
                Cell::from(tokens_short(row.usage.usage.total_tokens)),
                Cell::from(row.cost_display.clone()),
                Cell::from(shorten(&provider_route_brief(row, lang), 18)),
            ])
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        table_rows,
        [
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(STATS_BALANCE_COLUMN_WIDTH),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused { p.focus } else { p.border })),
    )
    .row_highlight_style(Style::default().bg(p.panel).fg(p.text))
    .highlight_symbol("  ");

    f.render_stateful_widget(table, area, state);
}

fn provider_route_brief(row: &UsageBalanceProviderRow, lang: Language) -> String {
    if row.routing.selected {
        return row
            .routing
            .selected_endpoint_id
            .as_deref()
            .map(|endpoint| format!("{} {endpoint}", i18n::label(lang, "selected")))
            .unwrap_or_else(|| i18n::label(lang, "selected").to_string());
    }
    if !row.routing.skip_reasons.is_empty() {
        return row.routing.skip_reasons.join(",");
    }
    if row.routing.candidate_count > 0 {
        return i18n::label(lang, "candidate").to_string();
    }
    "-".to_string()
}

#[allow(clippy::too_many_arguments)]
fn render_provider_usage_detail(
    f: &mut Frame<'_>,
    p: Palette,
    _ui: &UiState,
    usage_balance: &UsageBalanceView,
    row: Option<&UsageBalanceProviderRow>,
    window_label: &str,
    area: Rect,
    lang: Language,
) {
    let l = |text| i18n::label(lang, text);
    let block = Block::default()
        .title(Span::styled(
            format!(
                "{} / {}  {}: {window_label}",
                l("Usage"),
                l("Balance"),
                l("window")
            ),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));

    let Some(row) = row else {
        f.render_widget(
            Paragraph::new(Text::from(Line::from(Span::styled(
                l("No data in this window."),
                Style::default().fg(p.muted),
            ))))
            .block(block)
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    };

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(0)])
        .split(area);

    let balance_summary = row
        .primary_balance
        .as_ref()
        .map(|balance| balance.amount_summary.as_str())
        .unwrap_or("-");
    let route = provider_route_brief(row, lang);
    let latest_error = row.latest_balance_error.as_deref().unwrap_or("-");
    let lines = vec![
        Line::from(vec![
            Span::styled(format!("{}: ", l("provider")), Style::default().fg(p.muted)),
            Span::styled(row.provider_id.clone(), Style::default().fg(p.text)),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("requests")), Style::default().fg(p.muted)),
            Span::styled(
                row.usage.requests_total.to_string(),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled(format!("{} ", l("success")), Style::default().fg(p.muted)),
            Span::styled(
                fmt_per_mille(row.success_per_mille),
                Style::default().fg(p.good),
            ),
            Span::raw("  "),
            Span::styled(format!("{} ", l("errors")), Style::default().fg(p.muted)),
            Span::styled(
                row.usage.requests_error.to_string(),
                Style::default().fg(p.warn),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("tokens")), Style::default().fg(p.muted)),
            Span::styled(
                tokens_short(row.usage.usage.total_tokens),
                Style::default().fg(p.accent),
            ),
            Span::raw("  "),
            Span::styled(format!("{} ", l("cost")), Style::default().fg(p.muted)),
            Span::styled(row.cost_display.clone(), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled(format!("{} ", l("coverage")), Style::default().fg(p.muted)),
            Span::styled(
                cost_coverage_label(&row.usage, lang),
                Style::default().fg(p.muted),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("balance")), Style::default().fg(p.muted)),
            Span::styled(
                usage_balance_status_label(row.balance_status, lang),
                Style::default().fg(if row.balance_status.is_attention() {
                    p.warn
                } else {
                    p.good
                }),
            ),
            Span::raw("  "),
            Span::styled(shorten(balance_summary, 72), Style::default().fg(p.text)),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("counts")), Style::default().fg(p.muted)),
            Span::styled(
                usage_balance_counts_line(&row.balance_counts, lang),
                Style::default().fg(p.muted),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("route")), Style::default().fg(p.muted)),
            Span::styled(shorten(&route, 72), Style::default().fg(p.text)),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{} ", l("latest error")),
                Style::default().fg(p.muted),
            ),
            Span::styled(shorten(latest_error, 72), Style::default().fg(p.warn)),
        ]),
    ];

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: true }),
        inner[0],
    );

    render_endpoint_rows(
        f,
        p,
        usage_balance
            .endpoint_rows
            .iter()
            .filter(|endpoint| endpoint.provider_id == row.provider_id)
            .collect::<Vec<_>>(),
        inner[1],
        lang,
    );
}

fn render_endpoint_rows(
    f: &mut Frame<'_>,
    p: Palette,
    endpoints: Vec<&UsageBalanceEndpointRow>,
    area: Rect,
    lang: Language,
) {
    let l = |text| i18n::label(lang, text);
    let header = Row::new(vec![
        Cell::from(Span::styled(l("endpoint"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("balance"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("req"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("err"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("tok"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("route"), Style::default().fg(p.muted))),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = endpoints
        .into_iter()
        .map(|endpoint| {
            let endpoint_label = endpoint
                .base_url
                .as_deref()
                .unwrap_or(endpoint.endpoint_id.as_str());
            let balance = endpoint
                .balance
                .as_ref()
                .map(|balance| shorten(&balance.amount_summary, 28))
                .unwrap_or_else(|| {
                    usage_balance_status_label(endpoint.balance_status, lang).to_string()
                });
            let route = if endpoint.route_selected {
                i18n::label(lang, "selected").to_string()
            } else if endpoint.route_skip_reasons.is_empty() {
                "-".to_string()
            } else {
                endpoint.route_skip_reasons.join(",")
            };
            Row::new(vec![
                Cell::from(shorten_middle(endpoint_label, 30)),
                Cell::from(balance),
                Cell::from(endpoint.usage.requests_total.to_string()),
                Cell::from(endpoint.usage.requests_error.to_string()),
                Cell::from(tokens_short(endpoint.usage.usage.total_tokens)),
                Cell::from(shorten(&route, 24)),
            ])
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        rows,
        [
            Constraint::Min(16),
            Constraint::Length(28),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(l("Endpoints / recent sample"))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.border)),
    );
    f.render_widget(table, area);
}

#[allow(clippy::too_many_arguments)]
fn render_detail_panel(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    usage_balance: &UsageBalanceView,
    window_label: &str,
    area: Rect,
    lang: Language,
) {
    let l = |text| i18n::label(lang, text);
    if ui.stats_focus == StatsFocus::Providers {
        render_provider_usage_detail(
            f,
            p,
            ui,
            usage_balance,
            usage_balance
                .provider_rows
                .get(ui.selected_stats_provider_idx),
            window_label,
            area,
            lang,
        );
        return;
    }

    let selected = match ui.stats_focus {
        StatsFocus::Stations => snapshot
            .usage_rollup
            .by_config
            .get(ui.selected_stats_station_idx)
            .map(|(k, v)| ("station", k.as_str(), v)),
        StatsFocus::Providers => None,
    };

    let block = Block::default()
        .title(Span::styled(
            format!("{}  {}: {window_label}", l("Details"), l("window")),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));

    let Some((kind, name, bucket)) = selected else {
        f.render_widget(
            Paragraph::new(Text::from(Line::from(Span::styled(
                l("No data in this window."),
                Style::default().fg(p.muted),
            ))))
            .block(block)
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    };

    let series = match kind {
        "station" => snapshot
            .usage_rollup
            .by_config_day
            .get(name)
            .map(|v| {
                v.iter()
                    .map(|(_, b)| b.usage.total_tokens.max(0) as u64)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        _ => snapshot
            .usage_rollup
            .by_provider_day
            .get(name)
            .map(|v| {
                v.iter()
                    .map(|(_, b)| b.usage.total_tokens.max(0) as u64)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    };

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(5),
            Constraint::Min(0),
        ])
        .split(area);

    let cost = bucket.cost.display_total();
    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{}: ", i18n::label(lang, kind)),
                Style::default().fg(p.muted),
            ),
            Span::styled(name.to_string(), Style::default().fg(p.text)),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("requests")), Style::default().fg(p.muted)),
            Span::styled(
                bucket.requests_total.to_string(),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled(format!("{} ", l("success")), Style::default().fg(p.muted)),
            Span::styled(fmt_success_pct(bucket), Style::default().fg(p.good)),
            Span::raw("  "),
            Span::styled(format!("{} ", l("errors")), Style::default().fg(p.muted)),
            Span::styled(
                bucket.requests_error.to_string(),
                Style::default().fg(p.warn),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("tokens")), Style::default().fg(p.muted)),
            Span::styled(
                tokens_short(bucket.usage.total_tokens),
                Style::default().fg(p.accent),
            ),
            Span::raw("  "),
            Span::styled(format!("{} ", l("cost")), Style::default().fg(p.muted)),
            Span::styled(cost, Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled(format!("{} ", l("coverage")), Style::default().fg(p.muted)),
            Span::styled(
                cost_coverage_label(bucket, lang),
                Style::default().fg(p.muted),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("ttfb")), Style::default().fg(p.muted)),
            Span::styled(fmt_avg_ttfb_ms(bucket), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled(format!("{} ", l("avg")), Style::default().fg(p.muted)),
            Span::styled(
                fmt_avg_ms(bucket.duration_ms_total, bucket.requests_total),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} ", l("generation")),
                Style::default().fg(p.muted),
            ),
            Span::styled(fmt_avg_generation_ms(bucket), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled(format!("{} ", l("tok/s")), Style::default().fg(p.muted)),
            Span::styled(
                fmt_tok_s_0(calc_output_rate_tok_s(bucket)),
                Style::default().fg(p.text),
            ),
        ]),
        Line::from(vec![
            Span::styled(format!("{} ", l("cache hit")), Style::default().fg(p.muted)),
            Span::styled(fmt_cache_hit(&bucket.usage), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled(
                format!("{} ", l("read/create")),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                format!(
                    "{}/{}",
                    tokens_short(bucket.usage.cache_read_tokens_total()),
                    tokens_short(bucket.usage.cache_creation_tokens_total()),
                ),
                Style::default().fg(p.muted),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{} ", l("tok in/out/rsn")),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                format!(
                    "{}/{}/{}",
                    tokens_short(bucket.usage.input_tokens),
                    tokens_short(bucket.usage.output_tokens),
                    tokens_short(bucket.usage.reasoning_output_tokens_total()),
                ),
                Style::default().fg(p.muted),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{} ", l("loaded total req")),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                snapshot.usage_rollup.loaded.requests_total.to_string(),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled(
                if snapshot.usage_rollup.coverage.window_exceeds_loaded_start {
                    match lang {
                        Language::Zh => "所选窗口只加载了部分覆盖数据",
                        Language::En => "selected window has partial loaded coverage",
                    }
                } else {
                    ""
                },
                Style::default().fg(p.warn),
            ),
        ]),
    ];

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: true }),
        inner[0],
    );

    let sl_block = Block::default()
        .title(Span::styled(
            l("Tokens / day"),
            Style::default().fg(p.muted).add_modifier(Modifier::DIM),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let sl = Sparkline::default()
        .block(sl_block)
        .style(Style::default().fg(p.accent))
        .data(&series);
    f.render_widget(sl, inner[1]);

    render_recent_breakdown(f, p, ui, snapshot, kind, name, inner[2]);
}

fn render_recent_breakdown(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    kind: &str,
    name: &str,
    area: Rect,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let tips = Text::from(vec![
        Line::from(vec![
            Span::styled("Tab", Style::default().fg(p.text)),
            Span::styled(format!(" {}  ", l("focus")), Style::default().fg(p.muted)),
            Span::styled("d", Style::default().fg(p.text)),
            Span::styled(format!(" {}  ", l("window")), Style::default().fg(p.muted)),
            Span::styled("e", Style::default().fg(p.text)),
            Span::styled(
                format!(" {}(recent)", l("errors_only")),
                Style::default().fg(p.muted),
            ),
        ]),
        Line::from(vec![
            Span::styled("↑/↓", Style::default().fg(p.text)),
            Span::styled(
                match lang {
                    Language::Zh => " 选择  ",
                    Language::En => " select  ",
                },
                Style::default().fg(p.muted),
            ),
            Span::styled("y", Style::default().fg(p.text)),
            Span::styled(
                match lang {
                    Language::Zh => " 导出报告",
                    Language::En => " export report",
                },
                Style::default().fg(p.muted),
            ),
        ]),
    ]);

    let errors_only = ui.stats_errors_only;
    let mut recent_total = 0u64;
    let mut recent_err = 0u64;
    let mut class_2xx = 0u64;
    let mut class_3xx = 0u64;
    let mut class_4xx = 0u64;
    let mut class_5xx = 0u64;
    let mut by_model: HashMap<String, (u64, i64)> = HashMap::new();
    let mut by_status: HashMap<u16, u64> = HashMap::new();

    for r in &snapshot.recent {
        let matches = match kind {
            "station" => r.station_name.as_deref() == Some(name),
            _ => r.provider_id.as_deref() == Some(name),
        };
        if !matches {
            continue;
        }
        if errors_only && r.status_code < 400 {
            continue;
        }
        recent_total += 1;
        if r.status_code >= 400 {
            recent_err += 1;
        }
        match r.status_code {
            200..=299 => class_2xx += 1,
            300..=399 => class_3xx += 1,
            400..=499 => class_4xx += 1,
            500..=599 => class_5xx += 1,
            _ => {}
        }
        *by_status.entry(r.status_code).or_insert(0) += 1;
        let model = r.model.as_deref().unwrap_or("-");
        let tokens = r.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0);
        by_model
            .entry(model.to_string())
            .and_modify(|(c, t)| {
                *c = c.saturating_add(1);
                *t = t.saturating_add(tokens);
            })
            .or_insert((1, tokens));
    }

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} ", l("Recent sample")),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if errors_only {
                match lang {
                    Language::Zh => "（仅错误）",
                    Language::En => "(errors only)",
                }
            } else {
                match lang {
                    Language::Zh => "（全部）",
                    Language::En => "(all)",
                }
            },
            Style::default().fg(p.muted).add_modifier(Modifier::DIM),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", l("req")), Style::default().fg(p.muted)),
        Span::styled(recent_total.to_string(), Style::default().fg(p.text)),
        Span::raw("  "),
        Span::styled(format!("{} ", l("err")), Style::default().fg(p.muted)),
        Span::styled(recent_err.to_string(), Style::default().fg(p.warn)),
        Span::raw("  "),
        Span::styled("2xx/3xx/4xx/5xx ", Style::default().fg(p.muted)),
        Span::styled(
            format!("{class_2xx}/{class_3xx}/{class_4xx}/{class_5xx}"),
            Style::default().fg(p.muted),
        ),
    ]));

    let mut status_items = by_status.into_iter().collect::<Vec<_>>();
    status_items.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    let top_status = status_items
        .into_iter()
        .take(6)
        .map(|(s, c)| format!("{s}:{c}"))
        .collect::<Vec<_>>()
        .join("  ");
    if !top_status.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", l("status")), Style::default().fg(p.muted)),
            Span::styled(shorten(&top_status, 56), Style::default().fg(p.muted)),
        ]));
    }

    let mut models = by_model.into_iter().collect::<Vec<_>>();
    models.sort_by_key(|(_, (_, tok))| std::cmp::Reverse(*tok));
    let top_models = models
        .into_iter()
        .take(5)
        .map(|(m, (c, tok))| format!("{}({} / {})", shorten(&m, 18), c, tokens_short(tok)))
        .collect::<Vec<_>>()
        .join("  ");
    if !top_models.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", l("models")), Style::default().fg(p.muted)),
            Span::styled(shorten(&top_models, 56), Style::default().fg(p.muted)),
        ]));
    }

    lines.push(Line::from(""));
    for l in tips.lines {
        lines.push(l);
    }

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .title(Span::styled(
                        format!(
                            "{} (loaded <= {}) + Tips",
                            l("Recent sample"),
                            snapshot.recent.len()
                        ),
                        Style::default().fg(p.muted).add_modifier(Modifier::DIM),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(p.border)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}
