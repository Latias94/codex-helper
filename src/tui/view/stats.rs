use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Sparkline, Table, Wrap};

use crate::state::UsageBucket;
use crate::tui::ProviderOption;
use crate::tui::model::{Palette, Snapshot, shorten, shorten_middle, tokens_short};
use crate::tui::state::UiState;
use crate::tui::types::StatsFocus;

fn pricing_per_1k_usd() -> Option<(f64, f64)> {
    let input = std::env::var("CODEX_HELPER_PRICE_INPUT_PER_1K_USD")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())?;
    let output = std::env::var("CODEX_HELPER_PRICE_OUTPUT_PER_1K_USD")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())?;
    if input.is_finite() && output.is_finite() && input >= 0.0 && output >= 0.0 {
        Some((input, output))
    } else {
        None
    }
}

fn estimate_cost_usd(bucket: &UsageBucket) -> Option<f64> {
    let (input_price, output_price) = pricing_per_1k_usd()?;
    let input = (bucket.usage.input_tokens.max(0) as f64) / 1000.0;
    let output = (bucket.usage.output_tokens.max(0) as f64) / 1000.0;
    Some(input * input_price + output * output_price)
}

fn fmt_pct(num: u64, den: u64) -> String {
    if den == 0 {
        return "-".to_string();
    }
    format!("{:.1}%", (num as f64) * 100.0 / (den as f64))
}

fn fmt_avg_ms(total_ms: u64, n: u64) -> String {
    if n == 0 {
        return "-".to_string();
    }
    format!("{}ms", total_ms / n)
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

    render_kpis(f, p, snapshot, rows[0]);
    render_sparkline(f, p, snapshot, rows[1]);
    render_tables(f, p, ui, snapshot, rows[2]);
}

fn render_kpis(f: &mut Frame<'_>, p: Palette, snapshot: &Snapshot, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(area);

    let s = &snapshot.usage_rollup.since_start;
    let err_pct = fmt_pct(s.requests_error, s.requests_total);
    let avg_ms = fmt_avg_ms(s.duration_ms_total, s.requests_total);
    let tokens = &s.usage;
    let cost = estimate_cost_usd(s).map(|v| format!("${v:.2}"));
    let cost_hint = if pricing_per_1k_usd().is_some() {
        cost.unwrap_or_else(|| "-".to_string())
    } else {
        "(set CODEX_HELPER_PRICE_* env)".to_string()
    };

    let b1 = Block::default()
        .title("Requests (since start)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let t1 = Text::from(vec![
        Line::from(vec![
            Span::styled("total  ", Style::default().fg(p.muted)),
            Span::styled(s.requests_total.to_string(), Style::default().fg(p.text)),
            Span::raw("   "),
            Span::styled("errors  ", Style::default().fg(p.muted)),
            Span::styled(s.requests_error.to_string(), Style::default().fg(p.warn)),
        ]),
        Line::from(vec![
            Span::styled("err%   ", Style::default().fg(p.muted)),
            Span::styled(err_pct, Style::default().fg(p.warn)),
            Span::raw("   "),
            Span::styled("avg   ", Style::default().fg(p.muted)),
            Span::styled(avg_ms, Style::default().fg(p.text)),
        ]),
    ]);
    f.render_widget(
        Paragraph::new(t1).block(b1).wrap(Wrap { trim: true }),
        cols[0],
    );

    let b2 = Block::default()
        .title("Tokens (since start)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let t2 = Text::from(vec![
        Line::from(vec![
            Span::styled("in   ", Style::default().fg(p.muted)),
            Span::styled(
                tokens_short(tokens.input_tokens),
                Style::default().fg(p.text),
            ),
            Span::raw("   "),
            Span::styled("out   ", Style::default().fg(p.muted)),
            Span::styled(
                tokens_short(tokens.output_tokens),
                Style::default().fg(p.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("rsn  ", Style::default().fg(p.muted)),
            Span::styled(
                tokens_short(tokens.reasoning_tokens),
                Style::default().fg(p.text),
            ),
            Span::raw("   "),
            Span::styled("ttl  ", Style::default().fg(p.muted)),
            Span::styled(
                tokens_short(tokens.total_tokens),
                Style::default().fg(p.text),
            ),
        ]),
    ]);
    f.render_widget(
        Paragraph::new(t2).block(b2).wrap(Wrap { trim: true }),
        cols[1],
    );

    let b3 = Block::default()
        .title("Cost (estimated)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let t3 = Text::from(vec![
        Line::from(vec![
            Span::styled("usd  ", Style::default().fg(p.muted)),
            Span::styled(cost_hint, Style::default().fg(p.accent)),
        ]),
        Line::from(vec![
            Span::styled(
                "pricing  ",
                Style::default().fg(p.muted).add_modifier(Modifier::DIM),
            ),
            Span::styled(
                "global per-1k (env)",
                Style::default().fg(p.muted).add_modifier(Modifier::DIM),
            ),
        ]),
    ]);
    f.render_widget(
        Paragraph::new(t3).block(b3).wrap(Wrap { trim: true }),
        cols[2],
    );
}

