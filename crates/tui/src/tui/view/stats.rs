use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Sparkline, Table, Wrap};

use crate::dashboard_core::WindowStats;
use crate::pricing::CostConfidence;
use crate::state::{BalanceSnapshotStatus, ProviderBalanceSnapshot, UsageBucket};
use crate::tui::ProviderOption;
use crate::tui::model::{
    Palette, Snapshot, duration_short, provider_balance_compact, shorten, shorten_middle,
    station_balance_brief, tokens_short,
};
use crate::tui::state::UiState;
use crate::tui::types::StatsFocus;
use crate::usage::UsageMetrics;

fn stats_window_label(days: usize) -> String {
    match days {
        0 => "loaded".to_string(),
        1 => "today".to_string(),
        n => format!("{n}d"),
    }
}

fn fmt_pct(num: u64, den: u64) -> String {
    if den == 0 {
        return "-".to_string();
    }
    format!("{:.1}%", (num as f64) * 100.0 / (den as f64))
}

fn fmt_success_pct(bucket: &UsageBucket) -> String {
    fmt_pct(
        bucket.requests_total.saturating_sub(bucket.requests_error),
        bucket.requests_total,
    )
}

fn fmt_avg_ms(total_ms: u64, n: u64) -> String {
    if n == 0 {
        return "-".to_string();
    }
    duration_short(total_ms / n)
}

fn cost_confidence_label(confidence: CostConfidence) -> &'static str {
    match confidence {
        CostConfidence::Unknown => "unknown",
        CostConfidence::Partial => "partial",
        CostConfidence::Estimated => "estimated",
        CostConfidence::Exact => "exact",
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

fn cache_creation_tokens(usage: &UsageMetrics) -> i64 {
    usage.cache_creation_tokens_total().max(0)
}

fn cache_hit_rate(usage: &UsageMetrics) -> Option<f64> {
    let read = usage.cache_read_input_tokens.max(0);
    let create = cache_creation_tokens(usage);
    let effective_input = usage.input_tokens.max(0).saturating_sub(read);
    let denom = effective_input.saturating_add(create).saturating_add(read);
    if denom <= 0 {
        return None;
    }
    Some(read as f64 / denom as f64)
}

fn fmt_cache_hit(usage: &UsageMetrics) -> String {
    cache_hit_rate(usage)
        .map(|rate| format!("{:.1}%", rate * 100.0))
        .unwrap_or_else(|| "-".to_string())
}

fn cost_coverage_label(bucket: &UsageBucket) -> String {
    if bucket.requests_with_usage == 0 {
        return "-".to_string();
    }
    format!(
        "{}/{} {}",
        bucket.cost.priced_requests,
        bucket.requests_with_usage,
        cost_confidence_label(bucket.cost.confidence)
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

fn stats_coverage_line(p: Palette, snapshot: &Snapshot, window_label: &str) -> Line<'static> {
    let c = &snapshot.usage_rollup.coverage;
    let loaded = day_range_label(c.loaded_first_day, c.loaded_last_day);
    let window = day_range_label(c.window_first_day, c.window_last_day);
    let warning = if c.window_exceeds_loaded_start {
        "  partial: selected window starts before loaded log data"
    } else {
        ""
    };

    Line::from(vec![
        Span::styled(
            format!("window {window_label} {window}"),
            Style::default().fg(p.text),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "loaded {loaded} days={} req={}",
                c.loaded_days_with_data, c.loaded_requests
            ),
            Style::default().fg(p.muted),
        ),
        Span::styled(warning.to_string(), Style::default().fg(p.warn)),
    ])
}

fn live_health_line(stats: &WindowStats) -> String {
    if stats.total == 0 {
        return "-".to_string();
    }
    format!(
        "ok {} p95 {} retry {} 429 {} 5xx {} n={}",
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
        .values()
        .flat_map(|balances| balances.iter())
        .filter(|snapshot| snapshot.provider_id == provider_id)
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        balance_status_rank(left.status)
            .cmp(&balance_status_rank(right.status))
            .then_with(|| left.upstream_index.cmp(&right.upstream_index))
            .then_with(|| right.fetched_at_ms.cmp(&left.fetched_at_ms))
    });
    matches.into_iter().next()
}

fn provider_balance_brief(snapshot: &Snapshot, provider_id: &str, max_width: usize) -> String {
    provider_primary_balance(&snapshot.provider_balances, provider_id)
        .map(|balance| provider_balance_compact(balance, max_width))
        .unwrap_or_else(|| "-".to_string())
}

