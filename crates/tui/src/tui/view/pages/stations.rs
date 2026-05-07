use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::state::{BalanceSnapshotStatus, ProviderBalanceSnapshot};
use crate::tui::ProviderOption;
use crate::tui::model::{Palette, Snapshot, format_age, now_ms, shorten, shorten_middle};
use crate::tui::state::UiState;

fn balance_status_label(status: BalanceSnapshotStatus) -> &'static str {
    match status {
        BalanceSnapshotStatus::Unknown => "unknown",
        BalanceSnapshotStatus::Ok => "ok",
        BalanceSnapshotStatus::Exhausted => "exhausted",
        BalanceSnapshotStatus::Stale => "stale",
        BalanceSnapshotStatus::Error => "error",
    }
}

fn balance_status_style(p: Palette, status: BalanceSnapshotStatus) -> Style {
    match status {
        BalanceSnapshotStatus::Ok => Style::default().fg(p.good),
        BalanceSnapshotStatus::Exhausted | BalanceSnapshotStatus::Error => {
            Style::default().fg(p.bad)
        }
        BalanceSnapshotStatus::Stale => Style::default().fg(p.warn),
        BalanceSnapshotStatus::Unknown => Style::default().fg(p.muted),
    }
}

fn balance_amounts(snapshot: &ProviderBalanceSnapshot) -> String {
    let mut parts = Vec::new();
    if let Some(total) = snapshot.total_balance_usd.as_deref() {
        parts.push(format!("total=${total}"));
    }
    if let Some(budget) = snapshot.monthly_budget_usd.as_deref() {
        parts.push(format!("budget=${budget}"));
    }
    if let Some(spent) = snapshot.monthly_spent_usd.as_deref() {
        parts.push(format!("spent=${spent}"));
    }
    if let Some(sub) = snapshot.subscription_balance_usd.as_deref() {
        parts.push(format!("sub=${sub}"));
    }
    if let Some(paygo) = snapshot.paygo_balance_usd.as_deref() {
        parts.push(format!("paygo=${paygo}"));
    }
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StationRoutingPreview {
    mode: String,
    order: Vec<String>,
    skipped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StationRoutingCandidate {
    name: String,
    level: u8,
    active: bool,
    upstreams: usize,
}

fn station_routing_preview(
    providers: &[ProviderOption],
    station_meta_overrides: &HashMap<String, (Option<bool>, Option<u8>)>,
    session_override: Option<&str>,
    global_station_override: Option<&str>,
) -> StationRoutingPreview {
    if let Some(station) = non_empty_trimmed(session_override) {
        return pinned_station_routing_preview("pinned(session)", station, providers);
    }
    if let Some(station) = non_empty_trimmed(global_station_override) {
        return pinned_station_routing_preview("pinned(global-station)", station, providers);
    }
    automatic_station_routing_preview(providers, station_meta_overrides)
}

fn pinned_station_routing_preview(
    source: &str,
    station_name: &str,
    providers: &[ProviderOption],
) -> StationRoutingPreview {
    let order = providers
        .iter()
        .find(|provider| provider.name == station_name)
        .map(|provider| {
            vec![format!(
                "{} [L{}, pinned, upstreams={}]",
                provider.name,
                provider.level.clamp(1, 10),
                provider.upstreams.len()
            )]
        })
        .unwrap_or_default();
    let skipped = if order.is_empty() {
        vec![format!("{station_name}: missing pinned station")]
    } else {
        Vec::new()
    };

    StationRoutingPreview {
        mode: format!("{source}={station_name}"),
        order,
        skipped,
    }
}

fn automatic_station_routing_preview(
    providers: &[ProviderOption],
    station_meta_overrides: &HashMap<String, (Option<bool>, Option<u8>)>,
) -> StationRoutingPreview {
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();

    for provider in providers {
        let (enabled, level) = effective_station_enabled_level(provider, station_meta_overrides);
        let mut reasons = Vec::new();
        if !enabled && !provider.active {
            reasons.push("disabled");
        }
        if provider.upstreams.is_empty() {
            reasons.push("no_upstreams");
        }
        if reasons.is_empty() {
            candidates.push(StationRoutingCandidate {
                name: provider.name.clone(),
                level,
                active: provider.active,
                upstreams: provider.upstreams.len(),
            });
        } else {
            skipped.push(format!("{}: {}", provider.name, reasons.join(",")));
        }
    }

    let mut levels = candidates
        .iter()
        .map(|candidate| candidate.level)
        .collect::<Vec<_>>();
    levels.sort_unstable();
    levels.dedup();
    let has_multi_level = levels.len() > 1;

    if has_multi_level {
        candidates.sort_by(|a, b| {
            a.level
                .cmp(&b.level)
                .then_with(|| b.active.cmp(&a.active))
                .then_with(|| a.name.cmp(&b.name))
        });
    } else {
        candidates.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(pos) = candidates.iter().position(|candidate| candidate.active) {
            let item = candidates.remove(pos);
            candidates.insert(0, item);
        }
    }

    StationRoutingPreview {
        mode: if has_multi_level {
            "auto(level fallback)".to_string()
        } else {
            "auto(single-level fallback)".to_string()
        },
        order: candidates
            .iter()
            .map(|candidate| {
                let active = if candidate.active { ", active" } else { "" };
                format!(
                    "{} [L{}{}, upstreams={}]",
                    candidate.name, candidate.level, active, candidate.upstreams
                )
            })
            .collect(),
        skipped,
    }
}

fn effective_station_enabled_level(
    provider: &ProviderOption,
    station_meta_overrides: &HashMap<String, (Option<bool>, Option<u8>)>,
) -> (bool, u8) {
    let (enabled_override, level_override) = station_meta_overrides
        .get(provider.name.as_str())
        .copied()
        .unwrap_or((None, None));
    (
        enabled_override.unwrap_or(provider.enabled),
        level_override.unwrap_or(provider.level).clamp(1, 10),
    )
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    (!value.is_empty()).then_some(value)
}

pub(super) fn render_stations_page(
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
        .and_then(|r| r.override_station_name.as_deref());
    let global_station_override = snapshot.global_station_override.as_deref();

    let left_block = Block::default()
        .title(Span::styled(
            format!("Stations  (session: {selected_session})"),
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
                .station_meta_overrides
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
                    .station_health
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
            if global_station_override == Some(cfg.name.as_str()) {
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

    ui.stations_table.select(if providers.is_empty() {
        None
    } else {
        Some(ui.selected_station_idx)
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
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, columns[0], &mut ui.stations_table);

    let selected = providers.get(ui.selected_station_idx);
    let right_title = selected
        .map(|c| format!("Station details: {} (L{})", c.name, c.level.clamp(1, 10)))
        .unwrap_or_else(|| "Station details".to_string());

    let mut lines = Vec::new();
    if let Some(cfg) = selected {
        let (enabled_ovr, level_ovr) = snapshot
            .station_meta_overrides
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

        let routing = station_routing_preview(
            providers,
            &snapshot.station_meta_overrides,
            session_override,
            global_station_override,
        );
        lines.push(Line::from(vec![
            Span::styled("routing: ", Style::default().fg(p.muted)),
            Span::styled(routing.mode, Style::default().fg(p.muted)),
        ]));
        if !routing.order.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("order: ", Style::default().fg(p.muted)),
                Span::styled(
                    shorten_middle(&routing.order.join(" > "), 96),
                    Style::default().fg(p.text),
                ),
            ]));
        }
        if !routing.skipped.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("skipped: ", Style::default().fg(p.muted)),
                Span::styled(
                    shorten_middle(&routing.skipped.join(" | "), 96),
                    Style::default().fg(p.muted),
                ),
            ]));
        }

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

        if let Some(health) = snapshot.station_health.get(cfg.name.as_str()) {
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
            "Balance",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if let Some(balances) = snapshot.provider_balances.get(cfg.name.as_str()) {
            if balances.is_empty() {
                lines.push(Line::from(Span::styled(
                    "(none)",
                    Style::default().fg(p.muted),
                )));
            } else {
                for balance in balances.iter().take(12) {
                    let idx = balance
                        .upstream_index
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    let status = balance_status_label(balance.status);
                    lines.push(Line::from(vec![
                        Span::styled(format!("{idx:>2}. "), Style::default().fg(p.muted)),
                        Span::styled(
                            shorten_middle(&balance.provider_id, 20),
                            Style::default().fg(p.text),
                        ),
                        Span::raw("  "),
                        Span::styled(status, balance_status_style(p, balance.status)),
                        Span::raw("  "),
                        Span::styled(
                            shorten(&balance_amounts(balance), 72),
                            Style::default().fg(p.muted),
                        ),
                    ]));
                    if let Some(err) = balance.error.as_deref()
                        && !err.trim().is_empty()
                    {
                        lines.push(Line::from(vec![
                            Span::raw("     "),
                            Span::styled(shorten(err, 80), Style::default().fg(p.muted)),
                        ]));
                    }
                }
                if balances.len() > 12 {
                    lines.push(Line::from(Span::styled(
                        format!("… +{} more", balances.len() - 12),
                        Style::default().fg(p.muted),
                    )));
                }
            }
        } else {
            lines.push(Line::from(Span::styled(
                "(none)",
                Style::default().fg(p.muted),
            )));
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
            "  Enter        set active station (same-level failover enabled)",
        ));
        lines.push(Line::from("  Backspace    clear active (auto)"));
        lines.push(Line::from(
            "  o            set session override to selected station",
        ));
        lines.push(Line::from("  O            clear session override"));
        lines.push(Line::from("  h            health check selected station"));
        lines.push(Line::from("  H            health check all stations"));
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
            "No stations available.",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::UpstreamSummary;

    fn provider(
        name: &str,
        enabled: bool,
        level: u8,
        active: bool,
        upstreams: usize,
    ) -> ProviderOption {
        ProviderOption {
            name: name.to_string(),
            alias: None,
            enabled,
            level,
            active,
            upstreams: (0..upstreams)
                .map(|idx| UpstreamSummary {
                    base_url: format!("https://{name}-{idx}.example/v1"),
                    ..UpstreamSummary::default()
                })
                .collect(),
        }
    }

    #[test]
    fn station_routing_preview_uses_single_level_fallback_order() {
        let providers = vec![
            provider("alpha", true, 1, false, 1),
            provider("beta", true, 1, true, 1),
            provider("disabled", false, 1, false, 1),
        ];

        let preview = station_routing_preview(&providers, &HashMap::new(), None, None);

        assert_eq!(preview.mode, "auto(single-level fallback)");
        assert!(preview.order[0].starts_with("beta"));
        assert!(preview.order[1].starts_with("alpha"));
        assert_eq!(preview.skipped, vec!["disabled: disabled"]);
    }

    #[test]
    fn station_routing_preview_sorts_multi_level_and_active_tiebreak() {
        let providers = vec![
            provider("alpha", true, 2, false, 1),
            provider("beta", true, 1, false, 1),
            provider("zeta", true, 2, true, 1),
        ];

        let preview = station_routing_preview(&providers, &HashMap::new(), None, None);

        assert_eq!(preview.mode, "auto(level fallback)");
        assert!(preview.order[0].starts_with("beta"));
        assert!(preview.order[1].starts_with("zeta"));
        assert!(preview.order[2].starts_with("alpha"));
    }

    #[test]
    fn station_routing_preview_applies_runtime_meta_overrides() {
        let providers = vec![
            provider("alpha", true, 3, false, 1),
            provider("beta", true, 3, false, 1),
        ];
        let overrides = HashMap::from([
            ("alpha".to_string(), (Some(false), Some(1))),
            ("beta".to_string(), (None, Some(2))),
        ]);

        let preview = station_routing_preview(&providers, &overrides, None, None);

        assert_eq!(preview.order, vec!["beta [L2, upstreams=1]"]);
        assert_eq!(preview.skipped, vec!["alpha: disabled"]);
    }

    #[test]
    fn station_routing_preview_marks_pinned_targets() {
        let providers = vec![provider("alpha", false, 1, false, 0)];

        let preview = station_routing_preview(&providers, &HashMap::new(), Some("alpha"), None);

        assert_eq!(preview.mode, "pinned(session)=alpha");
        assert_eq!(preview.order, vec!["alpha [L1, pinned, upstreams=0]"]);
        assert!(preview.skipped.is_empty());
    }
}
