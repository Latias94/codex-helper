use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::tui::model::{
    Palette, Snapshot, format_age, now_ms, shorten, shorten_middle, status_style, usage_line,
};
use crate::tui::state::UiState;

pub(super) fn render_requests_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    let selected_sid = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|r| r.session_id.as_deref());

    let filtered = snapshot
        .recent
        .iter()
        .filter(|r| {
            if ui.request_page_errors_only && r.status_code < 400 {
                return false;
            }
            if ui.request_page_scope_session {
                match (selected_sid, r.session_id.as_deref()) {
                    (Some(sid), Some(rid)) => sid == rid,
                    (Some(_), None) => false,
                    (None, _) => true,
                }
            } else {
                true
            }
        })
        .collect::<Vec<_>>();

    if filtered.is_empty() {
        ui.selected_request_page_idx = 0;
        ui.request_page_table.select(None);
    } else {
        ui.selected_request_page_idx = ui.selected_request_page_idx.min(filtered.len() - 1);
        ui.request_page_table
            .select(Some(ui.selected_request_page_idx));
    }

    let left_title = format!(
        "Requests  (scope: {}, errors_only: {})",
        if ui.request_page_scope_session {
            "session"
        } else {
            "all"
        },
        if ui.request_page_errors_only {
            "on"
        } else {
            "off"
        }
    );
    let left_block = Block::default()
        .title(Span::styled(
            left_title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new(["Age", "St", "Dur", "Att", "Model", "Cfg", "Pid", "Path"])
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
            let dur = format!("{}ms", r.duration_ms);
            let attempts_n = r.retry.as_ref().map(|x| x.attempts).unwrap_or(1);
            let attempts = attempts_n.to_string();
            let model = r.model.as_deref().unwrap_or("-").to_string();
            let cfg = r.config_name.as_deref().unwrap_or("-").to_string();
            let pid = r.provider_id.as_deref().unwrap_or("-").to_string();
            let path = shorten_middle(&r.path, 60);

            Row::new(vec![
                Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                Cell::from(Line::from(vec![status])),
                Cell::from(Span::styled(dur, Style::default().fg(p.muted))),
                Cell::from(Span::styled(
                    attempts,
                    Style::default().fg(if attempts_n > 1 { p.warn } else { p.muted }),
                )),
                Cell::from(shorten(&model, 18)),
                Cell::from(shorten(&cfg, 14)),
                Cell::from(shorten(&pid, 10)),
                Cell::from(path),
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
            Constraint::Length(4),
            Constraint::Length(18),
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(left_block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, columns[0], &mut ui.request_page_table);

    let selected = filtered.get(ui.selected_request_page_idx);
    let mut lines = Vec::new();
    if let Some(r) = selected {
        lines.push(Line::from(vec![
            Span::styled("status: ", Style::default().fg(p.muted)),
            Span::styled(
                r.status_code.to_string(),
                status_style(p, Some(r.status_code)),
            ),
            Span::raw("  "),
            Span::styled("dur: ", Style::default().fg(p.muted)),
            Span::styled(format!("{}ms", r.duration_ms), Style::default().fg(p.muted)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("method: ", Style::default().fg(p.muted)),
            Span::styled(r.method.clone(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("path: ", Style::default().fg(p.muted)),
            Span::styled(shorten_middle(&r.path, 80), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("model: ", Style::default().fg(p.muted)),
            Span::styled(
                r.model.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("config: ", Style::default().fg(p.muted)),
            Span::styled(
                r.config_name.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.accent),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("provider: ", Style::default().fg(p.muted)),
            Span::styled(
                r.provider_id.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.text),
            ),
        ]));
        if let Some(u) = r.upstream_base_url.as_deref() {
            lines.push(Line::from(vec![
                Span::styled("upstream: ", Style::default().fg(p.muted)),
                Span::styled(shorten_middle(u, 80), Style::default().fg(p.text)),
            ]));
        }

        if let Some(ttfb_ms) = r.ttfb_ms.filter(|v| *v > 0) {
            lines.push(Line::from(vec![
                Span::styled("ttfb: ", Style::default().fg(p.muted)),
                Span::styled(format!("{ttfb_ms}ms"), Style::default().fg(p.text)),
            ]));
        }

        if let Some(u) = r.usage.as_ref().filter(|u| u.total_tokens > 0) {
            lines.push(Line::from(vec![
                Span::styled("usage: ", Style::default().fg(p.muted)),
                Span::styled(usage_line(u), Style::default().fg(p.accent)),
            ]));

            let ttfb_ms = r.ttfb_ms.unwrap_or(0);
            let gen_ms = if ttfb_ms > 0 && ttfb_ms < r.duration_ms {
                r.duration_ms.saturating_sub(ttfb_ms)
            } else {
                r.duration_ms
            };
            let out_tok_s = if gen_ms > 0 && u.output_tokens > 0 {
                Some((u.output_tokens as f64) / (gen_ms as f64 / 1000.0))
            } else {
                None
            };
            if let Some(rate) = out_tok_s.filter(|v| v.is_finite() && *v > 0.0) {
                lines.push(Line::from(vec![
                    Span::styled("out_tok/s: ", Style::default().fg(p.muted)),
                    Span::styled(format!("{rate:.1}"), Style::default().fg(p.text)),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Retry / route chain",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if let Some(retry) = r.retry.as_ref() {
            lines.push(Line::from(vec![
                Span::styled("attempts: ", Style::default().fg(p.muted)),
                Span::styled(retry.attempts.to_string(), Style::default().fg(p.text)),
            ]));
            let max = 12usize;
            for (idx, entry) in retry.upstream_chain.iter().take(max).enumerate() {
                lines.push(Line::from(vec![
                    Span::styled(format!("{:>2}. ", idx + 1), Style::default().fg(p.muted)),
                    Span::styled(shorten_middle(entry, 120), Style::default().fg(p.muted)),
                ]));
            }
            if retry.upstream_chain.len() > max {
                lines.push(Line::from(Span::styled(
                    format!("… +{} more", retry.upstream_chain.len() - max),
                    Style::default().fg(p.muted),
                )));
            }
        } else {
            lines.push(Line::from(Span::styled(
                "(no retries)",
                Style::default().fg(p.muted),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "No requests match the current filters.",
            Style::default().fg(p.muted),
        )));
    }

    let right_block = Block::default()
        .title(Span::styled(
            "Details",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));
    let content = Paragraph::new(Text::from(lines))
        .block(right_block)
        .style(Style::default().fg(p.text))
        .wrap(Wrap { trim: false });
    f.render_widget(content, columns[1]);
}
