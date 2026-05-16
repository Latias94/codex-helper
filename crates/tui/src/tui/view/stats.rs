use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Sparkline, Table, Wrap};

use crate::state::UsageBucket;
use crate::tui::Language;
use crate::tui::ProviderOption;
use crate::tui::i18n;
use crate::tui::model::{Palette, Snapshot, shorten, shorten_middle, tokens_short};
use crate::tui::state::UiState;
use crate::tui::types::StatsFocus;
use crate::usage_balance::{UsageBalanceEndpointRow, UsageBalanceProviderRow, UsageBalanceView};

mod summary;
use summary::*;

pub(super) fn render_stats_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    _providers: &[ProviderOption],
    area: Rect,
) {
    let usage_balance = ui.usage_balance_view_for_selection(snapshot);
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

    let provider_rows = ui.filtered_usage_balance_provider_rows(usage_balance);
    let provider_title = format!(
        "{} / {} ({window_label}){}",
        i18n::label(lang, "Provider"),
        i18n::label(lang, "Balance"),
        filter_suffix(ui.stats_attention_only, lang)
    );
    let station_title = format!(
        "{} scorecard ({window_label})",
        i18n::label(lang, "Stations")
    );
    let providers_focused = ui.stats_focus == StatsFocus::Providers;
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if providers_focused {
            [Constraint::Percentage(64), Constraint::Percentage(36)]
        } else {
            [Constraint::Percentage(50), Constraint::Percentage(50)]
        })
        .split(cols[0]);

    if providers_focused {
        render_provider_usage_balance_table_stateful(
            f,
            p,
            true,
            &provider_title,
            &provider_rows,
            snapshot,
            left[0],
            &mut ui.stats_providers_table,
            lang,
        );
        render_bucket_table_stateful(
            f,
            p,
            false,
            &station_title,
            &snapshot.usage_rollup.by_config,
            snapshot,
            StatsFocus::Stations,
            left[1],
            &mut ui.stats_stations_table,
            lang,
        );
    } else {
        render_bucket_table_stateful(
            f,
            p,
            true,
            &station_title,
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
            false,
            &provider_title,
            &provider_rows,
            snapshot,
            left[1],
            &mut ui.stats_providers_table,
            lang,
        );
    }

    render_detail_panel(
        f,
        p,
        ui,
        snapshot,
        usage_balance,
        &provider_rows,
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
    rows: &[&UsageBalanceProviderRow],
    snapshot: &Snapshot,
    area: Rect,
    state: &mut ratatui::widgets::TableState,
    lang: Language,
) {
    let l = |text| i18n::label(lang, text);
    let compact = area.width < 72;
    let header_cells = if compact {
        vec![
            Cell::from(Span::styled(l("provider"), Style::default().fg(p.muted))),
            Cell::from(Span::styled(l("status"), Style::default().fg(p.muted))),
            Cell::from(Span::styled(
                l("balance/quota"),
                Style::default().fg(p.muted),
            )),
            Cell::from(Span::styled(l("route"), Style::default().fg(p.muted))),
        ]
    } else {
        vec![
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
        ]
    };
    let header = Row::new(header_cells).style(Style::default().add_modifier(Modifier::BOLD));

    let table_rows = rows
        .iter()
        .map(|row| {
            let balance = provider_balance_brief(
                snapshot,
                &row.provider_id,
                usize::from(STATS_BALANCE_COLUMN_WIDTH),
                lang,
            );
            let status_style = provider_balance_status_style(p, row);
            let route = shorten(
                &provider_route_brief(row, lang),
                if compact { 10 } else { 18 },
            );
            let cells = if compact {
                vec![
                    Cell::from(shorten_middle(&row.provider_id, 18)),
                    Cell::from(Span::styled(
                        provider_usage_balance_status_label(row, true, lang),
                        status_style,
                    )),
                    Cell::from(Span::styled(balance, status_style)),
                    Cell::from(Span::styled(route, Style::default().fg(p.muted))),
                ]
            } else {
                vec![
                    Cell::from(shorten_middle(&row.provider_id, 22)),
                    Cell::from(Span::styled(
                        provider_usage_balance_status_label(row, false, lang),
                        status_style,
                    )),
                    Cell::from(Span::styled(balance, status_style)),
                    Cell::from(row.usage.requests_total.to_string()),
                    Cell::from(fmt_per_mille(row.success_per_mille)),
                    Cell::from(tokens_short(row.usage.usage.total_tokens)),
                    Cell::from(row.cost_display.clone()),
                    Cell::from(Span::styled(route, Style::default().fg(p.muted))),
                ]
            };
            Row::new(cells)
        })
        .collect::<Vec<_>>();
    let widths = if compact {
        vec![
            Constraint::Min(10),
            Constraint::Length(8),
            Constraint::Length(STATS_BALANCE_COLUMN_WIDTH),
            Constraint::Length(8),
        ]
    } else {
        vec![
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(STATS_BALANCE_COLUMN_WIDTH),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(10),
        ]
    };

    let table = Table::new(table_rows, widths)
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
    ui: &mut UiState,
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
    let status_style = provider_balance_status_style(p, row);
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
                provider_usage_balance_status_label(row, false, lang),
                status_style,
            ),
            Span::raw("  "),
            Span::styled(balance_summary.to_string(), status_style),
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
            Span::styled(latest_error.to_string(), Style::default().fg(p.warn)),
        ]),
    ];

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: true }),
        inner[0],
    );

    let endpoints = ui.selected_usage_balance_provider_endpoints(usage_balance);
    let visible_rows = endpoint_visible_rows(inner[1]);
    let max_scroll = endpoints.len().saturating_sub(visible_rows) as u16;
    ui.stats_provider_detail_scroll = ui.stats_provider_detail_scroll.min(max_scroll);
    render_endpoint_rows(
        f,
        p,
        &endpoints,
        ui.stats_provider_detail_scroll,
        visible_rows,
        inner[1],
        lang,
    );
}

