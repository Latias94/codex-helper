use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Tabs, Wrap};

use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    CODEX_RECENT_WINDOWS, Palette, codex_recent_window_label, codex_recent_window_threshold_ms,
    format_age, now_ms, shorten_middle,
};
use crate::tui::state::UiState;

pub(super) fn render_recent_page(f: &mut Frame<'_>, p: Palette, ui: &mut UiState, area: Rect) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    let now = now_ms();
    let threshold_ms = codex_recent_window_threshold_ms(now, ui.codex_recent_window_idx);
    let visible = ui
        .codex_recent_rows
        .iter()
        .filter(|r| r.mtime_ms >= threshold_ms)
        .take(300)
        .collect::<Vec<_>>();

    let selected_idx_in_visible = ui
        .codex_recent_selected_id
        .as_deref()
        .and_then(|sid| visible.iter().position(|r| r.session_id.as_str() == sid))
        .unwrap_or(ui.codex_recent_selected_idx)
        .min(visible.len().saturating_sub(1));
    ui.codex_recent_selected_idx = selected_idx_in_visible;
    ui.codex_recent_selected_id = visible
        .get(ui.codex_recent_selected_idx)
        .map(|r| r.session_id.clone());
    if visible.is_empty() {
        ui.codex_recent_table.select(None);
    } else {
        ui.codex_recent_table
            .select(Some(ui.codex_recent_selected_idx));
    }

    let title = format!(
        "{}  ({}: {}, {}: {})",
        i18n::text(lang, msg::RECENT_TITLE),
        l("window"),
        codex_recent_window_label(ui.codex_recent_window_idx),
        l("raw_cwd"),
        if ui.codex_recent_raw_cwd {
            l("on")
        } else {
            l("off")
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

    let left_inner = left_block.inner(columns[0]);
    f.render_widget(left_block, columns[0]);

    let inner_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(left_inner);

    let tabs = Tabs::new(
        CODEX_RECENT_WINDOWS
            .iter()
            .map(|(_, label)| Line::from(Span::raw(*label)))
            .collect::<Vec<_>>(),
    )
    .select(
        ui.codex_recent_window_idx
            .min(CODEX_RECENT_WINDOWS.len().saturating_sub(1)),
    )
    .style(Style::default().fg(p.muted))
    .highlight_style(Style::default().fg(p.text).add_modifier(Modifier::BOLD))
    .divider(Span::raw("  "));
    f.render_widget(tabs, inner_chunks[0]);

    let rows = visible
        .iter()
        .map(|r| {
            let root = shorten_middle(r.root.as_str(), 120);
            let branch = r.branch.as_deref().unwrap_or("-");
            let age = format_age(now, Some(r.mtime_ms));

            let line1 = Line::from(vec![
                Span::styled(root, Style::default().fg(p.text)),
                Span::raw("  "),
                Span::styled(
                    format!("[{branch}]"),
                    Style::default().fg(if r.branch.is_some() {
                        p.accent
                    } else {
                        p.muted
                    }),
                ),
            ]);
            let line2 = Line::from(vec![
                Span::styled(r.session_id.clone(), Style::default().fg(p.text)),
                Span::raw("  "),
                Span::styled(age, Style::default().fg(p.muted)),
            ]);
            let text = Text::from(vec![line1, line2]);
            Row::new(vec![Cell::from(text)]).height(2)
        })
        .collect::<Vec<_>>();

    let table = Table::new(rows, [Constraint::Min(10)])
        .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
        .highlight_symbol("  ")
        .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, inner_chunks[1], &mut ui.codex_recent_table);

    let right_block = Block::default()
        .title(Span::styled(
            i18n::text(ui.language, msg::DETAILS_TITLE),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    if ui.codex_recent_loading && !ui.codex_recent_rows.is_empty() {
        lines.push(Line::from(Span::styled(
            i18n::text(ui.language, msg::RECENT_REFRESHING),
            Style::default().fg(p.accent),
        )));
        lines.push(Line::from(""));
    }
    if let Some(err) = ui.codex_recent_error.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("{}: {err}", l("error")),
            Style::default().fg(p.bad),
        )));
        lines.push(Line::from(""));
    }

    if ui.codex_recent_rows.is_empty() {
        lines.push(Line::from(Span::styled(
            if ui.codex_recent_loading {
                i18n::text(ui.language, msg::RECENT_REFRESHING)
            } else {
                i18n::text(ui.language, msg::RECENT_EMPTY)
            },
            Style::default().fg(p.muted),
        )));
    } else if let Some(r) = visible.get(ui.codex_recent_selected_idx) {
        let branch = r.branch.as_deref().unwrap_or("-");
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("root")), Style::default().fg(p.muted)),
            Span::styled(r.root.clone(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("branch")), Style::default().fg(p.muted)),
            Span::styled(branch.to_string(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("sid")), Style::default().fg(p.muted)),
            Span::styled(r.session_id.clone(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("mtime")), Style::default().fg(p.muted)),
            Span::styled(
                format_age(now, Some(r.mtime_ms)),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("cwd")), Style::default().fg(p.muted)),
            Span::styled(
                r.cwd
                    .as_deref()
                    .map(|v| shorten_middle(v, 120))
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("copy")), Style::default().fg(p.muted)),
            Span::styled(
                format!("{} {}", r.root, r.session_id),
                Style::default().fg(p.accent),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            i18n::text(ui.language, msg::KEYS_LABEL),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(i18n::text(
            ui.language,
            msg::RECENT_KEYS_PRIMARY,
        )));
        lines.push(Line::from(i18n::text(ui.language, msg::RECENT_KEYS_NAV)));
    } else {
        lines.push(Line::from(Span::styled(
            i18n::text(ui.language, msg::NO_SELECTION),
            Style::default().fg(p.muted),
        )));
    }

    let content = Paragraph::new(Text::from(lines))
        .block(right_block)
        .style(Style::default().fg(p.text))
        .wrap(Wrap { trim: false });
    f.render_widget(content, columns[1]);
}
