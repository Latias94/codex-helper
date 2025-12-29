use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Buffer, Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, HighlightSpacing, List, ListItem, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table, Wrap,
};

use super::model::{
    Palette, ProviderOption, Snapshot, basename, compute_window_stats, format_age, now_ms,
    short_sid, shorten, status_style, tokens_short, usage_line,
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
        Overlay::Help => render_help_modal(f, p, ui.language),
        Overlay::ConfigInfo => render_config_info_modal(f, p, ui, snapshot, providers),
        Overlay::EffortMenu => render_effort_modal(f, p, ui),
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            let title = match ui.overlay {
                Overlay::ProviderMenuSession => crate::tui::i18n::pick(
                    ui.language,
                    "会话 Provider 覆盖",
                    "Session provider override",
                ),
                Overlay::ProviderMenuGlobal => crate::tui::i18n::pick(
                    ui.language,
                    "全局 Provider 覆盖",
                    "Global provider override",
                ),
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
    let (hc_running, hc_canceling) = {
        let mut running = 0usize;
        let mut canceling = 0usize;
        for st in snapshot.health_checks.values() {
            if !st.done {
                running += 1;
                if st.cancel_requested {
                    canceling += 1;
                }
            }
        }
        (running, canceling)
    };

    let global_cfg = snapshot
        .global_override
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let focus = match ui.focus {
        Focus::Sessions => crate::tui::i18n::pick(ui.language, "会话", "Sessions"),
        Focus::Requests => crate::tui::i18n::pick(ui.language, "请求", "Requests"),
        Focus::Configs => crate::tui::i18n::pick(ui.language, "配置", "Configs"),
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
        Span::styled(
            format!(
                "{}{focus}",
                crate::tui::i18n::pick(ui.language, "焦点：", "focus: ")
            ),
            Style::default().fg(p.muted),
        ),
    ]);

    let last_req = snapshot.recent.first();
    let last_provider = last_req
        .and_then(|r| r.provider_id.as_deref())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let last_config = last_req
        .and_then(|r| r.config_name.as_deref())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("-");
    let last_attempts = last_req
        .and_then(|r| r.retry.as_ref())
        .map(|x| x.attempts)
        .unwrap_or(1);

    let fmt_ok_pct = |ok: usize, total: usize| -> String {
        if total == 0 {
            "-".to_string()
        } else {
            format!("{:>2}%", ((ok as f64) * 100.0 / (total as f64)).round())
        }
    };
    let fmt_ms = |ms: Option<u64>| -> String {
        ms.map(|m| format!("{m}ms"))
            .unwrap_or_else(|| "-".to_string())
    };
    let fmt_attempts = |a: Option<f64>| -> String {
        a.map(|v| format!("{v:.1}"))
            .unwrap_or_else(|| "-".to_string())
    };

    let s5 = &snapshot.stats_5m;
    let s1 = &snapshot.stats_1h;

    let subtitle = Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "活跃 ", "active "),
            Style::default().fg(p.muted),
        ),
        Span::styled(active_total.to_string(), Style::default().fg(p.good)),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "错误(80) ", "errors(80) "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            recent_err.to_string(),
            Style::default().fg(if recent_err > 0 { p.warn } else { p.muted }),
        ),
        Span::raw("   "),
        Span::styled("5m ", Style::default().fg(p.muted)),
        Span::styled(
            fmt_ok_pct(s5.ok_2xx, s5.total),
            Style::default().fg(if s5.total > 0 && s5.ok_2xx == s5.total {
                p.good
            } else {
                p.muted
            }),
        ),
        Span::raw(" "),
        Span::styled("p95 ", Style::default().fg(p.muted)),
        Span::styled(fmt_ms(s5.p95_ms), Style::default().fg(p.muted)),
        Span::raw(" "),
        Span::styled("att ", Style::default().fg(p.muted)),
        Span::styled(fmt_attempts(s5.avg_attempts), Style::default().fg(p.muted)),
        Span::raw(" "),
        Span::styled("429 ", Style::default().fg(p.muted)),
        Span::styled(
            s5.err_429.to_string(),
            Style::default().fg(if s5.err_429 > 0 { p.warn } else { p.muted }),
        ),
        Span::raw(" "),
        Span::styled("5xx ", Style::default().fg(p.muted)),
        Span::styled(
            s5.err_5xx.to_string(),
            Style::default().fg(if s5.err_5xx > 0 { p.warn } else { p.muted }),
        ),
        Span::raw("   "),
        Span::styled("1h ", Style::default().fg(p.muted)),
        Span::styled(
            fmt_ok_pct(s1.ok_2xx, s1.total),
            Style::default().fg(p.muted),
        ),
        Span::raw(" "),
        Span::styled("p95 ", Style::default().fg(p.muted)),
        Span::styled(fmt_ms(s1.p95_ms), Style::default().fg(p.muted)),
        Span::raw(" "),
        Span::styled("429 ", Style::default().fg(p.muted)),
        Span::styled(
            s1.err_429.to_string(),
            Style::default().fg(if s1.err_429 > 0 { p.warn } else { p.muted }),
        ),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "当前 ", "cur "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            format!("{last_provider}/{last_config}×{last_attempts}"),
            Style::default().fg(p.accent),
        ),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "健康检查 ", "hc "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            if hc_running > 0 {
                if ui.language == crate::tui::Language::Zh {
                    format!("运行:{hc_running} 取消:{hc_canceling}")
                } else {
                    format!("run:{hc_running} cancel:{hc_canceling}")
                }
            } else {
                "-".to_string()
            },
            Style::default().fg(if hc_running > 0 { p.accent } else { p.muted }),
        ),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "覆盖 ", "overrides "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            format!("{overrides_effort}/{overrides_cfg}"),
            Style::default().fg(p.muted),
        ),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "配置(全局) ", "cfg(global) "),
            Style::default().fg(p.muted),
        ),
        Span::styled(global_cfg.to_string(), Style::default().fg(p.accent)),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "刷新 ", "updated "),
            Style::default().fg(p.muted),
        ),
        Span::styled(format!("{updated}ms"), Style::default().fg(p.muted)),
    ]);

    let tabs = ratatui::widgets::Tabs::new(
        page_titles(ui.language)
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
        Page::Settings => render_settings_page(f, p, ui, snapshot, area),
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

#[allow(dead_code)]
fn render_placeholder(
    f: &mut Frame<'_>,
    p: Palette,
    lang: crate::tui::Language,
    title: &str,
    area: Rect,
) {
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));
    f.render_widget(block, area);
    let content = Paragraph::new(crate::tui::i18n::pick(
        lang,
        "本页预留给后续操作与工作流。",
        "This page is reserved for future operations and workflows.",
    ))
    .style(Style::default().fg(p.muted))
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    f.render_widget(content, area);
}

