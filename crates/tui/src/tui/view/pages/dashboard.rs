use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Table, Wrap,
};

use crate::dashboard_core::WindowStats;
use crate::pricing::CostSummary;
use crate::state::FinishedRequest;
use crate::tui::ProviderOption;
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_status_style, basename, duration_short, format_age, now_ms,
    session_balance_brief, session_control_posture, session_primary_balance_snapshot,
    session_row_has_any_override, short_sid, shorten, shorten_middle, status_style, tokens_short,
    usage_line,
};
use crate::tui::state::UiState;
use crate::tui::types::{Focus, Overlay};
use crate::tui::view::widgets::kv_line;
use crate::usage::UsageMetrics;

pub(super) fn render_dashboard(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    if area.width < 100 {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
            .split(area);

        render_sessions_stack(f, p, ui, snapshot, rows[0]);
        render_overview_and_requests(f, p, ui, snapshot, providers, rows[1]);
        return;
    }

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    render_sessions_stack(f, p, ui, snapshot, columns[0]);
    render_overview_and_requests(f, p, ui, snapshot, providers, columns[1]);
}

fn render_sessions_stack(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    if area.height < 15 {
        render_sessions_table(f, p, ui, snapshot, area);
        return;
    }

    let summary_height = if area.height >= 24 { 9 } else { 7 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(7), Constraint::Length(summary_height)])
        .split(area);

    render_sessions_table(f, p, ui, snapshot, chunks[0]);
    render_session_summary_panel(f, p, ui, snapshot, chunks[1]);
}

fn render_sessions_table(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let title = Span::styled(
        "Sessions",
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    );
    let focused = ui.focus == Focus::Sessions && ui.overlay == Overlay::None;
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { p.focus } else { p.border }))
        .style(Style::default().bg(p.panel));

    let now = now_ms();
    let compact = area.width < 58;

    let header = if compact {
        Row::new(vec![
            Cell::from(Span::styled("Session", Style::default().fg(p.muted))),
            Cell::from(Span::styled("St", Style::default().fg(p.muted))),
            Cell::from(Span::styled("Age", Style::default().fg(p.muted))),
            Cell::from(Span::styled("Tok", Style::default().fg(p.muted))),
        ])
    } else {
        Row::new(vec![
            Cell::from(Span::styled("Session", Style::default().fg(p.muted))),
            Cell::from(Span::styled("CWD", Style::default().fg(p.muted))),
            Cell::from(Span::styled("St", Style::default().fg(p.muted))),
            Cell::from(Span::styled("Age", Style::default().fg(p.muted))),
            Cell::from(Span::styled("Tok", Style::default().fg(p.muted))),
        ])
    }
    .height(1)
    .style(Style::default().bg(p.panel));

    let rows = snapshot
        .rows
        .iter()
        .map(|r| {
            let sid = r
                .session_id
                .as_deref()
                .map(|s| short_sid(s, if compact { 10 } else { 12 }))
                .unwrap_or_else(|| "-".to_string());

            let cwd = r
                .cwd
                .as_deref()
                .map(basename)
                .map(|s| shorten(s, if area.width >= 76 { 24 } else { 18 }))
                .unwrap_or_else(|| "-".to_string());

            let last = match r.last_status {
                Some(s) => Span::styled(s.to_string(), status_style(p, Some(s))),
                None => Span::styled("-", Style::default().fg(p.muted)),
            };

            let age = if r.active_count > 0 {
                format_age(now, r.active_started_at_ms_min)
            } else {
                format_age(now, r.last_ended_at_ms)
            };

            let total_tokens = r.total_usage.as_ref().map(|u| u.total_tokens).unwrap_or(0);
            let tok = if total_tokens > 0 {
                Span::styled(tokens_short(total_tokens), Style::default().fg(p.accent))
            } else {
                Span::styled("-", Style::default().fg(p.muted))
            };

            let mut badges = Vec::new();
            if r.active_count > 0 {
                badges.push(Span::styled(
                    format!("RUN{}", r.active_count),
                    Style::default().fg(p.good).add_modifier(Modifier::BOLD),
                ));
            }
            if r.override_effort.is_some() {
                badges.push(Span::styled("E", Style::default().fg(p.accent)));
            }
            if r.override_station_name.is_some() {
                badges.push(Span::styled("C", Style::default().fg(p.accent)));
            }
            if r.override_model.is_some() {
                badges.push(Span::styled("M", Style::default().fg(p.accent)));
            }
            if r.override_service_tier.is_some() {
                badges.push(Span::styled("T", Style::default().fg(p.accent)));
            }

            let mut session_spans = vec![Span::styled(sid, Style::default().fg(p.text))];
            for b in badges {
                session_spans.push(Span::raw(" "));
                session_spans.push(Span::raw("["));
                session_spans.push(b);
                session_spans.push(Span::raw("]"));
            }

            let mut row_style = Style::default().fg(p.text).bg(p.panel);
            if session_row_has_any_override(r) {
                row_style = row_style.add_modifier(Modifier::ITALIC);
            }

            let cells = if compact {
                vec![
                    Cell::from(Line::from(session_spans)),
                    Cell::from(Line::from(vec![last])),
                    Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                    Cell::from(Line::from(vec![tok])),
                ]
            } else {
                vec![
                    Cell::from(Line::from(session_spans)),
                    Cell::from(cwd),
                    Cell::from(Line::from(vec![last])),
                    Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                    Cell::from(Line::from(vec![tok])),
                ]
            };

            Row::new(cells).style(row_style)
        })
        .collect::<Vec<_>>();

    let widths: Vec<Constraint> = if compact {
        [
            Constraint::Length(16),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(6),
        ]
        .into()
    } else {
        [
            Constraint::Length(18),
            Constraint::Min(12),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(6),
        ]
        .into()
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
        .highlight_spacing(HighlightSpacing::Always);

    f.render_stateful_widget(table, area, &mut ui.sessions_table);

    let visible_rows = area.height.saturating_sub(3) as usize;
    if snapshot.rows.len() > visible_rows {
        let mut scrollbar =
            ScrollbarState::new(snapshot.rows.len()).position(ui.sessions_table.offset());
        let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(p.border));
        f.render_stateful_widget(sb, area, &mut scrollbar);
    }
}

