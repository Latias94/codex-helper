use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::sessions::SessionSummarySource;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{Palette, basename, short_sid, shorten, shorten_middle};
use crate::tui::state::{CodexHistoryExternalFocusOrigin, UiState};

pub(super) fn render_history_page(f: &mut Frame<'_>, p: Palette, ui: &mut UiState, area: Rect) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    let title = i18n::text(lang, msg::HISTORY_TITLE);
    let left_block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new([
        l("Updated"),
        l("Session"),
        l("CWD"),
        l("Rounds"),
        l("First user message"),
    ])
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
            let sid = short_sid(s.id.as_str(), 18);
            let cwd = s
                .cwd
                .as_deref()
                .map(|v| shorten(basename(v), 14))
                .unwrap_or_else(|| "-".to_string());
            let rounds = s.rounds.to_string();
            let msg = s
                .first_user_message
                .as_deref()
                .map(history_message_preview)
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
            Constraint::Length(18),
            Constraint::Length(16),
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
            i18n::text(ui.language, msg::DETAILS_TITLE),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    if ui.codex_history_loading && !ui.codex_history_sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            i18n::text(ui.language, msg::HISTORY_REFRESHING),
            Style::default().fg(p.accent),
        )));
        lines.push(Line::from(""));
    }
    if let Some(err) = ui.codex_history_error.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("{}: {err}", l("error")),
            Style::default().fg(p.bad),
        )));
        lines.push(Line::from(""));
    }

    if ui.codex_history_sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            if ui.codex_history_loading {
                i18n::text(ui.language, msg::HISTORY_REFRESHING)
            } else {
                i18n::text(ui.language, msg::HISTORY_EMPTY)
            },
            Style::default().fg(p.muted),
        )));
    } else if let Some(s) = ui.codex_history_sessions.get(ui.selected_codex_history_idx) {
        if let Some(focus) = ui
            .codex_history_external_focus
            .as_ref()
            .filter(|focus| focus.summary.id == s.id)
        {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("context")), Style::default().fg(p.muted)),
                Span::styled(
                    format!(
                        "{} {}",
                        l("focused from"),
                        history_focus_origin_label(focus.origin, lang)
                    ),
                    Style::default().fg(p.accent),
                ),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("sid")), Style::default().fg(p.muted)),
            Span::styled(s.id.clone(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("cwd")), Style::default().fg(p.muted)),
            Span::styled(
                s.cwd
                    .as_deref()
                    .map(|v| shorten_middle(v, 80))
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("updated")), Style::default().fg(p.muted)),
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
            Span::styled(format!("{}: ", l("turns")), Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "{}={} {}={} {}={}",
                    l("user"),
                    s.user_turns,
                    l("assistant"),
                    s.assistant_turns,
                    l("rounds"),
                    s.rounds
                ),
                Style::default().fg(p.muted),
            ),
        ]));
        if let Some(ts) = s.last_response_at.as_deref() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}: ", l("last_response")),
                    Style::default().fg(p.muted),
                ),
                Span::styled(shorten_middle(ts, 80), Style::default().fg(p.muted)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("source")), Style::default().fg(p.muted)),
            Span::styled(
                match s.source {
                    SessionSummarySource::LocalFile => l("local transcript"),
                    SessionSummarySource::ObservedOnly => l("observed bridge"),
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
            i18n::text(ui.language, msg::FIRST_USER_MESSAGE),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )));
        if let Some(msg) = s.first_user_message.as_deref() {
            for line in msg.lines() {
                lines.push(Line::from(Span::raw(format!("  {line}"))));
            }
        } else {
            lines.push(Line::from(Span::styled(
                i18n::text(ui.language, msg::BULLET_DASH),
                Style::default().fg(p.muted),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            i18n::text(ui.language, msg::KEYS_LABEL),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(i18n::text(ui.language, msg::HISTORY_KEYS)));
        if s.source == SessionSummarySource::ObservedOnly {
            lines.push(Line::from(i18n::text(
                ui.language,
                msg::HISTORY_EXTERNAL_NO_TRANSCRIPT,
            )));
        }
    }

    let content = Paragraph::new(Text::from(lines))
        .block(right_block)
        .style(Style::default().fg(p.text))
        .wrap(Wrap { trim: false });
    f.render_widget(content, columns[1]);
}

fn history_focus_origin_label(
    origin: CodexHistoryExternalFocusOrigin,
    lang: crate::tui::Language,
) -> &'static str {
    match origin {
        CodexHistoryExternalFocusOrigin::Sessions => i18n::label(lang, "Sessions"),
        CodexHistoryExternalFocusOrigin::Requests => i18n::label(lang, "Requests"),
        CodexHistoryExternalFocusOrigin::Recent => i18n::label(lang, "Recent"),
    }
}

fn history_message_preview(message: &str) -> String {
    let first_line = message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("-");
    shorten(first_line, 80)
}

#[cfg(test)]
mod tests {
    use super::history_message_preview;

    #[test]
    fn history_message_preview_preserves_opening_words() {
        let preview = history_message_preview(
            "请帮我检查这次路由策略为什么会切换到 chili，然后看看余额展示是否清楚",
        );

        assert!(preview.starts_with("请帮我检查"), "{preview}");
    }
}