fn table_balance_brief(snapshot: &Snapshot, focus: StatsFocus, name: &str) -> String {
    match focus {
        StatsFocus::Stations => station_balance_brief(&snapshot.provider_balances, name, 18),
        StatsFocus::Providers => provider_balance_brief(snapshot, name, 18),
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
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(area);

    let window_label = stats_window_label(ui.stats_days);
    render_kpis(f, p, snapshot, &window_label, rows[0]);
    render_sparkline(f, p, snapshot, &window_label, rows[1]);
    render_tables(f, p, ui, snapshot, &window_label, rows[2]);
}

fn render_kpis(f: &mut Frame<'_>, p: Palette, snapshot: &Snapshot, window_label: &str, area: Rect) {
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
        .title(format!("Requests ({window_label})"))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let req_text = Text::from(vec![
        Line::from(vec![
            Span::styled("total ", Style::default().fg(p.muted)),
            Span::styled(s.requests_total.to_string(), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled("ok ", Style::default().fg(p.muted)),
            Span::styled(ok.to_string(), Style::default().fg(p.good)),
        ]),
        Line::from(vec![
            Span::styled("success ", Style::default().fg(p.muted)),
            Span::styled(fmt_success_pct(s), Style::default().fg(p.good)),
            Span::raw("  "),
            Span::styled("err ", Style::default().fg(p.muted)),
            Span::styled(s.requests_error.to_string(), Style::default().fg(p.warn)),
        ]),
    ]);
    f.render_widget(Paragraph::new(req_text).block(req_block), cols[0]);

    let spend_block = Block::default()
        .title("Spend & tokens")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let spend_text = Text::from(vec![
        Line::from(vec![
            Span::styled("cost ", Style::default().fg(p.muted)),
            Span::styled(s.cost.display_total(), Style::default().fg(p.accent)),
            Span::raw("  "),
            Span::styled("tok ", Style::default().fg(p.muted)),
            Span::styled(
                tokens_short(tokens.total_tokens),
                Style::default().fg(p.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("in/out ", Style::default().fg(p.muted)),
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
        .title("Cache & speed")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let perf_text = Text::from(vec![
        Line::from(vec![
            Span::styled("cache ", Style::default().fg(p.muted)),
            Span::styled(fmt_cache_hit(tokens), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled("tok/s ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_tok_s_0(calc_output_rate_tok_s(s)),
                Style::default().fg(p.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("ttfb ", Style::default().fg(p.muted)),
            Span::styled(fmt_avg_ttfb_ms(s), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled("avg ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_avg_ms(s.duration_ms_total, s.requests_total),
                Style::default().fg(p.text),
            ),
        ]),
    ]);
    f.render_widget(Paragraph::new(perf_text).block(perf_block), cols[2]);

    let live_block = Block::default()
        .title("Live health")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let live_text = Text::from(vec![
        Line::from(vec![
            Span::styled("5m ", Style::default().fg(p.muted)),
            Span::styled(
                live_health_line(&snapshot.stats_5m),
                Style::default().fg(p.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("1h ", Style::default().fg(p.muted)),
            Span::styled(
                live_health_line(&snapshot.stats_1h),
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
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let coverage = stats_coverage_line(p, snapshot, window_label);
    f.render_widget(Paragraph::new(Text::from(coverage)), rows[0]);

    let values = snapshot
        .usage_rollup
        .by_day
        .iter()
        .map(|(_, b)| b.usage.total_tokens.max(0) as u64)
        .collect::<Vec<_>>();
    let block = Block::default()
        .title("Tokens / day (window, zero-filled)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));

    let widget = Sparkline::default()
        .block(block)
        .style(Style::default().fg(p.accent))
        .data(&values);
    f.render_widget(widget, rows[1]);
}

fn render_tables(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    window_label: &str,
    area: Rect,
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
        &format!("Stations scorecard ({window_label})"),
        &snapshot.usage_rollup.by_config,
        snapshot,
        StatsFocus::Stations,
        left[0],
        &mut ui.stats_stations_table,
    );
    render_bucket_table_stateful(
        f,
        p,
        ui.stats_focus == StatsFocus::Providers,
        &format!("Providers scorecard ({window_label})"),
        &snapshot.usage_rollup.by_provider,
        snapshot,
        StatsFocus::Providers,
        left[1],
        &mut ui.stats_providers_table,
    );

    render_detail_panel(f, p, ui, snapshot, window_label, cols[1]);
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
) {
    let header = Row::new(vec![
        Cell::from(Span::styled("name", Style::default().fg(p.muted))),
        Cell::from(Span::styled("balance/quota", Style::default().fg(p.muted))),
        Cell::from(Span::styled("req", Style::default().fg(p.muted))),
        Cell::from(Span::styled("ok%", Style::default().fg(p.muted))),
        Cell::from(Span::styled("ttfb", Style::default().fg(p.muted))),
        Cell::from(Span::styled("tok/s", Style::default().fg(p.muted))),
        Cell::from(Span::styled("tok", Style::default().fg(p.muted))),
        Cell::from(Span::styled("usd", Style::default().fg(p.muted))),
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
                Cell::from(table_balance_brief(snapshot, focus, name)),
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
            Constraint::Length(14),
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

fn render_detail_panel(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    window_label: &str,
    area: Rect,
) {
    let selected = match ui.stats_focus {
        StatsFocus::Stations => snapshot
            .usage_rollup
            .by_config
            .get(ui.selected_stats_station_idx)
            .map(|(k, v)| ("station", k.as_str(), v)),
        StatsFocus::Providers => snapshot
            .usage_rollup
            .by_provider
            .get(ui.selected_stats_provider_idx)
            .map(|(k, v)| ("provider", k.as_str(), v)),
    };

    let block = Block::default()
        .title(Span::styled(
            format!("Detail  window: {window_label}"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));

    let Some((kind, name, bucket)) = selected else {
        f.render_widget(
            Paragraph::new(Text::from(Line::from(Span::styled(
                "No data in this window.",
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
            Span::styled(format!("{kind}: "), Style::default().fg(p.muted)),
            Span::styled(name.to_string(), Style::default().fg(p.text)),
        ]),
        Line::from(vec![
            Span::styled("requests ", Style::default().fg(p.muted)),
            Span::styled(
                bucket.requests_total.to_string(),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled("success ", Style::default().fg(p.muted)),
            Span::styled(fmt_success_pct(bucket), Style::default().fg(p.good)),
            Span::raw("  "),
            Span::styled("errors ", Style::default().fg(p.muted)),
            Span::styled(
                bucket.requests_error.to_string(),
                Style::default().fg(p.warn),
            ),
        ]),
        Line::from(vec![
            Span::styled("tokens ", Style::default().fg(p.muted)),
            Span::styled(
                tokens_short(bucket.usage.total_tokens),
                Style::default().fg(p.accent),
            ),
            Span::raw("  "),
            Span::styled("cost ", Style::default().fg(p.muted)),
            Span::styled(cost, Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled("coverage ", Style::default().fg(p.muted)),
            Span::styled(cost_coverage_label(bucket), Style::default().fg(p.muted)),
        ]),
        Line::from(vec![
            Span::styled("ttfb ", Style::default().fg(p.muted)),
            Span::styled(fmt_avg_ttfb_ms(bucket), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled("avg ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_avg_ms(bucket.duration_ms_total, bucket.requests_total),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled("gen ", Style::default().fg(p.muted)),
            Span::styled(fmt_avg_generation_ms(bucket), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled("tok/s ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_tok_s_0(calc_output_rate_tok_s(bucket)),
                Style::default().fg(p.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("cache hit ", Style::default().fg(p.muted)),
            Span::styled(fmt_cache_hit(&bucket.usage), Style::default().fg(p.text)),
            Span::raw("  "),
            Span::styled("read/create ", Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "{}/{}",
                    tokens_short(bucket.usage.cache_read_input_tokens),
                    tokens_short(bucket.usage.cache_creation_tokens_total()),
                ),
                Style::default().fg(p.muted),
            ),
        ]),
        Line::from(vec![
            Span::styled("tok in/out/rsn ", Style::default().fg(p.muted)),
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
            Span::styled("loaded total req ", Style::default().fg(p.muted)),
            Span::styled(
                snapshot.usage_rollup.loaded.requests_total.to_string(),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled(
                if snapshot.usage_rollup.coverage.window_exceeds_loaded_start {
                    "selected window has partial loaded coverage"
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
            "Tokens / day",
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
    let tips = Text::from(vec![
        Line::from(vec![
            Span::styled("Tab", Style::default().fg(p.text)),
            Span::styled(" focus  ", Style::default().fg(p.muted)),
            Span::styled("d", Style::default().fg(p.text)),
            Span::styled(" window  ", Style::default().fg(p.muted)),
            Span::styled("e", Style::default().fg(p.text)),
            Span::styled(" errors_only(recent)", Style::default().fg(p.muted)),
        ]),
        Line::from(vec![
            Span::styled("↑/↓", Style::default().fg(p.text)),
            Span::styled(" select  ", Style::default().fg(p.muted)),
            Span::styled("y", Style::default().fg(p.text)),
            Span::styled(" export report", Style::default().fg(p.muted)),
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
            "Recent sample ",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if errors_only {
                "(errors only)"
            } else {
                "(all)"
            },
            Style::default().fg(p.muted).add_modifier(Modifier::DIM),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("req ", Style::default().fg(p.muted)),
        Span::styled(recent_total.to_string(), Style::default().fg(p.text)),
        Span::raw("  "),
        Span::styled("err ", Style::default().fg(p.muted)),
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
            Span::styled("status ", Style::default().fg(p.muted)),
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
            Span::styled("models ", Style::default().fg(p.muted)),
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
                        format!("Recent sample (loaded <= {}) + Tips", snapshot.recent.len()),
                        Style::default().fg(p.muted).add_modifier(Modifier::DIM),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(p.border)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}