fn render_settings_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let now_epoch_ms = now_ms();
    let block = Block::default()
        .title(Span::styled(
            crate::tui::i18n::pick(ui.language, "设置", "Settings"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();

    let lang_name = match ui.language {
        crate::tui::Language::Zh => "中文",
        crate::tui::Language::En => "English",
    };
    let refresh_env = std::env::var("CODEX_HELPER_TUI_REFRESH_MS").ok();
    let recent_max_env = std::env::var("CODEX_HELPER_RECENT_FINISHED_MAX").ok();
    let health_timeout_env = std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_TIMEOUT_MS").ok();
    let health_inflight_env = std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_MAX_INFLIGHT").ok();
    let health_upstream_conc_env =
        std::env::var("CODEX_HELPER_TUI_HEALTHCHECK_UPSTREAM_CONCURRENCY").ok();

    let effective_recent_max = recent_max_env
        .as_deref()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(2_000)
        .clamp(200, 20_000);

    let s5 = &snapshot.stats_5m;
    let s1 = &snapshot.stats_1h;
    let ok_pct = |ok: usize, total: usize| -> String {
        if total == 0 {
            "-".to_string()
        } else {
            format!("{:.0}%", (ok as f64) * 100.0 / (total as f64))
        }
    };

    lines.push(Line::from(vec![Span::styled(
        crate::tui::i18n::pick(ui.language, "运行态概览", "Runtime overview"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled("5m ", Style::default().fg(p.muted)),
        Span::styled(
            format!(
                "ok={}  p95={}  att={}  429={}  5xx={}  n={}",
                ok_pct(s5.ok_2xx, s5.total),
                s5.p95_ms
                    .map(|v| format!("{v}ms"))
                    .unwrap_or_else(|| "-".to_string()),
                s5.avg_attempts
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "-".to_string()),
                s5.err_429,
                s5.err_5xx,
                s5.total
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("1h ", Style::default().fg(p.muted)),
        Span::styled(
            format!(
                "ok={}  p95={}  att={}  429={}  5xx={}  n={}",
                ok_pct(s1.ok_2xx, s1.total),
                s1.p95_ms
                    .map(|v| format!("{v}ms"))
                    .unwrap_or_else(|| "-".to_string()),
                s1.avg_attempts
                    .map(|v| format!("{v:.1}"))
                    .unwrap_or_else(|| "-".to_string()),
                s1.err_429,
                s1.err_5xx,
                s1.total
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    if let Some((pid, n)) = s5.top_provider.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("5m top provider: ", Style::default().fg(p.muted)),
            Span::styled(pid.to_string(), Style::default().fg(p.text)),
            Span::styled(format!("  n={n}"), Style::default().fg(p.muted)),
        ]));
    }
    if let Some((cfg, n)) = s5.top_config.as_ref() {
        lines.push(Line::from(vec![
            Span::styled("5m top config: ", Style::default().fg(p.muted)),
            Span::styled(cfg.to_string(), Style::default().fg(p.text)),
            Span::styled(format!("  n={n}"), Style::default().fg(p.muted)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        crate::tui::i18n::pick(ui.language, "TUI 选项", "TUI options"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "语言：", "language: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(lang_name, Style::default().fg(p.text)),
        Span::styled(
            crate::tui::i18n::pick(
                ui.language,
                "  (按 L 切换，并落盘到 ui.language)",
                "  (press L to toggle and persist to ui.language)",
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "刷新间隔：", "refresh: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(format!("{}ms", ui.refresh_ms), Style::default().fg(p.text)),
        Span::styled(
            format!(
                "  env CODEX_HELPER_TUI_REFRESH_MS={}",
                refresh_env.as_deref().unwrap_or("-")
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "窗口采样：", "window samples: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            format!("recent_finished_max={effective_recent_max}"),
            Style::default().fg(p.text),
        ),
        Span::styled(
            format!(
                "  env CODEX_HELPER_RECENT_FINISHED_MAX={}",
                recent_max_env.as_deref().unwrap_or("-")
            ),
            Style::default().fg(p.muted),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        crate::tui::i18n::pick(ui.language, "Health Check", "Health Check"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "timeout_ms={}  max_inflight={}  upstream_concurrency={}",
            health_timeout_env.as_deref().unwrap_or("-"),
            health_inflight_env.as_deref().unwrap_or("-"),
            health_upstream_conc_env.as_deref().unwrap_or("-"),
        ),
        Style::default().fg(p.muted),
    )]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        crate::tui::i18n::pick(ui.language, "路径", "Paths"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled("config: ", Style::default().fg(p.muted)),
        Span::styled(
            crate::config::config_file_path().display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));
    let home = crate::config::proxy_home_dir();
    lines.push(Line::from(vec![
        Span::styled("home:   ", Style::default().fg(p.muted)),
        Span::styled(home.display().to_string(), Style::default().fg(p.text)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("logs:   ", Style::default().fg(p.muted)),
        Span::styled(
            home.join("logs").display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("reports:", Style::default().fg(p.muted)),
        Span::styled(
            home.join("reports").display().to_string(),
            Style::default().fg(p.text),
        ),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        crate::tui::i18n::pick(ui.language, "常用快捷键", "Common keys"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(crate::tui::i18n::pick(
        ui.language,
        "  1-6 切页  ? 帮助  q 退出  L 语言  (Configs: i 详情  Stats: y 导出/复制)",
        "  1-6 pages  ? help  q quit  L language  (Configs: i details  Stats: y export/copy)",
    )));

    lines.push(Line::from(""));
    let updated_ms = snapshot.refreshed_at.elapsed().as_millis();
    lines.push(Line::from(vec![
        Span::styled("updated: ", Style::default().fg(p.muted)),
        Span::styled(format!("{updated_ms}ms"), Style::default().fg(p.muted)),
        Span::raw("  "),
        Span::styled("now: ", Style::default().fg(p.muted)),
        Span::styled(now_epoch_ms.to_string(), Style::default().fg(p.muted)),
    ]));

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false });
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
            Page::Dashboard => crate::tui::i18n::pick(
                ui.language,
                "1-6 页面  q 退出  L 语言  Tab 焦点  ↑/↓ 或 j/k 移动  Enter effort  l/m/h/X 设置  x 清除  p 会话配置  P 全局配置  ? 帮助",
                "1-6 pages  q quit  L language  Tab focus  ↑/↓ or j/k move  Enter effort  l/m/h/X set  x clear  p session cfg  P global cfg  ? help",
            ),
            Page::Configs => crate::tui::i18n::pick(
                ui.language,
                "1-6 页面  q 退出  L 语言  ↑/↓ 选择  i 详情  t 切换 enabled  +/- level  h 检查  H 全部检查  c 取消  C 全部取消  Enter 全局 override  Backspace 清除  o 会话 override  O 清除  ? 帮助",
                "1-6 pages  q quit  L language  ↑/↓ select  i details  t toggle enabled  +/- level  h check  H check all  c cancel  C cancel all  Enter global override  Backspace clear  o session override  O clear  ? help",
            ),
            Page::Requests => crate::tui::i18n::pick(
                ui.language,
                "q 退出  L 语言  ↑/↓ 选择  e 仅看错误  s scope(会话/全部)  ? 帮助",
                "q quit  L language  ↑/↓ select  e errors_only  s scope(session/all)  ? help",
            ),
            Page::Sessions => crate::tui::i18n::pick(
                ui.language,
                "q 退出  L 语言  ↑/↓ 选择  a 仅看活跃  e 仅看错误  v 仅看覆盖  r 重置  ? 帮助",
                "q quit  L language  ↑/↓ select  a active_only  e errors_only  v overrides_only  r reset  ? help",
            ),
            Page::Stats => crate::tui::i18n::pick(
                ui.language,
                "1-6 页面  q 退出  L 语言  Tab 焦点(config/provider)  ↑/↓ 选择  d 天数(7/21/60)  e 仅看错误(recent)  y 复制+导出报告  ? 帮助",
                "1-6 pages  q quit  L language  Tab focus(config/provider)  ↑/↓ select  d days(7/21/60)  e errors_only(recent)  y copy+export report  ? help",
            ),
            Page::Settings => crate::tui::i18n::pick(
                ui.language,
                "q 退出  L 语言  ? 帮助  （本页为运行态/配置入口）",
                "q quit  L language  ? help  (runtime + config overview)",
            ),
        },
        Overlay::Help => crate::tui::i18n::pick(
            ui.language,
            "Esc 关闭帮助  L 语言",
            "Esc close help  L language",
        ),
        Overlay::EffortMenu => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择  Enter 应用  Esc 取消",
            "↑/↓ select  Enter apply  Esc cancel",
        ),
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 选择  Enter 应用  Esc 取消",
            "↑/↓ select  Enter apply  Esc cancel",
        ),
        Overlay::ConfigInfo => crate::tui::i18n::pick(
            ui.language,
            "↑/↓ 滚动  PgUp/PgDn 翻页  Esc 关闭  L 语言",
            "↑/↓ scroll  PgUp/PgDn page  Esc close  L language",
        ),
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

fn render_config_info_modal(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
) {
    let area = centered_rect(84, 84, f.area());
    f.render_widget(Clear, area);

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

    let selected = providers.get(ui.selected_config_idx);
    let title = if let Some(cfg) = selected {
        let level = cfg.level.clamp(1, 10);
        format!(
            "{}: {} (L{})",
            crate::tui::i18n::pick(ui.language, "配置详情", "Config details"),
            cfg.name,
            level
        )
    } else {
        crate::tui::i18n::pick(ui.language, "配置详情", "Config details").to_string()
    };

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            crate::tui::i18n::pick(ui.language, "会话：", "session: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(short_sid(selected_session, 28), Style::default().fg(p.text)),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "固定：", "pinned: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            if let Some(s) = session_override {
                format!("session={s}")
            } else if let Some(g) = global_override {
                format!("global={g}")
            } else {
                "-".to_string()
            },
            Style::default().fg(if session_override.is_some() || global_override.is_some() {
                p.accent
            } else {
                p.muted
            }),
        ),
        Span::raw("   "),
        Span::styled(
            crate::tui::i18n::pick(ui.language, "按键：", "keys: "),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            crate::tui::i18n::pick(
                ui.language,
                "↑/↓ 滚动  PgUp/PgDn 翻页  Esc 关闭  L 语言",
                "↑/↓ scroll  PgUp/PgDn page  Esc close  L language",
            ),
            Style::default().fg(p.muted),
        ),
    ]));
    lines.push(Line::from(""));

    if let Some(cfg) = selected {
        let now = now_ms();

        let stats_5m_cfg = compute_window_stats(&snapshot.recent, now, 5 * 60_000, |r| {
            r.config_name.as_deref() == Some(cfg.name.as_str())
        });
        let stats_1h_cfg = compute_window_stats(&snapshot.recent, now, 60 * 60_000, |r| {
            r.config_name.as_deref() == Some(cfg.name.as_str())
        });

        let fmt_ok_pct = |ok: usize, total: usize| -> String {
            if total == 0 {
                "-".to_string()
            } else {
                format!("{:>2}%", ((ok as f64) * 100.0 / (total as f64)).round())
            }
        };
        let fmt_ms = |ms: Option<u64>| -> String {
            ms.map(|m| format!("{m}ms"))
                .unwrap_or_else(|| "-".to_string())
        };
        let fmt_attempts = |a: Option<f64>| -> String {
            a.map(|v| format!("{v:.1}"))
                .unwrap_or_else(|| "-".to_string())
        };
        let fmt_rate_pct = |r: Option<f64>| -> String {
            r.map(|v| format!("{:.0}%", v * 100.0))
                .unwrap_or_else(|| "-".to_string())
        };

        let (enabled_ovr, level_ovr) = snapshot
            .config_meta_overrides
            .get(cfg.name.as_str())
            .copied()
            .unwrap_or((None, None));
        let enabled = enabled_ovr.unwrap_or(cfg.enabled);
        let level = level_ovr.unwrap_or(cfg.level).clamp(1, 10);

        if let Some(alias) = cfg.alias.as_deref()
            && !alias.trim().is_empty()
        {
            lines.push(Line::from(vec![
                Span::styled(
                    crate::tui::i18n::pick(ui.language, "别名：", "alias: "),
                    Style::default().fg(p.muted),
                ),
                Span::styled(alias.to_string(), Style::default().fg(p.text)),
            ]));
        }

        lines.push(Line::from(vec![
            Span::styled(
                crate::tui::i18n::pick(ui.language, "状态：", "status: "),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                crate::tui::i18n::pick(
                    ui.language,
                    if enabled { "启用" } else { "禁用" },
                    if enabled { "enabled" } else { "disabled" },
                ),
                Style::default().fg(if enabled { p.good } else { p.warn }),
            ),
            Span::raw("  "),
            Span::styled(
                format!("L{level}"),
                Style::default().fg(if level_ovr.is_some() {
                    p.accent
                } else {
                    p.muted
                }),
            ),
            Span::raw("  "),
            Span::styled(
                crate::tui::i18n::pick(
                    ui.language,
                    if cfg.active { "active" } else { "" },
                    if cfg.active { "active" } else { "" },
                ),
                Style::default().fg(if cfg.active { p.accent } else { p.muted }),
            ),
        ]));
        lines.push(Line::from(""));

        lines.push(Line::from(vec![Span::styled(
            crate::tui::i18n::pick(
                ui.language,
                "运行态（可用性/体验）",
                "Runtime (availability/UX)",
            ),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(vec![
            Span::styled("5m ", Style::default().fg(p.muted)),
            Span::styled(
                crate::tui::i18n::pick(ui.language, "成功 ", "ok "),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                fmt_ok_pct(stats_5m_cfg.ok_2xx, stats_5m_cfg.total),
                Style::default().fg(
                    if stats_5m_cfg.total > 0 && stats_5m_cfg.ok_2xx == stats_5m_cfg.total {
                        p.good
                    } else {
                        p.muted
                    },
                ),
            ),
            Span::raw("  "),
            Span::styled("p95 ", Style::default().fg(p.muted)),
            Span::styled(fmt_ms(stats_5m_cfg.p95_ms), Style::default().fg(p.muted)),
            Span::raw("  "),
            Span::styled("att ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_attempts(stats_5m_cfg.avg_attempts),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled("r ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_rate_pct(stats_5m_cfg.retry_rate),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled("429 ", Style::default().fg(p.muted)),
            Span::styled(
                stats_5m_cfg.err_429.to_string(),
                Style::default().fg(if stats_5m_cfg.err_429 > 0 {
                    p.warn
                } else {
                    p.muted
                }),
            ),
            Span::raw("  "),
            Span::styled("5xx ", Style::default().fg(p.muted)),
            Span::styled(
                stats_5m_cfg.err_5xx.to_string(),
                Style::default().fg(if stats_5m_cfg.err_5xx > 0 {
                    p.warn
                } else {
                    p.muted
                }),
            ),
            Span::raw("  "),
            Span::styled(
                format!("n={}", stats_5m_cfg.total),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("1h ", Style::default().fg(p.muted)),
            Span::styled(
                crate::tui::i18n::pick(ui.language, "成功 ", "ok "),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                fmt_ok_pct(stats_1h_cfg.ok_2xx, stats_1h_cfg.total),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled("p95 ", Style::default().fg(p.muted)),
            Span::styled(fmt_ms(stats_1h_cfg.p95_ms), Style::default().fg(p.muted)),
            Span::raw("  "),
            Span::styled("att ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_attempts(stats_1h_cfg.avg_attempts),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled("r ", Style::default().fg(p.muted)),
            Span::styled(
                fmt_rate_pct(stats_1h_cfg.retry_rate),
                Style::default().fg(p.muted),
            ),
            Span::raw("  "),
            Span::styled("429 ", Style::default().fg(p.muted)),
            Span::styled(
                stats_1h_cfg.err_429.to_string(),
                Style::default().fg(if stats_1h_cfg.err_429 > 0 {
                    p.warn
                } else {
                    p.muted
                }),
            ),
            Span::raw("  "),
            Span::styled("5xx ", Style::default().fg(p.muted)),
            Span::styled(
                stats_1h_cfg.err_5xx.to_string(),
                Style::default().fg(if stats_1h_cfg.err_5xx > 0 {
                    p.warn
                } else {
                    p.muted
                }),
            ),
            Span::raw("  "),
            Span::styled(
                format!("n={}", stats_1h_cfg.total),
                Style::default().fg(p.muted),
            ),
        ]));
        if let Some((pid, cnt)) = stats_5m_cfg.top_provider.as_ref() {
            lines.push(Line::from(vec![
                Span::styled("5m top ", Style::default().fg(p.muted)),
                Span::styled(pid.to_string(), Style::default().fg(p.text)),
                Span::styled(format!("  n={cnt}"), Style::default().fg(p.muted)),
            ]));
        }
        lines.push(Line::from(""));

        lines.push(Line::from(vec![Span::styled(
            crate::tui::i18n::pick(ui.language, "上游（Providers）", "Upstreams (providers)"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));

        let health = snapshot.config_health.get(cfg.name.as_str());
        let lb = snapshot.lb_view.get(cfg.name.as_str());
        let (rt5_by_upstream, rt1_by_upstream) = {
            use std::collections::HashMap;

            #[derive(Default)]
            struct Rt {
                total: usize,
                ok: usize,
                err_429: usize,
                err_5xx: usize,
                ok_lat_ms: Vec<u64>,
                attempts_sum: u64,
                retry_cnt: u64,
            }

            fn add(map: &mut HashMap<String, Rt>, r: &crate::state::FinishedRequest) {
                let Some(url) = r.upstream_base_url.as_deref() else {
                    return;
                };
                if url.trim().is_empty() {
                    return;
                };
                let e = map.entry(url.to_string()).or_default();
                e.total += 1;
                let attempts = r.retry.as_ref().map(|x| x.attempts).unwrap_or(1);
                e.attempts_sum = e.attempts_sum.saturating_add(attempts as u64);
                if attempts > 1 {
                    e.retry_cnt = e.retry_cnt.saturating_add(1);
                }
                if r.status_code == 429 {
                    e.err_429 += 1;
                } else if (500..600).contains(&r.status_code) {
                    e.err_5xx += 1;
                }
                if (200..300).contains(&r.status_code) {
                    e.ok += 1;
                    e.ok_lat_ms.push(r.duration_ms);
                }
            }

            let mut m5: HashMap<String, Rt> = HashMap::new();
            let mut m1: HashMap<String, Rt> = HashMap::new();
            let cutoff_5 = now.saturating_sub(5 * 60_000);
            let cutoff_1 = now.saturating_sub(60 * 60_000);
            for r in snapshot.recent.iter() {
                if r.config_name.as_deref() != Some(cfg.name.as_str()) {
                    continue;
                }
                if r.ended_at_ms >= cutoff_5 {
                    add(&mut m5, r);
                }
                if r.ended_at_ms >= cutoff_1 {
                    add(&mut m1, r);
                }
            }
            (m5, m1)
        };

        if cfg.upstreams.is_empty() {
            lines.push(Line::from(Span::styled(
                crate::tui::i18n::pick(ui.language, "（无）", "(none)"),
                Style::default().fg(p.muted),
            )));
        } else {
            for (idx, up) in cfg.upstreams.iter().enumerate() {
                let pid = up.provider_id.as_deref().unwrap_or("-");
                let auth = up.auth.as_str();

                let (ok, status_code, latency_ms, err) = health
                    .and_then(|h| h.upstreams.iter().find(|u| u.base_url == up.base_url))
                    .map(|u| (u.ok, u.status_code, u.latency_ms, u.error.as_deref()))
                    .unwrap_or((None, None, None, None));

                let health_text = if let Some(ok) = ok {
                    if ok {
                        format!(
                            "ok {} {}",
                            status_code
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                            latency_ms
                                .map(|m| format!("{m}ms"))
                                .unwrap_or_else(|| "-".to_string())
                        )
                    } else {
                        format!(
                            "err {}",
                            status_code
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "-".to_string())
                        )
                    }
                } else {
                    crate::tui::i18n::pick(ui.language, "未检查", "not checked").to_string()
                };

                let lb_text = lb
                    .and_then(|v| v.upstreams.get(idx))
                    .map(|u| {
                        let mut parts = Vec::new();
                        if lb.and_then(|v| v.last_good_index) == Some(idx) {
                            parts.push("last_good".to_string());
                        }
                        if u.failure_count > 0 {
                            parts.push(format!("fail={}", u.failure_count));
                        }
                        if let Some(secs) = u.cooldown_remaining_secs {
                            parts.push(format!("cooldown={secs}s"));
                        }
                        if u.usage_exhausted {
                            parts.push("exhausted".to_string());
                        }
                        if parts.is_empty() {
                            "-".to_string()
                        } else {
                            parts.join(" ")
                        }
                    })
                    .unwrap_or_else(|| "-".to_string());

                let models_text = if up.supported_models.is_empty() && up.model_mapping.is_empty() {
                    crate::tui::i18n::pick(ui.language, "模型：全部", "models: all").to_string()
                } else {
                    let allow = up.supported_models.len();
                    let map = up.model_mapping.len();
                    crate::tui::i18n::pick(
                        ui.language,
                        &format!("模型：allow {allow} / map {map}"),
                        &format!("models: allow {allow} / map {map}"),
                    )
                    .to_string()
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("{idx:>2}. "), Style::default().fg(p.muted)),
                    Span::styled(pid.to_string(), Style::default().fg(p.muted)),
                    Span::raw("  "),
                    Span::styled(shorten(&up.base_url, 100), Style::default().fg(p.text)),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled("auth: ", Style::default().fg(p.muted)),
                    Span::styled(auth.to_string(), Style::default().fg(p.text)),
                    Span::raw("   "),
                    Span::styled(models_text, Style::default().fg(p.muted)),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled("health: ", Style::default().fg(p.muted)),
                    Span::styled(
                        health_text,
                        Style::default().fg(if ok == Some(true) { p.good } else { p.warn }),
                    ),
                    Span::raw("   "),
                    Span::styled("lb: ", Style::default().fg(p.muted)),
                    Span::styled(lb_text, Style::default().fg(p.muted)),
                ]));

                let runtime_line = {
                    fn pct(ok: usize, total: usize) -> String {
                        if total == 0 {
                            "-".to_string()
                        } else {
                            format!("{:.0}%", (ok as f64) * 100.0 / (total as f64))
                        }
                    }
                    fn p95(mut v: Vec<u64>) -> Option<u64> {
                        if v.is_empty() {
                            return None;
                        }
                        let n = v.len();
                        let idx =
                            ((0.95 * (n.saturating_sub(1) as f64)).ceil() as usize).min(n - 1);
                        let (_, nth, _) = v.select_nth_unstable(idx);
                        Some(*nth)
                    }
                    fn att(sum: u64, total: usize) -> String {
                        if total == 0 {
                            "-".to_string()
                        } else {
                            format!("{:.1}", sum as f64 / total as f64)
                        }
                    }

                    let rt5 = rt5_by_upstream.get(&up.base_url);
                    let rt1 = rt1_by_upstream.get(&up.base_url);

                    let s5 = rt5
                        .map(|x| {
                            let p95_ms = p95(x.ok_lat_ms.clone())
                                .map(|v| format!("{v}ms"))
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "5m ok{} p95={} att{} 429={} 5xx={}",
                                pct(x.ok, x.total),
                                p95_ms,
                                att(x.attempts_sum, x.total),
                                x.err_429,
                                x.err_5xx
                            )
                        })
                        .unwrap_or_else(|| "5m -".to_string());
                    let s1 = rt1
                        .map(|x| {
                            let p95_ms = p95(x.ok_lat_ms.clone())
                                .map(|v| format!("{v}ms"))
                                .unwrap_or_else(|| "-".to_string());
                            format!(
                                "1h ok{} p95={} att{} 429={} 5xx={}",
                                pct(x.ok, x.total),
                                p95_ms,
                                att(x.attempts_sum, x.total),
                                x.err_429,
                                x.err_5xx
                            )
                        })
                        .unwrap_or_else(|| "1h -".to_string());
                    format!("{s5} | {s1}")
                };
                lines.push(Line::from(vec![
                    Span::raw("     "),
                    Span::styled("rt: ", Style::default().fg(p.muted)),
                    Span::styled(runtime_line, Style::default().fg(p.muted)),
                ]));

                if !up.tags.is_empty() {
                    let tags = up
                        .tags
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled("tags: ", Style::default().fg(p.muted)),
                        Span::styled(shorten(&tags, 120), Style::default().fg(p.muted)),
                    ]));
                }

                if !up.supported_models.is_empty() {
                    let samples = up
                        .supported_models
                        .iter()
                        .take(8)
                        .cloned()
                        .collect::<Vec<_>>();
                    let mut s = samples.join(", ");
                    if up.supported_models.len() > samples.len() {
                        s.push_str(", …");
                    }
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled("allow: ", Style::default().fg(p.muted)),
                        Span::styled(s, Style::default().fg(p.muted)),
                    ]));
                }
                if !up.model_mapping.is_empty() {
                    let samples = up
                        .model_mapping
                        .iter()
                        .take(6)
                        .map(|(k, v)| format!("{k}->{v}"))
                        .collect::<Vec<_>>();
                    let mut s = samples.join(", ");
                    if up.model_mapping.len() > samples.len() {
                        s.push_str(", …");
                    }
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled("map: ", Style::default().fg(p.muted)),
                        Span::styled(s, Style::default().fg(p.muted)),
                    ]));
                }

                if let Some(e) = err
                    && !e.trim().is_empty()
                {
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(shorten(e, 140), Style::default().fg(p.muted)),
                    ]));
                }
                lines.push(Line::from(""));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            crate::tui::i18n::pick(ui.language, "未选中任何配置。", "No config selected."),
            Style::default().fg(p.muted),
        )));
    }

    let inner_height = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(inner_height);
    ui.config_info_scroll = ui
        .config_info_scroll
        .min(max_scroll.min(u16::MAX as usize) as u16);

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false })
        .scroll((ui.config_info_scroll, 0));
    f.render_widget(content, area);
}

fn render_help_modal(f: &mut Frame<'_>, p: Palette, lang: crate::tui::Language) {
    let area = centered_rect(70, 70, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            crate::tui::i18n::pick(lang, "帮助", "Help"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.focus))
        .style(Style::default().bg(p.panel));

    let lines = if lang == crate::tui::Language::Zh {
        vec![
            Line::from(vec![Span::styled(
                "导航",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  ↑/↓, j/k   移动选中项"),
            Line::from("  1-6        切换页面"),
            Line::from("            1 总览  2 配置  3 会话  4 请求  5 统计  6 设置"),
            Line::from("  L          切换语言（中/英，自动落盘）"),
            Line::from("  Tab        切换焦点（总览页）"),
            Line::from("  6 设置     查看运行态与关键配置入口"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "推理强度",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Enter      打开 effort 菜单（会话列表）"),
            Line::from("  l/m/h/X    设置 low/medium/high/xhigh"),
            Line::from("  x          清除 effort 覆盖"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Provider 覆盖",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  p          会话级 provider 覆盖"),
            Line::from("  P          全局 provider 覆盖"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "配置页（Configs）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Enter      设置全局 override 为当前 config"),
            Line::from("  Backspace  清除全局 override"),
            Line::from("  o          设置会话 override 为当前 config"),
            Line::from("  O          清除会话 override"),
            Line::from("  i          查看 Provider 详情（可滚动）"),
            Line::from("  t          切换 enabled（热更新 + 落盘）"),
            Line::from("  +/-        调整 level（热更新 + 落盘）"),
            Line::from("  h/H        运行健康检查（当前/全部）"),
            Line::from("  c/C        取消健康检查（当前/全部）"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "请求页（Requests）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  e          仅看错误（errors-only）"),
            Line::from("  s          scope：全部 vs 当前会话"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "会话页（Sessions）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  a          仅看活跃（active-only）"),
            Line::from("  e          仅看错误（errors-only）"),
            Line::from("  v          仅看覆盖（overrides-only）"),
            Line::from("  r          重置筛选"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "统计页（Stats）",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Tab        切换焦点（config vs provider）"),
            Line::from("  d          切换窗口（7/21/60 天）"),
            Line::from("  e          recent 仅看错误"),
            Line::from("  y          复制 + 导出报告（当前选中项）"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "退出",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  q          退出并触发 shutdown"),
            Line::from("  Esc/?      关闭帮助"),
        ]
    } else {
        vec![
            Line::from(vec![Span::styled(
                "Navigation",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Tab        switch focus (Dashboard)"),
            Line::from("  ↑/↓, j/k   move selection"),
            Line::from("  1-6        switch page"),
            Line::from(
                "            1 Dashboard  2 Configs  3 Sessions  4 Requests  5 Stats  6 Settings",
            ),
            Line::from("  L          toggle language (zh/en, persisted)"),
            Line::from("  6 Settings show runtime + config overview"),
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
            Line::from("  i          open provider details (scrollable)"),
            Line::from("  t          toggle enabled (hot reload + saved)"),
            Line::from("  +/-        adjust level (hot reload + saved)"),
            Line::from("  h/H        run health checks (selected/all)"),
            Line::from("  c/C        cancel health checks (selected/all)"),
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
                "Stats page",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  Tab        switch focus (config vs provider)"),
            Line::from("  d          cycle time window (7/21/60 days)"),
            Line::from("  e          toggle errors-only (recent breakdown)"),
            Line::from("  y          copy + export report (selected item)"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Quit",
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            )]),
            Line::from("  q          quit and request shutdown"),
            Line::from("  Esc/?      close this modal"),
        ]
    };

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
