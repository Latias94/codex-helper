use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, Wrap};

use crate::tui::ProviderOption;
use crate::tui::model::{
    Palette, Snapshot, format_age, now_ms, short_sid, shorten, shorten_middle,
};
use crate::tui::state::UiState;

pub(super) fn render_configs_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    let selected_session = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|r| r.session_id.as_deref())
        .unwrap_or("-");
    let session_override = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|r| r.override_config_name.as_deref());
    let global_override = snapshot.global_override.as_deref();

    let left_block = Block::default()
        .title(Span::styled(
            format!("Configs  (session: {})", short_sid(selected_session, 20)),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new(["Lvl", "Name", "Alias", "On", "Up", "Health"])
        .style(Style::default().fg(p.muted))
        .height(1);

    let rows = providers
        .iter()
        .map(|cfg| {
            let (enabled_ovr, level_ovr) = snapshot
                .config_meta_overrides
                .get(cfg.name.as_str())
                .copied()
                .unwrap_or((None, None));
            let enabled = enabled_ovr.unwrap_or(cfg.enabled);
            let level = level_ovr.unwrap_or(cfg.level).clamp(1, 10);

            let mut name = cfg.name.clone();
            if cfg.active {
                name = format!("* {name}");
            }

            let alias = cfg
                .alias
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("-");
            let on = if enabled { "on" } else { "off" };
            let up = cfg.upstreams.len().to_string();
            let health = if let Some(st) = snapshot.health_checks.get(cfg.name.as_str())
                && !st.done
            {
                if st.cancel_requested {
                    format!("cancel {}/{}", st.completed, st.total.max(1))
                } else {
                    format!("run {}/{}", st.completed, st.total.max(1))
                }
            } else if let Some(st) = snapshot.health_checks.get(cfg.name.as_str())
                && st.done
                && st.canceled
            {
                "canceled".to_string()
            } else {
                snapshot
                    .config_health
                    .get(cfg.name.as_str())
                    .map(|h| {
                        let total = h.upstreams.len().max(1);
                        let ok = h.upstreams.iter().filter(|u| u.ok == Some(true)).count();
                        let best_ms = h
                            .upstreams
                            .iter()
                            .filter(|u| u.ok == Some(true))
                            .filter_map(|u| u.latency_ms)
                            .min();
                        if ok > 0 {
                            if let Some(ms) = best_ms {
                                format!("{ok}/{total} {ms}ms")
                            } else {
                                format!("{ok}/{total} ok")
                            }
                        } else {
                            let status = h.upstreams.iter().filter_map(|u| u.status_code).next();
                            if let Some(code) = status {
                                format!("err {code}")
                            } else {
                                "err".to_string()
                            }
                        }
                    })
                    .unwrap_or_else(|| "-".to_string())
            };

            let mut style = Style::default().fg(if enabled { p.text } else { p.muted });
            if global_override == Some(cfg.name.as_str()) {
                style = style.fg(p.accent).add_modifier(Modifier::BOLD);
            }
            if session_override == Some(cfg.name.as_str()) {
                style = style.fg(p.focus).add_modifier(Modifier::BOLD);
            }

            Row::new([
                format!("L{level}"),
                name,
                alias.to_string(),
                on.to_string(),
                up,
                health,
            ])
            .style(style)
            .height(1)
        })
        .collect::<Vec<_>>();

    ui.configs_table.select(if providers.is_empty() {
        None
    } else {
        Some(ui.selected_config_idx)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(16),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(left_block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
    .highlight_symbol("  ");
    f.render_stateful_widget(table, columns[0], &mut ui.configs_table);

    let selected = providers.get(ui.selected_config_idx);
    let right_title = selected
        .map(|c| format!("Config details: {} (L{})", c.name, c.level.clamp(1, 10)))
        .unwrap_or_else(|| "Config details".to_string());

    let mut lines = Vec::new();
    if let Some(cfg) = selected {
        let (enabled_ovr, level_ovr) = snapshot
            .config_meta_overrides
            .get(cfg.name.as_str())
            .copied()
            .unwrap_or((None, None));
        let enabled = enabled_ovr.unwrap_or(cfg.enabled);
        let level = level_ovr.unwrap_or(cfg.level).clamp(1, 10);
        let level_note = if level_ovr.is_some() {
            " (override)"
        } else {
            ""
        };
        let enabled_note = if enabled_ovr.is_some() {
            " (override)"
        } else {
            ""
        };

        if let Some(alias) = cfg.alias.as_deref()
            && !alias.trim().is_empty()
        {
            lines.push(Line::from(vec![
                Span::styled("alias: ", Style::default().fg(p.muted)),
                Span::styled(alias.to_string(), Style::default().fg(p.text)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("enabled: ", Style::default().fg(p.muted)),
            Span::styled(
                format!("{}{enabled_note}", if enabled { "true" } else { "false" }),
                Style::default().fg(if enabled { p.good } else { p.warn }),
            ),
            Span::raw("   "),
            Span::styled("level: ", Style::default().fg(p.muted)),
            Span::styled(
                format!("L{level}{level_note}"),
                Style::default().fg(p.muted),
            ),
            Span::raw("   "),
            Span::styled("active: ", Style::default().fg(p.muted)),
            Span::styled(
                if cfg.active { "true" } else { "false" },
                Style::default().fg(if cfg.active { p.accent } else { p.muted }),
            ),
        ]));

        let routing = if let Some(s) = session_override {
            format!("pinned(session)={s}")
        } else if let Some(g) = global_override {
            format!("pinned(global)={g}")
        } else {
            let mut levels = providers
                .iter()
                .filter(|c| c.enabled || c.active)
                .map(|c| c.level.clamp(1, 10))
                .collect::<Vec<_>>();
            levels.sort_unstable();
            levels.dedup();
            if levels.len() > 1 {
                "auto(level-based)".to_string()
            } else {
                "auto(active-only)".to_string()
            }
        };
        lines.push(Line::from(vec![
            Span::styled("routing: ", Style::default().fg(p.muted)),
            Span::styled(routing, Style::default().fg(p.muted)),
        ]));

        if let Some(st) = snapshot.health_checks.get(cfg.name.as_str()) {
            let status = if !st.done {
                if st.cancel_requested {
                    format!("cancel {}/{}", st.completed, st.total.max(1))
                } else {
                    format!("running {}/{}", st.completed, st.total.max(1))
                }
            } else if st.canceled {
                "canceled".to_string()
            } else {
                "done".to_string()
            };
            lines.push(Line::from(vec![
                Span::styled("health_check: ", Style::default().fg(p.muted)),
                Span::styled(
                    status,
                    Style::default().fg(if st.done && !st.canceled {
                        p.good
                    } else {
                        p.warn
                    }),
                ),
            ]));
            if let Some(e) = st.last_error.as_deref()
                && !e.trim().is_empty()
            {
                lines.push(Line::from(vec![
                    Span::raw("             "),
                    Span::styled(shorten(e, 80), Style::default().fg(p.muted)),
                ]));
            }
        }

        if let Some(health) = snapshot.config_health.get(cfg.name.as_str()) {
            let age = format_age(now_ms(), Some(health.checked_at_ms));
            lines.push(Line::from(vec![
                Span::styled("health: ", Style::default().fg(p.muted)),
                Span::styled(
                    format!("checked {age} ago"),
                    Style::default().fg(p.muted).add_modifier(Modifier::DIM),
                ),
            ]));
            for (idx, u) in health.upstreams.iter().enumerate() {
                let ok = u.ok.unwrap_or(false);
                let status = u
                    .status_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".to_string());
                let ms = u
                    .latency_ms
                    .map(|c| format!("{c}ms"))
                    .unwrap_or_else(|| "-".to_string());
                let head = format!("{idx:>2}. ");
                lines.push(Line::from(vec![
                    Span::styled(head, Style::default().fg(p.muted)),
                    Span::styled(
                        if ok { "ok" } else { "err" },
                        Style::default().fg(if ok { p.good } else { p.warn }),
                    ),
                    Span::raw("  "),
                    Span::styled(status, Style::default().fg(p.muted)),
                    Span::raw("  "),
                    Span::styled(ms, Style::default().fg(p.muted)),
                    Span::raw("  "),
                    Span::styled(shorten_middle(&u.base_url, 60), Style::default().fg(p.text)),
                ]));
                if !ok
                    && let Some(e) = u.error.as_deref()
                    && !e.trim().is_empty()
                {
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(shorten(e, 80), Style::default().fg(p.muted)),
                    ]));
                }
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("health: ", Style::default().fg(p.muted)),
                Span::styled(
                    "not checked (press 'h')",
                    Style::default().fg(p.muted).add_modifier(Modifier::DIM),
                ),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Upstreams",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if cfg.upstreams.is_empty() {
            lines.push(Line::from(Span::styled(
                "(none)",
                Style::default().fg(p.muted),
            )));
        } else {
            for (idx, u) in cfg.upstreams.iter().enumerate() {
                let pid = u.provider_id.as_deref().unwrap_or("-");
                lines.push(Line::from(vec![
                    Span::styled(format!("{idx:>2}. "), Style::default().fg(p.muted)),
                    Span::styled(pid.to_string(), Style::default().fg(p.muted)),
                    Span::raw("  "),
                    Span::styled(u.base_url.clone(), Style::default().fg(p.text)),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Actions",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(crate::tui::i18n::pick(
            ui.language,
            "  i            Provider 详情（可滚动）",
            "  i            provider details (scrollable)",
        )));
        lines.push(Line::from(
            "  Enter        set global override to selected config",
        ));
        lines.push(Line::from("  Backspace    clear global override"));
        lines.push(Line::from(
            "  o            set session override to selected config",
        ));
        lines.push(Line::from("  O            clear session override"));
        lines.push(Line::from("  h            health check selected config"));
        lines.push(Line::from("  H            health check all configs"));
        lines.push(Line::from("  c            cancel health check (selected)"));
        lines.push(Line::from("  C            cancel health check (all)"));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Edit (hot reload + persisted)",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(
            "  t            toggle enabled (immediate, saved)",
        ));
        lines.push(Line::from("  +/-          adjust level (immediate, saved)"));
    } else {
        lines.push(Line::from(Span::styled(
            "No configs available.",
            Style::default().fg(p.muted),
        )));
    }

    let right_block = Block::default()
        .title(Span::styled(
            right_title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let content = Paragraph::new(Text::from(lines))
        .block(right_block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false });
    f.render_widget(content, columns[1]);
}