fn render_endpoint_rows(
    f: &mut Frame<'_>,
    p: Palette,
    endpoints: &[&UsageBalanceEndpointRow],
    scroll: u16,
    visible_rows: usize,
    area: Rect,
    lang: Language,
) {
    let l = |text| i18n::label(lang, text);
    let compact = area.width < 70;
    let header_cells = if compact {
        vec![
            Cell::from(Span::styled(l("endpoint"), Style::default().fg(p.muted))),
            Cell::from(Span::styled(l("balance"), Style::default().fg(p.muted))),
            Cell::from(Span::styled(l("req"), Style::default().fg(p.muted))),
        ]
    } else {
        vec![
            Cell::from(Span::styled(l("endpoint"), Style::default().fg(p.muted))),
            Cell::from(Span::styled(l("balance"), Style::default().fg(p.muted))),
            Cell::from(Span::styled(l("req"), Style::default().fg(p.muted))),
            Cell::from(Span::styled(l("err"), Style::default().fg(p.muted))),
            Cell::from(Span::styled(l("tok"), Style::default().fg(p.muted))),
            Cell::from(Span::styled(l("route"), Style::default().fg(p.muted))),
        ]
    };
    let header = Row::new(header_cells).style(Style::default().add_modifier(Modifier::BOLD));

    let scroll = usize::from(scroll).min(endpoints.len().saturating_sub(visible_rows));
    let balance_width = if compact {
        STATS_BALANCE_COLUMN_WIDTH
    } else {
        STATS_ENDPOINT_BALANCE_COLUMN_WIDTH
    };
    let rows = endpoints
        .iter()
        .skip(scroll)
        .take(visible_rows)
        .map(|endpoint| {
            let endpoint = *endpoint;
            let endpoint_label = endpoint
                .base_url
                .as_deref()
                .unwrap_or(endpoint.endpoint_id.as_str());
            let balance = endpoint
                .balance
                .as_ref()
                .map(|balance| {
                    atomic_summary_or_status(
                        &balance.amount_summary,
                        endpoint.balance_status,
                        usize::from(balance_width),
                        lang,
                    )
                })
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
            let balance_style = endpoint_balance_status_style(p, endpoint);
            let cells = if compact {
                vec![
                    Cell::from(shorten_middle(endpoint_label, 14)),
                    Cell::from(Span::styled(balance, balance_style)),
                    Cell::from(endpoint.usage.requests_total.to_string()),
                ]
            } else {
                vec![
                    Cell::from(shorten_middle(endpoint_label, 30)),
                    Cell::from(Span::styled(balance, balance_style)),
                    Cell::from(endpoint.usage.requests_total.to_string()),
                    Cell::from(endpoint.usage.requests_error.to_string()),
                    Cell::from(tokens_short(endpoint.usage.usage.total_tokens)),
                    Cell::from(shorten(&route, 24)),
                ]
            };
            Row::new(cells)
        })
        .collect::<Vec<_>>();
    let widths = if compact {
        vec![
            Constraint::Min(10),
            Constraint::Length(STATS_BALANCE_COLUMN_WIDTH),
            Constraint::Length(4),
        ]
    } else {
        vec![
            Constraint::Min(16),
            Constraint::Length(STATS_ENDPOINT_BALANCE_COLUMN_WIDTH),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Length(12),
        ]
    };

    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .title(endpoint_table_title(
                endpoints.len(),
                scroll,
                visible_rows,
                lang,
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.border)),
    );
    f.render_widget(table, area);
}

fn endpoint_visible_rows(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(3))
}

fn endpoint_table_title(
    total: usize,
    scroll: usize,
    visible_rows: usize,
    lang: Language,
) -> String {
    let base = i18n::label(lang, "Endpoints / recent sample");
    if total > visible_rows && visible_rows > 0 {
        format!(
            "{base}  PgUp/PgDn {}-{} / {total}",
            scroll.saturating_add(1),
            scroll.saturating_add(visible_rows).min(total)
        )
    } else {
        base.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_detail_panel(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    usage_balance: &UsageBalanceView,
    _provider_rows: &[&UsageBalanceProviderRow],
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
            ui.selected_usage_balance_provider_row(usage_balance),
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
            Span::raw("  "),
            Span::styled("a", Style::default().fg(p.text)),
            Span::styled(
                format!(" {}", l("attention only")),
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
            Span::raw("  "),
            Span::styled("PgUp/PgDn", Style::default().fg(p.text)),
            Span::styled(
                match lang {
                    Language::Zh => " 详情滚动",
                    Language::En => " detail scroll",
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

#[cfg(test)]
mod tests;