fn render_overview_and_requests(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    if area.height < 14 {
        render_requests_panel(f, p, ui, snapshot, providers, area);
        return;
    }

    let health_height = if area.height >= 26 { 8 } else { 6 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(health_height), Constraint::Min(0)])
        .split(area);

    render_recent_health_panel(f, p, snapshot, chunks[0]);
    render_requests_panel(f, p, ui, snapshot, providers, chunks[1]);
}

fn render_session_summary_panel(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let selected = snapshot.rows.get(ui.selected_session_idx);
    let value_width = inner_value_width(area, 11);
    let mut lines = Vec::new();

    if let Some(row) = selected {
        let sid = row.session_id.as_deref().unwrap_or("-");
        let cwd = row
            .cwd
            .as_deref()
            .map(|s| shorten_middle(s, value_width))
            .unwrap_or_else(|| "-".to_string());
        let station = row.last_station_name.as_deref().unwrap_or("-");
        let provider = row.last_provider_id.as_deref().unwrap_or("-");
        let balance = session_balance_brief(row, &snapshot.provider_balances, value_width);
        let balance_snapshot = session_primary_balance_snapshot(row, &snapshot.provider_balances);
        let provider_line = match balance.as_deref() {
            Some(balance) if provider != "-" => format!("{provider} | {balance}"),
            Some(balance) => balance.to_string(),
            None => provider.to_string(),
        };
        let model = row
            .override_model
            .as_deref()
            .or(row.last_model.as_deref())
            .unwrap_or("-");
        let effort = row
            .override_effort
            .as_deref()
            .or(row.last_reasoning_effort.as_deref())
            .unwrap_or("-");
        let service_tier = row
            .override_service_tier
            .as_deref()
            .or(row
                .effective_service_tier
                .as_ref()
                .map(|value| value.value.as_str()))
            .or(row.last_service_tier.as_deref())
            .unwrap_or("-");
        let now = now_ms();
        let active_age = if row.active_count > 0 {
            format_age(now, row.active_started_at_ms_min)
        } else {
            "-".to_string()
        };
        let last_age = format_age(now, row.last_ended_at_ms);
        let last_status = row.last_status;
        let last_status_text = last_status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());
        let last_dur = row
            .last_duration_ms
            .map(duration_short)
            .unwrap_or_else(|| "-".to_string());
        let activity_age = if row.active_count > 0 {
            active_age
        } else {
            last_age
        };
        let turns_total = row.turns_total.unwrap_or(0);
        let turns_with_usage = row.turns_with_usage.unwrap_or(0);
        let last_usage = row
            .last_usage
            .as_ref()
            .map(usage_line)
            .unwrap_or_else(|| "tok in/out/rsn/ttl: -".to_string());
        let total_usage = row
            .total_usage
            .as_ref()
            .filter(|u| u.total_tokens > 0)
            .map(usage_line)
            .unwrap_or_else(|| "tok in/out/rsn/ttl: -".to_string());
        let posture = session_control_posture(row, snapshot.global_station_override.as_deref());

        lines.push(kv_line(
            p,
            "session",
            format!(
                "{} active={} last={}/{} age={}",
                short_sid(sid, 18),
                row.active_count,
                last_status_text,
                last_dur,
                activity_age
            ),
            status_style(p, last_status),
        ));
        lines.push(kv_line(p, "cwd", cwd, Style::default().fg(p.text)));
        lines.push(kv_line(
            p,
            "route",
            shorten_middle(
                &format!("station={station} provider={provider_line}"),
                value_width,
            ),
            balance_snapshot
                .map(|snapshot| balance_snapshot_status_style(p, snapshot))
                .unwrap_or_else(|| Style::default().fg(p.text)),
        ));
        lines.push(kv_line(
            p,
            "model",
            shorten_middle(
                &format!(
                    "{} effort={} tier={}",
                    shorten(model, 28),
                    effort,
                    service_tier
                ),
                value_width,
            ),
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            "last",
            shorten_middle(&last_usage, value_width),
            Style::default().fg(p.muted),
        ));
        lines.push(kv_line(
            p,
            "sum",
            shorten_middle(
                &format!("{total_usage} | turns {turns_total}/{turns_with_usage}"),
                value_width,
            ),
            Style::default().fg(p.accent),
        ));
        lines.push(kv_line(
            p,
            "control",
            shorten_middle(&posture.headline, value_width),
            Style::default().fg(posture.color),
        ));
    } else {
        lines.push(Line::from(Span::styled(
            "No sessions observed yet.",
            Style::default().fg(p.muted),
        )));
    }

    let title = Span::styled(
        "Session Summary",
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));
    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.text))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

