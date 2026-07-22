use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Table, Tabs, Wrap,
};

use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    CODEX_RECENT_WINDOWS, Palette, codex_recent_window_label, format_age, now_ms, shorten_middle,
};
use crate::tui::state::UiState;
use crate::tui::view::widgets::max_wrapped_vertical_scroll;

const SIDE_BY_SIDE_MIN_WIDTH: u16 = 136;

pub(super) fn render_recent_page(f: &mut Frame<'_>, p: Palette, ui: &mut UiState, area: Rect) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let (direction, constraints) = if area.width >= SIDE_BY_SIDE_MIN_WIDTH {
        (
            Direction::Horizontal,
            [Constraint::Percentage(65), Constraint::Percentage(35)],
        )
    } else {
        (
            Direction::Vertical,
            [Constraint::Percentage(55), Constraint::Percentage(45)],
        )
    };
    let columns = Layout::default()
        .direction(direction)
        .constraints(constraints)
        .split(area);

    let now = now_ms();
    let visible = ui.codex_recent_visible_indices(now);

    let title = format!(
        "{}  ({}: {}, {}: {})",
        i18n::text(
            lang,
            if ui.runtime_connection.is_remote_observer() {
                msg::RECENT_TITLE_OBSERVER_LOCAL
            } else {
                msg::RECENT_TITLE
            },
        ),
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
        .filter_map(|idx| ui.codex_recent_rows.get(*idx))
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
            format!("{}  PgUp/PgDn", i18n::text(ui.language, msg::DETAILS_TITLE)),
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
    } else if let Some(r) = visible
        .get(ui.codex_recent_selected_idx)
        .and_then(|idx| ui.codex_recent_rows.get(*idx))
    {
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
        lines.push(Line::from(
            match (ui.language, ui.can_bridge_runtime_sessions_to_local_codex()) {
                (crate::tui::Language::Zh, true) => {
                    "  s 打开到 Sessions  f 打开到 Requests  h 打开到 History"
                }
                (crate::tui::Language::En, true) => {
                    "  s open Sessions  f open Requests  h open History"
                }
                (crate::tui::Language::Zh, false) => "  h 打开到本机 History",
                (crate::tui::Language::En, false) => "  h open local History",
            },
        ));
    } else {
        lines.push(Line::from(Span::styled(
            i18n::text(ui.language, msg::NO_SELECTION),
            Style::default().fg(p.muted),
        )));
    }

    let right_inner = right_block.inner(columns[1]);
    let max_scroll = max_wrapped_vertical_scroll(&lines, right_inner.width, right_inner.height);
    ui.codex_recent_details_scroll = ui.codex_recent_details_scroll.min(max_scroll);
    let content = Paragraph::new(Text::from(lines))
        .block(right_block)
        .style(Style::default().fg(p.text))
        .scroll((ui.codex_recent_details_scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(content, columns[1]);
    if max_scroll > 0 {
        let mut scrollbar = ScrollbarState::new(usize::from(max_scroll) + 1)
            .position(usize::from(ui.codex_recent_details_scroll));
        let widget = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(p.border));
        f.render_stateful_widget(widget, columns[1], &mut scrollbar);
    }
}
