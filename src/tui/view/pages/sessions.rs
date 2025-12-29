use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::tui::model::{
    Palette, Snapshot, basename, format_age, now_ms, short_sid, shorten, shorten_middle, status_style,
    tokens_short, usage_line,
};
use crate::tui::state::UiState;
use crate::tui::view::widgets::kv_line;

pub(super) fn render_sessions_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let filtered = snapshot
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            if ui.sessions_page_active_only && row.active_count == 0 {
                return false;
            }
            if ui.sessions_page_errors_only && row.last_status.is_some_and(|s| s < 400) {
                return false;
            }
            if ui.sessions_page_overrides_only
                && row.override_effort.is_none()
                && row.override_config_name.is_none()
            {
                return false;
            }
            true
        })
        .take(200)
        .collect::<Vec<_>>();

    let selected_idx_in_filtered = ui
        .selected_session_id
        .as_deref()
        .and_then(|sid| {
            filtered
                .iter()
                .position(|(_, row)| row.session_id.as_deref() == Some(sid))
        })
        .unwrap_or(
            ui.selected_sessions_page_idx
                .min(filtered.len().saturating_sub(1)),
        );

    ui.selected_sessions_page_idx = selected_idx_in_filtered;
    if filtered.is_empty() {
        ui.sessions_page_table.select(None);
    } else {
        ui.sessions_page_table
            .select(Some(ui.selected_sessions_page_idx));
    }

    let title = format!(
        "Sessions  (active_only: {}, errors_only: {}, overrides_only: {})",
        if ui.sessions_page_active_only {
            "on"
        } else {
            "off"
        },
        if ui.sessions_page_errors_only {
            "on"
        } else {
            "off"
        },
        if ui.sessions_page_overrides_only {
            "on"
        } else {
            "off"
        }
    );
    let left_block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new(["Session", "CWD", "A", "St", "Last", "Turns", "Tok", "Pin"])
        .style(Style::default().fg(p.muted))
        .height(1);

    let now = now_ms();
    let rows = filtered
        .iter()
        .map(|(_, row)| {
            let sid = row
                .session_id
                .as_deref()
                .map(|s| short_sid(s, 16))
                .unwrap_or_else(|| "-".to_string());
            let cwd = row
                .cwd
                .as_deref()
                .map(|s| shorten(basename(s), 16))
                .unwrap_or_else(|| "-".to_string());
            let active = row.active_count.to_string();
            let status = row
                .last_status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".to_string());
            let last = format_age(now, row.last_ended_at_ms);
            let turns = row.turns_total.unwrap_or(0).to_string();
            let tok = row
                .total_usage
                .as_ref()
                .map(|u| tokens_short(u.total_tokens))
                .unwrap_or_else(|| "-".to_string());
            let pin = row
                .override_config_name
                .as_deref()
                .map(|s| shorten(s, 12))
                .unwrap_or_else(|| "-".to_string());

            let mut style = Style::default().fg(p.text);
            if row.last_status.is_some_and(|s| s >= 500) {
                style = style.fg(p.bad);
            } else if row.last_status.is_some_and(|s| s >= 400) {
                style = style.fg(p.warn);
            }
            if row.override_effort.is_some() || row.override_config_name.is_some() {
                style = style.add_modifier(Modifier::BOLD);
            }

            Row::new(vec![
                Cell::from(sid),
                Cell::from(Span::styled(cwd, Style::default().fg(p.muted))),
                Cell::from(Span::styled(
                    active,
                    Style::default().fg(if row.active_count > 0 {
                        p.good
                    } else {
                        p.muted
                    }),
                )),
                Cell::from(Span::styled(status, status_style(p, row.last_status))),
                Cell::from(Span::styled(last, Style::default().fg(p.muted))),
                Cell::from(Span::styled(turns, Style::default().fg(p.muted))),
                Cell::from(Span::styled(tok, Style::default().fg(p.muted))),
                Cell::from(Span::styled(
                    pin,
                    Style::default().fg(if row.override_config_name.is_some() {
                        p.accent
                    } else {
                        p.muted
                    }),
                )),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        rows,
        [
            Constraint::Length(18),
            Constraint::Length(18),
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Min(8),
        ],
    )
    .header(header)
    .block(left_block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, columns[0], &mut ui.sessions_page_table);

    let selected = filtered
        .get(ui.selected_sessions_page_idx)
        .map(|(_, row)| *row);
    let mut lines = Vec::new();
    if let Some(row) = selected {
        let sid_full = row.session_id.as_deref().unwrap_or("-");
        let cwd_full = row
            .cwd
            .as_deref()
            .map(|s| shorten_middle(s, 80))
            .unwrap_or_else(|| "-".to_string());
        let model = row.last_model.as_deref().unwrap_or("-");
        let provider = row.last_provider_id.as_deref().unwrap_or("-");
        let cfg = row.last_config_name.as_deref().unwrap_or("-");
        let effort = row
            .override_effort
            .as_deref()
            .or(row.last_reasoning_effort.as_deref())
            .unwrap_or("-");
        let override_effort = row.override_effort.as_deref().unwrap_or("-");
        let override_cfg = row.override_config_name.as_deref().unwrap_or("-");
        let global_cfg = snapshot.global_override.as_deref().unwrap_or("-");
        let routing = if override_cfg != "-" {
            format!("pinned(session)={override_cfg}")
        } else if global_cfg != "-" {
            format!("pinned(global)={global_cfg}")
        } else {
            "auto".to_string()
        };

        lines.push(kv_line(
            p,
            "session",
            short_sid(sid_full, 28),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ));
        lines.push(kv_line(p, "cwd", cwd_full, Style::default().fg(p.text)));
        lines.push(kv_line(
            p,
            "model",
            model.to_string(),
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            "provider",
            provider.to_string(),
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            "config",
            cfg.to_string(),
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            "effort",
            effort.to_string(),
            Style::default().fg(if override_effort != "-" {
                p.accent
            } else {
                p.text
            }),
        ));
        lines.push(kv_line(
            p,
            "override",
            format!("effort={override_effort}, cfg={override_cfg}, global={global_cfg}"),
            Style::default().fg(if override_effort != "-" || override_cfg != "-" {
                p.accent
            } else {
                p.muted
            }),
        ));
        lines.push(kv_line(p, "routing", routing, Style::default().fg(p.muted)));

        let last_status = row
            .last_status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());
        let last_dur = row
            .last_duration_ms
            .map(|d| format!("{d}ms"))
            .unwrap_or_else(|| "-".to_string());
        let active_age = if row.active_count > 0 {
            format_age(now, row.active_started_at_ms_min)
        } else {
            "-".to_string()
        };
        let last_age = format_age(now, row.last_ended_at_ms);
        lines.push(kv_line(
            p,
            "activity",
            format!(
                "active={} (age={active_age})  last_status={last_status} last_dur={last_dur} last_age={last_age}",
                row.active_count
            ),
            status_style(p, row.last_status),
        ));

        let turns_total = row.turns_total.unwrap_or(0);
        let turns_with_usage = row.turns_with_usage.unwrap_or(0);
        let total_usage = row
            .total_usage
            .as_ref()
            .filter(|u| u.total_tokens > 0)
            .map(usage_line)
            .unwrap_or_else(|| "tok in/out/rsn/ttl: -".to_string());
        lines.push(kv_line(
            p,
            "usage",
            format!("{total_usage} | turns {turns_total}/{turns_with_usage}"),
            Style::default().fg(p.muted),
        ));

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Keys",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from("  a toggle active-only"));
        lines.push(Line::from("  e toggle errors-only"));
        lines.push(Line::from("  v toggle overrides-only"));
        lines.push(Line::from("  r reset filters"));
        lines.push(Line::from("  Enter effort menu  p/P provider override"));
    } else {
        lines.push(Line::from(Span::styled(
            "No sessions match the current filters.",
            Style::default().fg(p.muted),
        )));
    }

    let right_block = Block::default()
        .title(Span::styled(
            "Session details",
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