fn render_recent_health_panel(f: &mut Frame<'_>, p: Palette, snapshot: &Snapshot, area: Rect) {
    let now = now_ms();
    let summary = summarize_recent_usage(&snapshot.recent, now, 60 * 60 * 1_000);
    let errors_style = if summary.errors > 0 {
        Style::default().fg(p.warn)
    } else {
        Style::default().fg(p.good)
    };

    let mut lines = vec![
        stats_line(p, "5m", &snapshot.stats_5m),
        stats_line(p, "1h", &snapshot.stats_1h),
        kv_line(
            p,
            "recent_1h",
            format!(
                "req={} err={} retry={} failover={}",
                summary.requests, summary.errors, summary.retries, summary.failovers
            ),
            errors_style,
        ),
        kv_line(
            p,
            "usage_1h",
            format!(
                "tok={} in/out={}/{} cache={}/{} cost={}",
                tokens_short(summary.usage.total_tokens),
                tokens_short(summary.usage.input_tokens),
                tokens_short(summary.usage.output_tokens),
                tokens_short(
                    summary
                        .usage
                        .cache_read_input_tokens
                        .max(summary.usage.cached_input_tokens)
                ),
                tokens_short(summary.usage.cache_creation_tokens_total()),
                summary.cost.display_total_with_confidence(),
            ),
            Style::default().fg(p.muted),
        ),
    ];

    if area.height >= 8 {
        lines.push(kv_line(
            p,
            "top_1h",
            top_route_label(&snapshot.stats_1h),
            Style::default().fg(p.text),
        ));
    }

    let title = Span::styled(
        "Recent Health",
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));
    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.text))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

