use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Buffer, Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, HighlightSpacing, List, ListItem, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table, Wrap,
};

use super::model::{
    Palette, ProviderOption, Snapshot, basename, format_age, now_ms, short_sid, shorten,
    status_style, tokens_short, usage_line,
};
use super::state::UiState;
use super::types::{EffortChoice, Focus, Overlay, Page, page_index, page_titles};

mod stats;

pub(in crate::tui) fn render_app(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    service_name: &'static str,
    port: u16,
    providers: &[ProviderOption],
) {
    f.render_widget(BackgroundWidget { p }, f.area());

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());

    render_header(f, p, ui, snapshot, service_name, port, outer[0]);
    render_body(f, p, ui, snapshot, providers, outer[1]);
    render_footer(f, p, ui, outer[2]);

    match ui.overlay {
        Overlay::None => {}
        Overlay::Help => render_help_modal(f, p),
        Overlay::EffortMenu => render_effort_modal(f, p, ui),
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            let title = match ui.overlay {
                Overlay::ProviderMenuSession => "Session provider override",
                Overlay::ProviderMenuGlobal => "Global provider override",
                _ => unreachable!(),
            };
            render_provider_modal(f, p, ui, providers, title);
        }
    }
}

fn render_header(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    service_name: &'static str,
    port: u16,
    area: Rect,
) {
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 0,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let active_total = snapshot.rows.iter().map(|r| r.active_count).sum::<usize>();
    let recent_err = snapshot
        .recent
        .iter()
        .take(80)
        .filter(|r| r.status_code >= 400)
        .count();
    let updated = snapshot.refreshed_at.elapsed().as_millis();
    let overrides_effort = snapshot.overrides.len();
    let overrides_cfg = snapshot.config_overrides.len();

    let global_cfg = snapshot
        .global_override
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let focus = match ui.focus {
        Focus::Sessions => "Sessions",
        Focus::Requests => "Requests",
        Focus::Configs => "Configs",
    };
    let title = Line::from(vec![
        Span::styled(
            "codex-helper",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{service_name}:{port}"),
            Style::default().fg(p.muted),
        ),
        Span::raw("  "),
        Span::styled(format!("focus: {focus}"), Style::default().fg(p.muted)),
    ]);

    let subtitle = Line::from(vec![
        Span::styled("active ", Style::default().fg(p.muted)),
        Span::styled(active_total.to_string(), Style::default().fg(p.good)),
        Span::raw("   "),
        Span::styled("errors(80) ", Style::default().fg(p.muted)),
        Span::styled(
            recent_err.to_string(),
            Style::default().fg(if recent_err > 0 { p.warn } else { p.muted }),
        ),
        Span::raw("   "),
        Span::styled("overrides ", Style::default().fg(p.muted)),
        Span::styled(
            format!("{overrides_effort}/{overrides_cfg}"),
            Style::default().fg(p.muted),
        ),
        Span::raw("   "),
        Span::styled("cfg(global) ", Style::default().fg(p.muted)),
        Span::styled(global_cfg.to_string(), Style::default().fg(p.accent)),
        Span::raw("   "),
        Span::styled("updated ", Style::default().fg(p.muted)),
        Span::styled(format!("{updated}ms"), Style::default().fg(p.muted)),
    ]);

    let tabs = ratatui::widgets::Tabs::new(
        page_titles()
            .iter()
            .map(|t| Line::from(*t))
            .collect::<Vec<_>>(),
    )
    .select(page_index(ui.page))
    .style(Style::default().fg(p.muted))
    .highlight_style(Style::default().fg(p.text).add_modifier(Modifier::BOLD))
    .divider(Span::raw("  "));

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(p.border));
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(Text::from(title)), chunks[0]);
    f.render_widget(Paragraph::new(Text::from(subtitle)), chunks[1]);
    f.render_widget(tabs, chunks[2]);
}

fn render_body(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    let area = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });

    match ui.page {
        Page::Dashboard => render_dashboard(f, p, ui, snapshot, providers, area),
        Page::Configs => render_configs_page(f, p, ui, snapshot, providers, area),
        Page::Sessions => render_sessions_page(f, p, ui, snapshot, area),
        Page::Requests => render_requests_page(f, p, ui, snapshot, area),
        Page::Stats => stats::render_stats_page(f, p, ui, snapshot, providers, area),
        Page::Settings => render_placeholder(f, p, "Settings view (coming soon)", area),
    }
}

fn render_sessions_page(
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
            .map(|s| shorten(s, 80))
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

fn render_configs_page(
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
            let health = snapshot
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
                .unwrap_or_else(|| "-".to_string());

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
                    Span::styled(shorten(&u.base_url, 60), Style::default().fg(p.text)),
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

fn render_requests_page(
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
            let path = shorten(&r.path, 60);

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
            Span::styled(shorten(&r.path, 80), Style::default().fg(p.text)),
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
                Span::styled(shorten(u, 80), Style::default().fg(p.text)),
            ]));
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
                    Span::styled(shorten(entry, 120), Style::default().fg(p.muted)),
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

fn render_placeholder(f: &mut Frame<'_>, p: Palette, title: &str, area: Rect) {
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));
    f.render_widget(block, area);
    let content = Paragraph::new("This page is reserved for future operations and workflows.")
        .style(Style::default().fg(p.muted))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    f.render_widget(content, area);
}

fn render_dashboard(
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

fn kv_line<'a>(p: Palette, k: &'a str, v: String, v_style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{k}: "), Style::default().fg(p.muted)),
        Span::styled(v, v_style),
    ])
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
        .map(|s| shorten(s, 64))
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
            let path = shorten(&r.path, 48);
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

