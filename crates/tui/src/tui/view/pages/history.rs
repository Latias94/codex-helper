use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::sessions::SessionSummarySource;
use crate::tui::model::{Palette, basename, short_sid, shorten, shorten_middle};
use crate::tui::state::{CodexHistoryExternalFocusOrigin, UiState};

pub(super) fn render_history_page(f: &mut Frame<'_>, p: Palette, ui: &mut UiState, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    let title = crate::tui::i18n::pick(ui.language, "历史会话 (Codex)", "History sessions (Codex)");
    let left_block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new(["Updated", "Session", "CWD", "Rounds", "First user message"])
        .style(Style::default().fg(p.muted))
        .height(1);

    ui.sync_codex_history_selection();

    let rows = ui
        .codex_history_sessions
        .iter()
        .take(300)
        .map(|s| {
            let updated = s
                .updated_at
                .as_deref()
                .or(s.created_at.as_deref())
                .map(|t| shorten_middle(t, 20))
                .unwrap_or_else(|| "-".to_string());
            let sid = short_sid(s.id.as_str(), 14);
            let cwd = s
                .cwd
                .as_deref()
                .map(|v| shorten(basename(v), 16))
                .unwrap_or_else(|| "-".to_string());
            let rounds = s.rounds.to_string();
            let msg = s
                .first_user_message
                .as_deref()
                .map(|m| shorten_middle(m, 80))
                .unwrap_or_else(|| "-".to_string());

            Row::new(vec![
                Cell::from(Span::styled(updated, Style::default().fg(p.muted))),
                Cell::from(sid),
                Cell::from(Span::styled(cwd, Style::default().fg(p.muted))),
                Cell::from(Span::styled(rounds, Style::default().fg(p.muted))),
                Cell::from(msg),
            ])
            .style(Style::default().fg(p.text))
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        rows,
        [
            Constraint::Length(20),
            Constraint::Length(16),
            Constraint::Length(18),
            Constraint::Length(6),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(left_block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, columns[0], &mut ui.codex_history_table);

    let right_block = Block::default()
        .title(Span::styled(
            crate::tui::i18n::pick(ui.language, "详情", "Details"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    if let Some(err) = ui.codex_history_error.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("error: {err}"),
            Style::default().fg(p.bad),
        )));
        lines.push(Line::from(""));
    }

    if ui.codex_history_sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            crate::tui::i18n::pick(
                ui.language,
                "未找到历史会话。按 r 刷新；或确认 ~/.codex/sessions 存在。",
                "No history sessions found. Press r to refresh; or check ~/.codex/sessions.",
            ),
            Style::default().fg(p.muted),
        )));
    } else if let Some(s) = ui.codex_history_sessions.get(ui.selected_codex_history_idx) {
        if let Some(focus) = ui
            .codex_history_external_focus
            .as_ref()
            .filter(|focus| focus.summary.id == s.id)
        {
            lines.push(Line::from(vec![
                Span::styled("context: ", Style::default().fg(p.muted)),
                Span::styled(
                    format!("focused from {}", history_focus_origin_label(focus.origin)),
                    Style::default().fg(p.accent),
                ),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("sid: ", Style::default().fg(p.muted)),
            Span::styled(s.id.clone(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("cwd: ", Style::default().fg(p.muted)),
            Span::styled(
                s.cwd
                    .as_deref()
                    .map(|v| shorten_middle(v, 80))
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("updated: ", Style::default().fg(p.muted)),
            Span::styled(
                s.updated_at
                    .as_deref()
                    .or(s.created_at.as_deref())
                    .map(|v| shorten_middle(v, 80))
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("turns: ", Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "user={} assistant={} rounds={}",
                    s.user_turns, s.assistant_turns, s.rounds
                ),
                Style::default().fg(p.muted),
            ),
        ]));
        if let Some(ts) = s.last_response_at.as_deref() {
            lines.push(Line::from(vec![
                Span::styled("last_response: ", Style::default().fg(p.muted)),
                Span::styled(shorten_middle(ts, 80), Style::default().fg(p.muted)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("source: ", Style::default().fg(p.muted)),
            Span::styled(
                match s.source {
                    SessionSummarySource::LocalFile => "local transcript",
                    SessionSummarySource::ObservedOnly => "observed bridge",
                },
                Style::default().fg(if s.source == SessionSummarySource::LocalFile {
                    p.text
                } else {
                    p.warn
                }),
            ),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            crate::tui::i18n::pick(ui.language, "首条用户消息", "First user message"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )));
        if let Some(msg) = s.first_user_message.as_deref() {
            for line in msg.lines() {
                lines.push(Line::from(Span::raw(format!("  {line}"))));
            }
        } else {
            lines.push(Line::from(Span::styled(
                crate::tui::i18n::pick(ui.language, "  -", "  -"),
                Style::default().fg(p.muted),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            crate::tui::i18n::pick(ui.language, "按键", "Keys"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(crate::tui::i18n::pick(
            ui.language,
            "  ↑/↓ 选择  r 刷新  t/Enter 打开对话记录  s 打开到 Sessions  f 打开到 Requests",
            "  ↑/↓ select  r refresh  t/Enter open transcript  s open in Sessions  f open in Requests",
        )));
        if s.source == SessionSummarySource::ObservedOnly {
            lines.push(Line::from(crate::tui::i18n::pick(
                ui.language,
                "  当前条目来自外部桥接，没有本地 transcript 文件。",
                "  This entry comes from an external bridge and may not have a local transcript file.",
            )));
        }
    }

    let content = Paragraph::new(Text::from(lines))
        .block(right_block)
        .style(Style::default().fg(p.text))
        .wrap(Wrap { trim: false });
    f.render_widget(content, columns[1]);
}

fn history_focus_origin_label(origin: CodexHistoryExternalFocusOrigin) -> &'static str {
    match origin {
        CodexHistoryExternalFocusOrigin::Sessions => "Sessions",
        CodexHistoryExternalFocusOrigin::Requests => "Requests",
        CodexHistoryExternalFocusOrigin::Recent => "Recent",
    }
}
