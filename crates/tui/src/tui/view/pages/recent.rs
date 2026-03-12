use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Tabs, Wrap};

use crate::tui::model::{
    CODEX_RECENT_WINDOWS, Palette, codex_recent_window_label, codex_recent_window_threshold_ms,
    format_age, now_ms, shorten_middle,
};
use crate::tui::state::UiState;

pub(super) fn render_recent_page(f: &mut Frame<'_>, p: Palette, ui: &mut UiState, area: Rect) {
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
        "{}  (window: {}, raw_cwd: {})",
        crate::tui::i18n::pick(ui.language, "最近会话 (Codex)", "Recent sessions (Codex)"),
        codex_recent_window_label(ui.codex_recent_window_idx),
        if ui.codex_recent_raw_cwd { "on" } else { "off" }
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
            crate::tui::i18n::pick(ui.language, "详情", "Details"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    if let Some(err) = ui.codex_recent_error.as_deref() {
        lines.push(Line::from(Span::styled(
            format!("error: {err}"),
            Style::default().fg(p.bad),
        )));
        lines.push(Line::from(""));
    }

    if ui.codex_recent_rows.is_empty() {
        lines.push(Line::from(Span::styled(
            crate::tui::i18n::pick(
                ui.language,
                "未加载最近会话。按 r 刷新；或确认 ~/.codex/sessions 存在。",
                "No recent sessions loaded. Press r to refresh; or check ~/.codex/sessions.",
            ),
            Style::default().fg(p.muted),
        )));
    } else if let Some(r) = visible.get(ui.codex_recent_selected_idx) {
        let branch = r.branch.as_deref().unwrap_or("-");
        lines.push(Line::from(vec![
            Span::styled("root: ", Style::default().fg(p.muted)),
            Span::styled(r.root.clone(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("branch: ", Style::default().fg(p.muted)),
            Span::styled(branch.to_string(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("sid: ", Style::default().fg(p.muted)),
            Span::styled(r.session_id.clone(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("mtime: ", Style::default().fg(p.muted)),
            Span::styled(
                format_age(now, Some(r.mtime_ms)),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("cwd: ", Style::default().fg(p.muted)),
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
            Span::styled("copy: ", Style::default().fg(p.muted)),
            Span::styled(
                format!("{} {}", r.root, r.session_id),
                Style::default().fg(p.accent),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            crate::tui::i18n::pick(ui.language, "按键", "Keys"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(crate::tui::i18n::pick(
            ui.language,
            "  Enter 复制条目  y 复制可见列表  t 打开 transcript",
            "  Enter copy selected  y copy visible list  t open transcript",
        )));
        lines.push(Line::from(crate::tui::i18n::pick(
            ui.language,
            "  s 打开到 Sessions  f 打开到 Requests  h 打开到 History",
            "  s open in Sessions  f open in Requests  h open in History",
        )));
    } else {
        lines.push(Line::from(Span::styled(
            crate::tui::i18n::pick(ui.language, "未选中任何条目。", "No selection."),
            Style::default().fg(p.muted),
        )));
    }

    let content = Paragraph::new(Text::from(lines))
        .block(right_block)
        .style(Style::default().fg(p.text))
        .wrap(Wrap { trim: false });
    f.render_widget(content, columns[1]);
}