fn render_requests_panel(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    _providers: &[ProviderOption],
    area: Rect,
) {
    let focused = ui.focus == Focus::Requests && ui.overlay == Overlay::None;
    let selected_sid = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|r| r.session_id.as_deref())
        .map(|s| s.to_string());

    let filtered = snapshot
        .recent
        .iter()
        .filter(|r| match (&selected_sid, &r.session_id) {
            (Some(sid), Some(rid)) => sid == rid,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .take(60)
        .collect::<Vec<_>>();

    let title = Span::styled(
        request_panel_title(selected_sid.as_deref(), &filtered, area.width),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { p.focus } else { p.border }))
        .style(Style::default().bg(p.panel));

    let layout = if area.width >= 92 {
        RequestTableLayout::Wide
    } else if area.width >= 64 {
        RequestTableLayout::Regular
    } else {
        RequestTableLayout::Narrow
    };

    let header = match layout {
        RequestTableLayout::Wide => Row::new(["Age", "St", "Route", "Model", "Lat", "Tok", "Cost"]),
        RequestTableLayout::Regular => Row::new(["Age", "St", "Route", "Lat", "Tok", "Cost"]),
        RequestTableLayout::Narrow => Row::new(["Age", "St", "Route", "Tok"]),
    }
    .style(Style::default().fg(p.muted))
    .height(1);

    let now = now_ms();
    let rows = filtered
        .iter()
        .map(|r| {
            let age = format_age(now, Some(r.ended_at_ms));
            let status = Span::styled(
                r.status_code.to_string(),
                status_style(p, Some(r.status_code)),
            );
            let route = request_route_label(r);
            let route_style = Style::default().fg(if r.attempt_count() > 1 {
                p.warn
            } else {
                p.text
            });
            let latency = request_latency_label(r);
            let tokens = request_tokens_label(r);
            let cost = request_cost_label(r);
            let model = r.model.as_deref().unwrap_or("-");

            let cells = match layout {
                RequestTableLayout::Wide => vec![
                    Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                    Cell::from(Line::from(vec![status])),
                    Cell::from(Span::styled(shorten_middle(&route, 28), route_style)),
                    Cell::from(shorten_middle(model, 20)),
                    Cell::from(Span::styled(latency, Style::default().fg(p.muted))),
                    Cell::from(Span::styled(tokens, Style::default().fg(p.accent))),
                    Cell::from(Span::styled(
                        shorten_middle(&cost, 10),
                        Style::default().fg(p.text),
                    )),
                ],
                RequestTableLayout::Regular => vec![
                    Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                    Cell::from(Line::from(vec![status])),
                    Cell::from(Span::styled(shorten_middle(&route, 24), route_style)),
                    Cell::from(Span::styled(latency, Style::default().fg(p.muted))),
                    Cell::from(Span::styled(tokens, Style::default().fg(p.accent))),
                    Cell::from(Span::styled(
                        shorten_middle(&cost, 9),
                        Style::default().fg(p.text),
                    )),
                ],
                RequestTableLayout::Narrow => vec![
                    Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                    Cell::from(Line::from(vec![status])),
                    Cell::from(Span::styled(shorten_middle(&route, 18), route_style)),
                    Cell::from(Span::styled(tokens, Style::default().fg(p.accent))),
                ],
            };

            Row::new(cells).style(Style::default().bg(p.panel).fg(p.text))
        })
        .collect::<Vec<_>>();

    let widths: Vec<Constraint> = match layout {
        RequestTableLayout::Wide => [
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Min(18),
            Constraint::Length(20),
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Length(10),
        ]
        .into(),
        RequestTableLayout::Regular => [
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Min(12),
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Length(9),
        ]
        .into(),
        RequestTableLayout::Narrow => [
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(12),
        ]
        .into(),
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
        .highlight_spacing(HighlightSpacing::Always);

    f.render_stateful_widget(table, area, &mut ui.requests_table);

    let visible_rows = area.height.saturating_sub(3) as usize;
    if filtered.len() > visible_rows {
        let mut scrollbar =
            ScrollbarState::new(filtered.len()).position(ui.requests_table.offset());
        let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(p.border));
        f.render_stateful_widget(sb, area, &mut scrollbar);
    }
}

#[derive(Debug, Clone, Copy)]
enum RequestTableLayout {
    Narrow,
    Regular,
    Wide,
}

#[derive(Default)]
struct RecentUsageSummary {
    requests: usize,
    errors: usize,
    retries: usize,
    failovers: usize,
    usage: UsageMetrics,
    cost: CostSummary,
}

fn summarize_recent_usage(
    recent: &[FinishedRequest],
    now_ms: u64,
    window_ms: u64,
) -> RecentUsageSummary {
    let cutoff = now_ms.saturating_sub(window_ms);
    let mut summary = RecentUsageSummary::default();

    for request in recent
        .iter()
        .filter(|request| request.ended_at_ms >= cutoff)
    {
        summary.requests += 1;
        if request.status_code >= 400 {
            summary.errors += 1;
        }
        if request.attempt_count() > 1 {
            summary.retries += 1;
        }
        if request.crossed_station_boundary() {
            summary.failovers += 1;
        }
        if let Some(usage) = request.usage.as_ref() {
            summary.usage.add_assign(usage);
        }
        summary.cost.record_usage_cost(&request.cost);
    }

    summary
}

