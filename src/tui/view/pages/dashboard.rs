use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Table, Wrap,
};

use crate::tui::ProviderOption;
use crate::tui::model::{
    Palette, Snapshot, basename, format_age, now_ms, short_sid, shorten, status_style,
    shorten_middle, tokens_short, usage_line,
};
use crate::tui::state::UiState;
use crate::tui::types::{Focus, Overlay};
use crate::tui::view::widgets::kv_line;

pub(super) fn render_dashboard(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    render_sessions_panel(f, p, ui, snapshot, columns[0]);
    render_details_and_requests(f, p, ui, snapshot, providers, columns[1]);
}

fn render_sessions_panel(
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

    let header = Row::new(vec![
        Cell::from(Span::styled("Session", Style::default().fg(p.muted))),
        Cell::from(Span::styled("CWD", Style::default().fg(p.muted))),
        Cell::from(Span::styled("A", Style::default().fg(p.muted))),
        Cell::from(Span::styled("Last", Style::default().fg(p.muted))),
        Cell::from(Span::styled("Age", Style::default().fg(p.muted))),
        Cell::from(Span::styled("ΣTok", Style::default().fg(p.muted))),
    ])
    .height(1)
    .style(Style::default().bg(p.panel));

    let rows = snapshot
        .rows
        .iter()
        .map(|r| {
            let sid = r
                .session_id
                .as_deref()
                .map(|s| short_sid(s, 12))
                .unwrap_or_else(|| "-".to_string());

            let cwd = r
                .cwd
                .as_deref()
                .map(basename)
                .map(|s| shorten(s, 18))
                .unwrap_or_else(|| "-".to_string());

            let active = if r.active_count > 0 {
                Span::styled(r.active_count.to_string(), Style::default().fg(p.good))
            } else {
                Span::styled("-", Style::default().fg(p.muted))
            };

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
                    "RUN",
                    Style::default().fg(p.good).add_modifier(Modifier::BOLD),
                ));
            }
            if r.override_effort.is_some() {
                badges.push(Span::styled("E", Style::default().fg(p.accent)));
            }
            if r.override_config_name.is_some() {
                badges.push(Span::styled("C", Style::default().fg(p.accent)));
            }

            let mut session_spans = vec![Span::styled(sid, Style::default().fg(p.text))];
            for b in badges {
                session_spans.push(Span::raw(" "));
                session_spans.push(Span::raw("["));
                session_spans.push(b);
                session_spans.push(Span::raw("]"));
            }

            let mut row_style = Style::default().fg(p.text).bg(p.panel);
            if r.override_effort.is_some() || r.override_config_name.is_some() {
                row_style = row_style.add_modifier(Modifier::ITALIC);
            }

            Row::new(vec![
                Cell::from(Line::from(session_spans)),
                Cell::from(cwd),
                Cell::from(Line::from(vec![active])),
                Cell::from(Line::from(vec![last])),
                Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                Cell::from(Line::from(vec![tok])),
            ])
            .style(row_style)
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        rows,
        [
            Constraint::Length(18),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_spacing(HighlightSpacing::Always);

    f.render_stateful_widget(table, area, &mut ui.sessions_table);

    if snapshot.rows.len() > 8 {
        let mut scrollbar =
            ScrollbarState::new(snapshot.rows.len()).position(ui.sessions_table.offset());
        let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(p.border));
        f.render_stateful_widget(sb, area, &mut scrollbar);
    }
}

fn render_details_and_requests(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(0)])
        .split(area);

    render_session_details(f, p, ui, snapshot, chunks[0]);
    render_requests_panel(f, p, ui, snapshot, providers, chunks[1]);
}