fn render_footer(f: &mut Frame<'_>, p: Palette, ui: &mut UiState, area: Rect) {
    let now = std::time::Instant::now();
    if let Some((_, ts)) = ui.toast.as_ref()
        && now.duration_since(*ts) > Duration::from_secs(3)
    {
        ui.toast = None;
    }

    let left = match ui.overlay {
        Overlay::None => match ui.page {
            Page::Dashboard => {
                "q quit  Tab focus  ↑/↓ or j/k move  Enter effort  l/m/h/X set effort  x clear  p session cfg  P global cfg  ? help"
            }
            Page::Configs => {
                "q quit  ↑/↓ select  t toggle enabled  +/- level  h check  H check all  Enter global override  Backspace clear  o session override  O clear  ? help"
            }
            Page::Requests => "q quit  ↑/↓ select  e errors_only  s scope(session/all)  ? help",
            Page::Sessions => {
                "q quit  ↑/↓ select  a active_only  e errors_only  v overrides_only  r reset  ? help"
            }
            Page::Stats => {
                "q quit  Tab focus(config/provider)  ↑/↓ select  d days(7/21/60)  ? help"
            }
            Page::Settings => "q quit  ? help",
        },
        Overlay::Help => "Esc close help",
        Overlay::EffortMenu => "↑/↓ select  Enter apply  Esc cancel",
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            "↑/↓ select  Enter apply  Esc cancel"
        }
    };
    let right = ui.toast.as_ref().map(|(s, _)| s.as_str()).unwrap_or("");

    let line = Line::from(vec![
        Span::styled(left, Style::default().fg(p.muted)),
        Span::raw(" "),
        Span::styled(right, Style::default().fg(p.accent)),
    ]);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(p.border));
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(Text::from(line)).wrap(Wrap { trim: true }),
        area,
    );
}

fn render_help_modal(f: &mut Frame<'_>, p: Palette) {
    let area = centered_rect(70, 70, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            "Help",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let lines = vec![
        Line::from(vec![Span::styled(
            "Navigation",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  Tab        switch focus (Dashboard)"),
        Line::from("  ↑/↓, j/k   move selection"),
        Line::from("  1-5        switch page"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Effort",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  Enter      open effort menu (on Sessions)"),
        Line::from("  l/m/h/X    set low/medium/high/xhigh"),
        Line::from("  x          clear effort override"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Provider override",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  p          session provider override"),
        Line::from("  P          global provider override"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Configs page",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  Enter      set global override to selected config"),
        Line::from("  Backspace  clear global override"),
        Line::from("  o          set session override to selected config"),
        Line::from("  O          clear session override"),
        Line::from("  t          toggle enabled (hot reload + saved)"),
        Line::from("  +/-        adjust level (hot reload + saved)"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Requests page",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  e          toggle errors-only filter"),
        Line::from("  s          toggle scope (all vs selected session)"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Sessions page",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  a          toggle active-only"),
        Line::from("  e          toggle errors-only"),
        Line::from("  v          toggle overrides-only"),
        Line::from("  r          reset filters"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Quit",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]),
        Line::from("  q          quit and request shutdown"),
        Line::from("  Esc/?      close this modal"),
    ];

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

fn render_effort_modal(f: &mut Frame<'_>, p: Palette, ui: &mut UiState) {
    let area = centered_rect(50, 55, f.area());
    f.render_widget(Clear, area);
    let focused = ui.overlay == Overlay::EffortMenu;
    let block = Block::default()
        .title(Span::styled(
            "Set reasoning effort",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { p.focus } else { p.border }))
        .style(Style::default().bg(p.panel));

    let choices = [
        EffortChoice::Clear,
        EffortChoice::Low,
        EffortChoice::Medium,
        EffortChoice::High,
        EffortChoice::XHigh,
    ];
    let items = choices
        .iter()
        .map(|c| ListItem::new(Line::from(c.label())))
        .collect::<Vec<_>>();

    ui.menu_list.select(Some(
        ui.effort_menu_idx.min(choices.len().saturating_sub(1)),
    ));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}

fn render_provider_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    providers: &[ProviderOption],
    title: &str,
) {
    let area = centered_rect(60, 70, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let mut items = Vec::with_capacity(providers.len() + 1);
    items.push(ListItem::new(Line::from("(Clear override)")));
    for pvd in providers {
        let mut label = format!("L{} {}", pvd.level.clamp(1, 10), pvd.name);
        if pvd.active {
            label.push_str(" *");
        }
        if !pvd.enabled {
            label.push_str(" [off]");
        }
        if let Some(alias) = pvd.alias.as_deref()
            && !alias.trim().is_empty()
            && alias != pvd.name
        {
            label.push_str(&format!(" ({alias})"));
        }
        let style = Style::default().fg(if pvd.enabled { p.text } else { p.muted });
        items.push(ListItem::new(Line::from(label)).style(style));
    }

    let max = items.len().saturating_sub(1);
    ui.menu_list.select(Some(ui.provider_menu_idx.min(max)));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ");
    f.render_stateful_widget(list, area, &mut ui.menu_list);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn render_background(area: Rect, buf: &mut Buffer, p: Palette) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].set_style(Style::default().bg(p.bg));
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct BackgroundWidget {
    p: Palette,
}

impl ratatui::widgets::Widget for BackgroundWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        render_background(area, buf, self.p);
    }
}