fn render_sparkline(f: &mut Frame<'_>, p: Palette, snapshot: &Snapshot, area: Rect) {
    let values = snapshot
        .usage_rollup
        .by_day
        .iter()
        .map(|(_, b)| b.usage.total_tokens.max(0) as u64)
        .collect::<Vec<_>>();
    let block = Block::default()
        .title("Tokens / day (rollup)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));

    let widget = Sparkline::default()
        .block(block)
        .style(Style::default().fg(p.accent))
        .data(&values);
    f.render_widget(widget, area);
}

fn render_tables(f: &mut Frame<'_>, p: Palette, ui: &mut UiState, snapshot: &Snapshot, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(cols[0]);

    render_bucket_table_stateful(
        f,
        p,
        ui.stats_focus == StatsFocus::Configs,
        "Top configs (by total tokens)",
        &snapshot.usage_rollup.by_config,
        left[0],
        &mut ui.stats_configs_table,
    );
    render_bucket_table_stateful(
        f,
        p,
        ui.stats_focus == StatsFocus::Providers,
        "Top providers (by total tokens)",
        &snapshot.usage_rollup.by_provider,
        left[1],
        &mut ui.stats_providers_table,
    );

    render_detail_panel(f, p, ui, snapshot, cols[1]);
}

fn render_bucket_table_stateful(
    f: &mut Frame<'_>,
    p: Palette,
    focused: bool,
    title: &str,
    items: &[(String, UsageBucket)],
    area: Rect,
    state: &mut ratatui::widgets::TableState,
) {
    let header = Row::new(vec![
        Cell::from(Span::styled("name", Style::default().fg(p.muted))),
        Cell::from(Span::styled("req", Style::default().fg(p.muted))),
        Cell::from(Span::styled("err%", Style::default().fg(p.muted))),
        Cell::from(Span::styled("tok", Style::default().fg(p.muted))),
        Cell::from(Span::styled("avg", Style::default().fg(p.muted))),
        Cell::from(Span::styled("usd", Style::default().fg(p.muted))),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows = items
        .iter()
        .map(|(name, b)| {
            let cost = estimate_cost_usd(b)
                .map(|v| format!("{v:.2}"))
                .unwrap_or_else(|| "-".to_string());
            Row::new(vec![
                Cell::from(name.clone()),
                Cell::from(b.requests_total.to_string()),
                Cell::from(fmt_pct(b.requests_error, b.requests_total)),
                Cell::from(tokens_short(b.usage.total_tokens)),
                Cell::from(fmt_avg_ms(b.duration_ms_total, b.requests_total)),
                Cell::from(cost),
            ])
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(40),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(6),
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
    area: Rect,
) {
    let selected = match ui.stats_focus {
        StatsFocus::Configs => snapshot
            .usage_rollup
            .by_config
            .get(ui.selected_stats_config_idx)
            .map(|(k, v)| ("config", k.as_str(), v)),
        StatsFocus::Providers => snapshot
            .usage_rollup
            .by_provider
            .get(ui.selected_stats_provider_idx)
            .map(|(k, v)| ("provider", k.as_str(), v)),
    };

    let block = Block::default()
        .title(Span::styled(
            format!("Detail  window: {}d", ui.stats_days),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));

    let Some((kind, name, bucket)) = selected else {
        f.render_widget(
            Paragraph::new(Text::from(Line::from(Span::styled(
                "No data yet.",
                Style::default().fg(p.muted),
            ))))
            .block(block)
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    };

    let series = match kind {
        "config" => snapshot
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
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Min(0),
        ])
        .split(area);

    let err_pct = fmt_pct(bucket.requests_error, bucket.requests_total);
    let avg_ms = fmt_avg_ms(bucket.duration_ms_total, bucket.requests_total);
    let cost = estimate_cost_usd(bucket)
        .map(|v| format!("${v:.2}"))
        .unwrap_or_else(|| "-".to_string());

    let lines = vec![
        Line::from(vec![
            Span::styled(format!("{kind}: "), Style::default().fg(p.muted)),
            Span::styled(name.to_string(), Style::default().fg(p.text)),
        ]),
        Line::from(vec![
            Span::styled("requests  ", Style::default().fg(p.muted)),
            Span::styled(
                bucket.requests_total.to_string(),
                Style::default().fg(p.text),
            ),
            Span::raw("   "),
            Span::styled("errors  ", Style::default().fg(p.muted)),
            Span::styled(
                bucket.requests_error.to_string(),
                Style::default().fg(p.warn),
            ),
            Span::raw("   "),
            Span::styled("err%  ", Style::default().fg(p.muted)),
            Span::styled(err_pct, Style::default().fg(p.warn)),
        ]),
        Line::from(vec![
            Span::styled("tokens  ", Style::default().fg(p.muted)),
            Span::styled(
                tokens_short(bucket.usage.total_tokens),
                Style::default().fg(p.accent),
            ),
            Span::raw("   "),
            Span::styled("avg  ", Style::default().fg(p.muted)),
            Span::styled(avg_ms, Style::default().fg(p.text)),
            Span::raw("   "),
            Span::styled("usd  ", Style::default().fg(p.muted)),
            Span::styled(cost, Style::default().fg(p.muted)),
        ]),
        Line::from(vec![
            Span::styled("tok(in/out/rsn)  ", Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "{}/{}/{}",
                    tokens_short(bucket.usage.input_tokens),
                    tokens_short(bucket.usage.output_tokens),
                    tokens_short(bucket.usage.reasoning_tokens),
                ),
                Style::default().fg(p.muted),
            ),
        ]),
        Line::from(vec![Span::styled(
            "pricing: set CODEX_HELPER_PRICE_* env for usd",
            Style::default().fg(p.muted).add_modifier(Modifier::DIM),
        )]),
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
            Span::styled("1-6", Style::default().fg(p.text)),
            Span::styled(" pages", Style::default().fg(p.muted)),
        ]),
    ]);

    let errors_only = ui.stats_errors_only;
    let mut recent_total = 0u64;
    let mut recent_err = 0u64;
    let mut class_2xx = 0u64;
    let mut class_3xx = 0u64;
    let mut class_4xx = 0u64;
    let mut class_5xx = 0u64;
    let mut by_model: std::collections::HashMap<String, (u64, i64)> =
        std::collections::HashMap::new();
    let mut by_path: std::collections::HashMap<String, (u64, u64, i64)> =
        std::collections::HashMap::new();
    let mut by_status: std::collections::HashMap<u16, u64> = std::collections::HashMap::new();

    for r in &snapshot.recent {
        let matches = match ui.stats_focus {
            StatsFocus::Configs => r.config_name.as_deref() == Some(name),
            StatsFocus::Providers => r.provider_id.as_deref() == Some(name),
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
            _ => class_5xx += 1,
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
        by_path
            .entry(r.path.clone())
            .and_modify(|(c, e, t)| {
                *c = c.saturating_add(1);
                if r.status_code >= 400 {
                    *e = e.saturating_add(1);
                }
                *t = t.saturating_add(tokens);
            })
            .or_insert((1, if r.status_code >= 400 { 1 } else { 0 }, tokens));
    }

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            "Recent breakdown ",
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
        Span::styled("req  ", Style::default().fg(p.muted)),
        Span::styled(recent_total.to_string(), Style::default().fg(p.text)),
        Span::raw("   "),
        Span::styled("err  ", Style::default().fg(p.muted)),
        Span::styled(recent_err.to_string(), Style::default().fg(p.warn)),
        Span::raw("   "),
        Span::styled("2xx/3xx/4xx/5xx  ", Style::default().fg(p.muted)),
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
            Span::styled("top status  ", Style::default().fg(p.muted)),
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
            Span::styled("top models  ", Style::default().fg(p.muted)),
            Span::styled(shorten(&top_models, 56), Style::default().fg(p.muted)),
        ]));
    }

    let mut paths = by_path.into_iter().collect::<Vec<_>>();
    paths.sort_by_key(|(_, (_, _, tok))| std::cmp::Reverse(*tok));
    let top_paths = paths
        .into_iter()
        .take(5)
        .map(|(path, (c, e, tok))| {
            let path = shorten_middle(&path, 22);
            if e > 0 {
                format!("{path}({c} err{e} / {})", tokens_short(tok))
            } else {
                format!("{path}({c} / {})", tokens_short(tok))
            }
        })
        .collect::<Vec<_>>()
        .join("  ");
    if !top_paths.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("top paths   ", Style::default().fg(p.muted)),
            Span::styled(shorten(&top_paths, 56), Style::default().fg(p.muted)),
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
                        "Recent (<=200) + Tips",
                        Style::default().fg(p.muted).add_modifier(Modifier::DIM),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(p.border)),
            )
            .wrap(Wrap { trim: true }),
        inner[2],
    );
}