fn request_panel_title(
    selected_sid: Option<&str>,
    filtered: &[&FinishedRequest],
    width: u16,
) -> String {
    let sid = selected_sid.map(|sid| short_sid(sid, 12));
    if width < 68 {
        return sid
            .map(|sid| format!("Requests [{sid}]"))
            .unwrap_or_else(|| "Requests".to_string());
    }

    let errors = filtered
        .iter()
        .filter(|request| request.status_code >= 400)
        .count();
    let retries = filtered
        .iter()
        .filter(|request| request.attempt_count() > 1)
        .count();

    match sid {
        Some(sid) => format!(
            "Requests [{sid}]  rows={} err={} retry={}",
            filtered.len(),
            errors,
            retries
        ),
        None => format!(
            "Requests  rows={} err={} retry={}",
            filtered.len(),
            errors,
            retries
        ),
    }
}

fn request_latency_label(request: &FinishedRequest) -> String {
    let ttfb = request
        .ttfb_ms
        .map(duration_short)
        .unwrap_or_else(|| "-".to_string());
    format!("{ttfb}/{}", duration_short(request.duration_ms))
}

fn request_tokens_label(request: &FinishedRequest) -> String {
    request
        .usage
        .as_ref()
        .map(|usage| {
            format!(
                "{}/{}/{}",
                tokens_short(usage.input_tokens),
                tokens_short(usage.output_tokens),
                tokens_short(usage.total_tokens)
            )
        })
        .unwrap_or_else(|| "-".to_string())
}

fn request_cost_label(request: &FinishedRequest) -> String {
    request.cost.display_total_with_confidence()
}

fn request_route_label(request: &FinishedRequest) -> String {
    let mut route = Vec::new();

    if let Some(retry) = request.retry.as_ref() {
        for attempt in retry.route_attempts_or_derived() {
            if let Some(label) = request_target_label(
                attempt.station_name.as_deref(),
                attempt.provider_id.as_deref(),
            ) && route.last() != Some(&label)
            {
                route.push(label);
            }
        }
    }

    if let Some(final_label) = request_target_label(
        request.station_name.as_deref(),
        request.provider_id.as_deref(),
    ) && route.last() != Some(&final_label)
    {
        route.push(final_label);
    }

    match route.as_slice() {
        [] => "-".to_string(),
        [only] => only.clone(),
        [first, second] => format!("{first}>{second}"),
        [first, .., last] => format!("{first}>{last}+{}", route.len().saturating_sub(2)),
    }
}

fn request_target_label(station: Option<&str>, provider: Option<&str>) -> Option<String> {
    let station = station.map(str::trim).filter(|value| !value.is_empty());
    let provider = provider.map(str::trim).filter(|value| !value.is_empty());

    match (station, provider) {
        (Some(station), Some(provider)) if station != provider => {
            Some(format!("{station}/{provider}"))
        }
        (Some(station), _) => Some(station.to_string()),
        (_, Some(provider)) => Some(provider.to_string()),
        (None, None) => None,
    }
}

fn stats_line(p: Palette, label: &'static str, stats: &WindowStats) -> Line<'static> {
    let errors = stats
        .err_429
        .saturating_add(stats.err_4xx)
        .saturating_add(stats.err_5xx);
    let p95 = stats
        .p95_ms
        .map(duration_short)
        .unwrap_or_else(|| "-".to_string());
    let retry = percent_label(stats.retry_rate);
    let style = if errors > 0 {
        Style::default().fg(p.warn)
    } else if stats.total > 0 {
        Style::default().fg(p.good)
    } else {
        Style::default().fg(p.muted)
    };

    kv_line(
        p,
        label,
        format!(
            "req={} ok={} err={} p95={} retry={} top={}",
            stats.total,
            stats.ok_2xx,
            errors,
            p95,
            retry,
            top_route_label(stats)
        ),
        style,
    )
}

fn percent_label(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{:.0}%", value * 100.0))
        .unwrap_or_else(|| "-".to_string())
}

fn top_route_label(stats: &WindowStats) -> String {
    match (&stats.top_config, &stats.top_provider) {
        (Some((station, _)), Some((provider, _))) if station != provider => {
            format!("{station}/{provider}")
        }
        (Some((station, _)), _) => station.clone(),
        (_, Some((provider, _))) => provider.clone(),
        (None, None) => "-".to_string(),
    }
}

fn inner_value_width(area: Rect, reserved: usize) -> usize {
    (area.width as usize)
        .saturating_sub(4)
        .saturating_sub(reserved)
        .max(8)
}