fn render_session_details(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let selected = snapshot.rows.get(ui.selected_session_idx);
    let sid = selected
        .and_then(|r| r.session_id.as_deref())
        .unwrap_or("-");
    let cwd = selected
        .and_then(|r| r.cwd.as_deref())
        .map(|s| shorten_middle(s, 64))
        .unwrap_or_else(|| "-".to_string());

    let override_effort = selected
        .and_then(|r| r.override_effort.as_deref())
        .unwrap_or("-");
    let override_cfg = selected
        .and_then(|r| r.override_config_name.as_deref())
        .unwrap_or("-");
    let model = selected
        .and_then(|r| r.last_model.as_deref())
        .unwrap_or("-");
    let provider = selected
        .and_then(|r| r.last_provider_id.as_deref())
        .unwrap_or("-");
    let cfg = selected
        .and_then(|r| r.last_config_name.as_deref())
        .unwrap_or("-");
    let effort = selected
        .and_then(|r| r.override_effort.as_deref())
        .or_else(|| selected.and_then(|r| r.last_reasoning_effort.as_deref()))
        .unwrap_or("-");

    let now = now_ms();
    let active_age = if selected.map(|r| r.active_count).unwrap_or(0) > 0 {
        format_age(now, selected.and_then(|r| r.active_started_at_ms_min))
    } else {
        "-".to_string()
    };
    let last_age = format_age(now, selected.and_then(|r| r.last_ended_at_ms));
    let last_status = selected.and_then(|r| r.last_status);
    let last_dur = selected
        .and_then(|r| r.last_duration_ms)
        .map(|d| format!("{d}ms"))
        .unwrap_or_else(|| "-".to_string());

    let turns_total = selected.and_then(|r| r.turns_total).unwrap_or(0);
    let turns_with_usage = selected.and_then(|r| r.turns_with_usage).unwrap_or(0);

    let last_usage = selected
        .and_then(|r| r.last_usage.as_ref())
        .map(usage_line)
        .unwrap_or_else(|| "tok in/out/rsn/ttl: -".to_string());

    let total_usage = selected
        .and_then(|r| r.total_usage.as_ref())
        .filter(|u| u.total_tokens > 0)
        .map(usage_line)
        .unwrap_or_else(|| "tok in/out/rsn/ttl: -".to_string());

    let lines = vec![
        kv_line(
            p,
            "session",
            short_sid(sid, 24),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
        kv_line(p, "cwd", cwd, Style::default().fg(p.text)),
        kv_line(p, "model", model.to_string(), Style::default().fg(p.text)),
        kv_line(
            p,
            "provider",
            provider.to_string(),
            Style::default().fg(p.text),
        ),
        kv_line(p, "config", cfg.to_string(), Style::default().fg(p.text)),
        kv_line(
            p,
            "effort",
            effort.to_string(),
            Style::default().fg(if override_effort != "-" {
                p.accent
            } else {
                p.text
            }),
        ),
        kv_line(
            p,
            "override",
            format!("effort={override_effort}, cfg={override_cfg}"),
            Style::default().fg(if override_effort != "-" || override_cfg != "-" {
                p.accent
            } else {
                p.muted
            }),
        ),
        kv_line(
            p,
            "activity",
            format!(
                "active_age={active_age}, last_age={last_age}, last_status={}, last_dur={last_dur}",
                last_status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "-".to_string())
            ),
            status_style(p, last_status),
        ),
        kv_line(
            p,
            "usage",
            format!("{last_usage} | sum {total_usage} | turns {turns_total}/{turns_with_usage}"),
            Style::default().fg(p.muted),
        ),
    ];

    let title = Span::styled(
        "Details",
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
        .wrap(Wrap { trim: true });
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
    let title = Span::styled(
        "Requests",
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { p.focus } else { p.border }))
        .style(Style::default().bg(p.panel));

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

    let header = Row::new(vec![
        Cell::from(Span::styled("Age", Style::default().fg(p.muted))),
        Cell::from(Span::styled("St", Style::default().fg(p.muted))),
        Cell::from(Span::styled("Method", Style::default().fg(p.muted))),
        Cell::from(Span::styled("Path", Style::default().fg(p.muted))),
        Cell::from(Span::styled("Dur", Style::default().fg(p.muted))),
        Cell::from(Span::styled("Tok", Style::default().fg(p.muted))),
    ]);

    let now = now_ms();
    let rows = filtered
        .iter()
        .map(|r| {
            let age = format_age(now, Some(r.ended_at_ms));
            let status = Span::styled(
                r.status_code.to_string(),
                status_style(p, Some(r.status_code)),
            );
            let method = Span::styled(r.method.clone(), Style::default().fg(p.muted));
            let path = shorten_middle(&r.path, 48);
            let dur = format!("{}ms", r.duration_ms);
            let tok = r
                .usage
                .as_ref()
                .map(|u| tokens_short(u.total_tokens))
                .unwrap_or_else(|| "-".to_string());

            Row::new(vec![
                Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                Cell::from(Line::from(vec![status])),
                Cell::from(Line::from(vec![method])),
                Cell::from(path),
                Cell::from(Span::styled(dur, Style::default().fg(p.muted))),
                Cell::from(Span::styled(tok, Style::default().fg(p.muted))),
            ])
            .style(Style::default().bg(p.panel).fg(p.text))
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(8),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_spacing(HighlightSpacing::Always);

    f.render_stateful_widget(table, area, &mut ui.requests_table);

    if filtered.len() > 8 {
        let mut scrollbar =
            ScrollbarState::new(filtered.len()).position(ui.requests_table.offset());
        let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(p.border));
        f.render_stateful_widget(sb, area, &mut scrollbar);
    }
}
