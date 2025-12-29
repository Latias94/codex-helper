use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Sparkline, Table, Wrap};

use crate::state::UsageBucket;
use crate::tui::ProviderOption;
use crate::tui::model::{Palette, Snapshot, tokens_short};
use crate::tui::state::UiState;

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
    _ui: &mut UiState,
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
    render_tables(f, p, snapshot, rows[2]);
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

fn render_tables(f: &mut Frame<'_>, p: Palette, snapshot: &Snapshot, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_bucket_table(
        f,
        p,
        "Top configs (by total tokens)",
        &snapshot.usage_rollup.by_config,
        cols[0],
    );
    render_bucket_table(
        f,
        p,
        "Top providers (by total tokens)",
        &snapshot.usage_rollup.by_provider,
        cols[1],
    );
}

fn render_bucket_table(
    f: &mut Frame<'_>,
    p: Palette,
    title: &str,
    items: &[(String, UsageBucket)],
    area: Rect,
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
            .border_style(Style::default().fg(p.border)),
    )
    .row_highlight_style(Style::default().fg(p.text));

    f.render_widget(table, area);
}
