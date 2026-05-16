use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::dashboard_core::{
    StationRetryBoundary, StationRoutingCandidate, StationRoutingMode, StationRoutingPosture,
    StationRoutingPostureInput, StationRoutingSkipReason, StationRoutingSource,
    build_station_routing_posture, summarize_recent_retry_observations,
};
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    Palette, Snapshot, balance_amount_brief_lang, balance_snapshot_status_label_lang,
    balance_snapshot_status_style, format_age, now_ms, shorten, shorten_middle,
    station_balance_brief_lang,
};
use crate::tui::state::UiState;
use crate::tui::{Language, ProviderOption};

mod route_graph;
use route_graph::*;

fn station_routing_posture(
    providers: &[ProviderOption],
    station_meta_overrides: &HashMap<String, (Option<bool>, Option<u8>)>,
    lb_view: &HashMap<String, crate::state::LbConfigView>,
    provider_balances: &HashMap<String, Vec<crate::state::ProviderBalanceSnapshot>>,
    session_override: Option<&str>,
    global_station_override: Option<&str>,
    retry: Option<&crate::config::ResolvedRetryConfig>,
) -> StationRoutingPosture {
    let candidates = providers
        .iter()
        .map(|provider| {
            let lb = lb_view.get(provider.name.as_str());
            let (enabled, level) =
                effective_station_enabled_level(provider, station_meta_overrides);
            StationRoutingCandidate {
                name: provider.name.clone(),
                alias: provider.alias.clone(),
                level,
                enabled,
                active: provider.active,
                upstreams: lb
                    .map(|view| view.upstreams.len())
                    .or(Some(provider.upstreams.len())),
                runtime_state: crate::state::RuntimeConfigState::Normal,
                has_cooldown: lb.is_some_and(|view| {
                    view.upstreams
                        .iter()
                        .any(|upstream| upstream.cooldown_remaining_secs.is_some())
                }),
                any_usage_exhausted: lb.is_some_and(|view| {
                    view.upstreams
                        .iter()
                        .any(|upstream| upstream.usage_exhausted)
                }),
                all_usage_exhausted: lb.is_some_and(|view| {
                    !view.upstreams.is_empty()
                        && view
                            .upstreams
                            .iter()
                            .all(|upstream| upstream.usage_exhausted)
                }),
                balance: crate::dashboard_core::StationRoutingBalanceSummary::from_snapshots(
                    provider_balances
                        .get(provider.name.as_str())
                        .map(Vec::as_slice),
                ),
            }
        })
        .collect::<Vec<_>>();
    let configured_active_station = providers
        .iter()
        .find(|provider| provider.active)
        .map(|provider| provider.name.as_str());

    build_station_routing_posture(StationRoutingPostureInput {
        stations: &candidates,
        session_station_override: session_override,
        global_station_override,
        configured_active_station,
        session_pin_count: 0,
        retry,
    })
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

fn format_routing_source_lang(source: &StationRoutingSource, lang: Language) -> String {
    match source {
        StationRoutingSource::SessionPin(station) => match lang {
            Language::Zh => format!("会话 pin={station}"),
            Language::En => format!("session pin={station}"),
        },
        StationRoutingSource::GlobalPin(station) => match lang {
            Language::Zh => format!("全局 pin={station}"),
            Language::En => format!("global pin={station}"),
        },
        StationRoutingSource::ConfiguredActiveStation(station) => match lang {
            Language::Zh => format!("配置活跃站点={station}"),
            Language::En => format!("configured active={station}"),
        },
        StationRoutingSource::Auto => i18n::label(lang, "auto").to_string(),
    }
}

fn format_routing_mode_lang(mode: StationRoutingMode, lang: Language) -> &'static str {
    match mode {
        StationRoutingMode::PinnedStation => i18n::label(lang, "pinned"),
        StationRoutingMode::AutoLevelFallback => i18n::label(lang, "auto(level fallback)"),
        StationRoutingMode::AutoSingleLevelFallback => {
            i18n::label(lang, "auto(single-level fallback)")
        }
    }
}

#[cfg(test)]
fn format_routing_order_hint(mode: StationRoutingMode) -> &'static str {
    format_routing_order_hint_lang(mode, Language::En)
}

fn format_routing_order_hint_lang(mode: StationRoutingMode, lang: Language) -> &'static str {
    match mode {
        StationRoutingMode::PinnedStation => i18n::label(
            lang,
            "pinned target only; breaker_open / empty upstreams block.",
        ),
        StationRoutingMode::AutoLevelFallback => i18n::label(
            lang,
            "known fully exhausted stations are demoted by default; provider-level exceptions only show balance/quota.",
        ),
        StationRoutingMode::AutoSingleLevelFallback => i18n::label(
            lang,
            "known fully exhausted stations are demoted by default unless a provider opts out of routing trust.",
        ),
    }
}

fn format_retry_boundary_lang(boundary: StationRetryBoundary, lang: Language) -> String {
    match boundary {
        StationRetryBoundary::Unknown => {
            i18n::label(lang, "resolved policy unavailable").to_string()
        }
        StationRetryBoundary::CrossStationBeforeFirstOutput {
            provider_max_attempts,
        } => match lang {
            Language::Zh => {
                format!("provider failover x{provider_max_attempts}；首个输出前允许跨站点")
            }
            Language::En => format!(
                "provider failover x{provider_max_attempts}; cross-station before first output"
            ),
        },
        StationRetryBoundary::CurrentStationFirst {
            provider_strategy,
            provider_max_attempts,
        } => match lang {
            Language::Zh => {
                format!("provider {provider_strategy:?} x{provider_max_attempts}；选中站点优先")
                    .to_ascii_lowercase()
            }
            Language::En => format!(
                "provider {provider_strategy:?} x{provider_max_attempts}; selected station first"
            )
            .to_ascii_lowercase(),
        },
        StationRetryBoundary::NextRequestOnly => match lang {
            Language::Zh => "provider x1；下次路由请求自动切换".to_string(),
            Language::En => "provider x1; auto switch on next routed request".to_string(),
        },
    }
}

#[cfg(test)]
fn format_routing_candidate(candidate: &StationRoutingCandidate) -> String {
    format_routing_candidate_lang(candidate, Language::En)
}

fn format_routing_candidate_lang(candidate: &StationRoutingCandidate, lang: Language) -> String {
    let mut parts = vec![format!("L{}", candidate.level.clamp(1, 10))];
    if candidate.active {
        parts.push(i18n::label(lang, "active").to_string());
    }
    match candidate.upstreams {
        Some(upstreams) => parts.push(format!("{}={upstreams}", i18n::label(lang, "upstreams"))),
        None => parts.push(format!("{}=?", i18n::label(lang, "upstreams"))),
    }
    if candidate.has_cooldown {
        parts.push(i18n::label(lang, "cooldown").to_string());
    }
    if candidate.all_usage_exhausted {
        parts.push(format!(
            "{}={}",
            i18n::label(lang, "quota"),
            i18n::label(lang, "all_exhausted")
        ));
    } else if candidate.any_usage_exhausted {
        parts.push(format!(
            "{}={}",
            i18n::label(lang, "quota"),
            i18n::label(lang, "partial_exhausted")
        ));
    }
    if !candidate.balance.is_empty() {
        parts.push(format_routing_balance_lang(candidate, lang));
    }

    format!("{} [{}]", candidate.name, parts.join(", "))
}

fn format_routing_balance_lang(candidate: &StationRoutingCandidate, lang: Language) -> String {
    if lang == Language::En {
        return format_routing_balance_en(candidate);
    }

    let balance = &candidate.balance;
    let mut parts = Vec::new();
    if balance.routing_snapshots == 0 {
        if balance.exhausted > 0 {
            parts.push("耗尽但不参与路由".to_string());
        }
    } else if balance.routing_exhausted == balance.routing_snapshots {
        parts.push("路由可见全部耗尽".to_string());
    } else if balance.routing_exhausted > 0 {
        parts.push(format!(
            "耗尽={}/{}",
            balance.routing_exhausted, balance.routing_snapshots
        ));
    }
    if balance.routing_ignored_exhausted > 0 {
        parts.push(format!(
            "路由忽略耗尽={}",
            balance.routing_ignored_exhausted
        ));
    }
    if balance.stale > 0 {
        parts.push(format!("{}={}", i18n::label(lang, "stale"), balance.stale));
    }
    let unknown = balance.unknown + balance.error;
    if unknown > 0 {
        parts.push(format!("{}={unknown}", i18n::label(lang, "unknown")));
    }
    if parts.is_empty() {
        format!(
            "{}={}({})",
            i18n::label(lang, "balance"),
            i18n::label(lang, "ok"),
            balance.snapshots
        )
    } else {
        format!("{}={}", i18n::label(lang, "balance"), parts.join("/"))
    }
}

fn format_routing_balance_en(candidate: &StationRoutingCandidate) -> String {
    let balance = &candidate.balance;
    let mut parts = Vec::new();
    if balance.routing_snapshots == 0 {
        if balance.exhausted > 0 {
            parts.push("exhausted_untrusted".to_string());
        }
    } else if balance.routing_exhausted == balance.routing_snapshots {
        parts.push("exhausted_all".to_string());
    } else if balance.routing_exhausted > 0 {
        parts.push(format!(
            "exhausted={}/{}",
            balance.routing_exhausted, balance.routing_snapshots
        ));
    }
    if balance.routing_ignored_exhausted > 0 {
        parts.push(format!(
            "ignored_for_routing={}",
            balance.routing_ignored_exhausted
        ));
    }
    if balance.stale > 0 {
        parts.push(format!("stale={}", balance.stale));
    }
    let unknown = balance.unknown + balance.error;
    if unknown > 0 {
        parts.push(format!("unknown={unknown}"));
    }
    if parts.is_empty() {
        format!("balance=ok({})", balance.snapshots)
    } else {
        format!("balance={}", parts.join("/"))
    }
}

fn format_skipped_station_lang(
    skipped: &crate::dashboard_core::StationRoutingSkipped,
    lang: Language,
) -> String {
    format!(
        "{}: {}",
        skipped.station_name,
        skipped
            .reasons
            .iter()
            .map(|reason| format_skip_reason_lang(reason, lang))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn format_skip_reason_lang(reason: &StationRoutingSkipReason, lang: Language) -> String {
    match reason {
        StationRoutingSkipReason::Disabled => i18n::label(lang, "disabled").to_string(),
        StationRoutingSkipReason::RuntimeState(state) => match lang {
            Language::Zh => format!("状态={state:?}").to_ascii_lowercase(),
            Language::En => format!("state={state:?}").to_ascii_lowercase(),
        },
        StationRoutingSkipReason::NoRoutableUpstreams => {
            i18n::label(lang, "no_upstreams").to_string()
        }
        StationRoutingSkipReason::MissingPinnedTarget => {
            i18n::label(lang, "missing_pinned_station").to_string()
        }
        StationRoutingSkipReason::BreakerOpenBlocksPinned => {
            i18n::label(lang, "breaker_open_blocks_pin").to_string()
        }
    }
}

fn format_runtime_selected_route(
    explain: &crate::routing_explain::RoutingExplainResponse,
) -> String {
    match explain.selected_route.as_ref() {
        Some(selected) => {
            let compatibility = format_runtime_compatibility(selected.compatibility.as_ref());
            format!(
                "selected={} endpoint={} {} path={}",
                selected.provider_id,
                selected.endpoint_id,
                compatibility,
                selected.route_path.join(" > ")
            )
        }
        None => "selected=<none>".to_string(),
    }
}

fn format_runtime_candidate(candidate: &crate::routing_explain::RoutingExplainCandidate) -> String {
    let marker = if candidate.selected { "*" } else { " " };
    let compatibility = format_runtime_compatibility(candidate.compatibility.as_ref());
    format!(
        "{} {} endpoint={} {} skip={}",
        marker,
        candidate.provider_id,
        candidate.endpoint_id,
        compatibility,
        format_runtime_skip_reasons(&candidate.skip_reasons)
    )
}

fn format_runtime_compatibility(
    compatibility: Option<&crate::routing_explain::RoutingExplainCompatibility>,
) -> String {
    compatibility
        .map(|compatibility| {
            format!(
                "compat_station={} upstream#{}",
                compatibility.station_name, compatibility.upstream_index
            )
        })
        .unwrap_or_else(|| "compatibility=-".to_string())
}

fn format_runtime_skip_reasons(
    reasons: &[crate::routing_explain::RoutingExplainSkipReason],
) -> String {
    if reasons.is_empty() {
        return "-".to_string();
    }
    reasons
        .iter()
        .map(crate::routing_explain::RoutingExplainSkipReason::code)
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn render_stations_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    if ui.uses_route_graph_routing() {
        render_route_graph_routing_page(f, p, ui, snapshot, area);
        return;
    }
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);
    let now = now_ms();

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
            format!("{}  ({}: {selected_session})", l("Stations"), l("session")),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new([
        l("Lvl"),
        l("Name"),
        l("On"),
        l("Up"),
        l("Balance/Quota"),
        l("Health"),
    ])
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

            let on = if enabled { l("on") } else { l("off") };
            let up = cfg.upstreams.len().to_string();
            let balance = station_balance_brief_lang(
                &snapshot.provider_balances,
                cfg.name.as_str(),
                18,
                lang,
            );
            let health = if let Some(st) = snapshot.health_checks.get(cfg.name.as_str())
                && !st.done
            {
                if st.cancel_requested {
                    format!("{} {}/{}", l("cancel"), st.completed, st.total.max(1))
                } else {
                    format!("{} {}/{}", l("run"), st.completed, st.total.max(1))
                }
            } else if let Some(st) = snapshot.health_checks.get(cfg.name.as_str())
                && st.done
                && st.canceled
            {
                l("canceled").to_string()
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
                                format!("{ok}/{total} {}", l("ok"))
                            }
                        } else {
                            let status = h.upstreams.iter().filter_map(|u| u.status_code).next();
                            if let Some(code) = status {
                                format!("{} {code}", l("err"))
                            } else {
                                l("err").to_string()
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
                on.to_string(),
                up,
                balance,
                health,
            ])
            .style(style)
            .height(1)
        })
        .collect::<Vec<_>>();

    let table_visible_rows = usize::from(left_block.inner(columns[0]).height.saturating_sub(1));
    ui.sync_stations_table_viewport(providers.len(), table_visible_rows);

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(18),
            Constraint::Length(8),
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
        .map(|c| {
            format!(
                "{}: {} (L{})",
                l("Station details"),
                c.name,
                c.level.clamp(1, 10)
            )
        })
        .unwrap_or_else(|| l("Station details").to_string());

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
            match lang {
                Language::Zh => "（覆盖）",
                Language::En => " (override)",
            }
        } else {
            ""
        };
        let enabled_note = if enabled_ovr.is_some() {
            match lang {
                Language::Zh => "（覆盖）",
                Language::En => " (override)",
            }
        } else {
            ""
        };

        if let Some(alias) = cfg.alias.as_deref()
            && !alias.trim().is_empty()
        {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("alias")), Style::default().fg(p.muted)),
                Span::styled(alias.to_string(), Style::default().fg(p.text)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("enabled")), Style::default().fg(p.muted)),
            Span::styled(
                format!("{}{enabled_note}", if enabled { l("yes") } else { l("no") }),
                Style::default().fg(if enabled { p.good } else { p.warn }),
            ),
            Span::raw("   "),
            Span::styled(format!("{}: ", l("Lvl")), Style::default().fg(p.muted)),
            Span::styled(
                format!("L{level}{level_note}"),
                Style::default().fg(p.muted),
            ),
            Span::raw("   "),
            Span::styled(format!("{}: ", l("active")), Style::default().fg(p.muted)),
            Span::styled(
                if cfg.active { l("yes") } else { l("no") },
                Style::default().fg(if cfg.active { p.accent } else { p.muted }),
            ),
        ]));

        let routing = station_routing_posture(
            providers,
            &snapshot.station_meta_overrides,
            &snapshot.lb_view,
            &snapshot.provider_balances,
            session_override,
            global_station_override,
            ui.last_runtime_retry.as_ref(),
        );
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("routing")), Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "{} · {}",
                    format_routing_source_lang(&routing.source, lang),
                    format_routing_mode_lang(routing.mode, lang)
                ),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("retry")), Style::default().fg(p.muted)),
            Span::styled(
                shorten_middle(
                    &format_retry_boundary_lang(routing.retry_boundary, lang),
                    96,
                ),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", l("order_rule")),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                shorten_middle(format_routing_order_hint_lang(routing.mode, lang), 96),
                Style::default().fg(p.muted),
            ),
        ]));
        let observations = summarize_recent_retry_observations(&snapshot.recent);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", l("Recent sample")),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                format!(
                    "retry={} same={} cross={} fast={}",
                    observations.recent_retried_requests,
                    observations.recent_same_station_retries,
                    observations.recent_cross_station_failovers,
                    observations.recent_fast_mode_requests
                ),
                Style::default().fg(p.muted),
            ),
        ]));
        if !routing.eligible_candidates.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("order")), Style::default().fg(p.muted)),
                Span::styled(
                    shorten_middle(
                        &routing
                            .eligible_candidates
                            .iter()
                            .map(|candidate| format_routing_candidate_lang(candidate, lang))
                            .collect::<Vec<_>>()
                            .join(" > "),
                        96,
                    ),
                    Style::default().fg(p.text),
                ),
            ]));
        }
        if !routing.skipped.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("skipped")), Style::default().fg(p.muted)),
                Span::styled(
                    shorten_middle(
                        &routing
                            .skipped
                            .iter()
                            .map(|skipped| format_skipped_station_lang(skipped, lang))
                            .collect::<Vec<_>>()
                            .join(" | "),
                        96,
                    ),
                    Style::default().fg(p.muted),
                ),
            ]));
        }
        if let Some(explain) = ui.routing_explain.as_ref() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}: ", l("Runtime route")),
                    Style::default().fg(p.muted),
                ),
                Span::styled(
                    shorten_middle(&format_runtime_selected_route(explain), 96),
                    Style::default().fg(p.text),
                ),
            ]));
            let candidates = explain
                .candidates
                .iter()
                .map(format_runtime_candidate)
                .collect::<Vec<_>>()
                .join(" | ");
            if !candidates.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{}: ", l("Runtime candidates")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(
                        shorten_middle(&candidates, 96),
                        Style::default().fg(p.muted),
                    ),
                ]));
            }
        }

        if let Some(st) = snapshot.health_checks.get(cfg.name.as_str()) {
            let status = if !st.done {
                if st.cancel_requested {
                    format!("{} {}/{}", l("cancel"), st.completed, st.total.max(1))
                } else {
                    format!("{} {}/{}", l("running"), st.completed, st.total.max(1))
                }
            } else if st.canceled {
                l("canceled").to_string()
            } else {
                l("done").to_string()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}: ", l("health_check")),
                    Style::default().fg(p.muted),
                ),
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
            let age = format_age(now, Some(health.checked_at_ms));
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("health")), Style::default().fg(p.muted)),
                Span::styled(
                    match lang {
                        crate::tui::Language::Zh => format!("{age} 前检查"),
                        crate::tui::Language::En => format!("checked {age} ago"),
                    },
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
                        if ok { l("ok") } else { l("err") },
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
                Span::styled(format!("{}: ", l("health")), Style::default().fg(p.muted)),
                Span::styled(
                    i18n::text(lang, msg::NOT_CHECKED),
                    Style::default().fg(p.muted).add_modifier(Modifier::DIM),
                ),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Balance / quota"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if let Some(balances) = snapshot.provider_balances.get(cfg.name.as_str()) {
            if balances.is_empty() {
                lines.push(Line::from(Span::styled(
                    i18n::text(lang, msg::NONE_PARENS),
                    Style::default().fg(p.muted),
                )));
            } else {
                for balance in balances.iter().take(12) {
                    let idx = balance
                        .upstream_index
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    lines.push(Line::from(vec![
                        Span::styled(format!("{idx:>2}. "), Style::default().fg(p.muted)),
                        Span::styled(
                            shorten_middle(&balance.provider_id, 20),
                            Style::default().fg(p.text),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            balance_snapshot_status_label_lang(balance, lang),
                            balance_snapshot_status_style(p, balance),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            shorten_middle(
                                &balance_amount_brief_lang(balance, lang)
                                    .unwrap_or_else(|| balance.amount_summary()),
                                56,
                            ),
                            Style::default().fg(p.muted),
                        ),
                    ]));
                    if let Some(err) = balance.error.as_deref()
                        && !err.trim().is_empty()
                    {
                        lines.push(Line::from(vec![
                            Span::raw("     "),
                            Span::styled(
                                format!("{}: {}", l("balance lookup failed"), shorten(err, 56)),
                                Style::default().fg(p.muted),
                            ),
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
                i18n::text(lang, msg::NONE_PARENS),
                Style::default().fg(p.muted),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Upstreams"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if cfg.upstreams.is_empty() {
            lines.push(Line::from(Span::styled(
                i18n::text(lang, msg::NONE_PARENS),
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
            l("Actions"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(i18n::text(
            ui.language,
            msg::ROUTING_ACTION_PROVIDER_DETAILS,
        )));
        lines.extend(match lang {
            crate::tui::Language::Zh => vec![
                Line::from("  Enter        设置全局 pin"),
                Line::from("  Backspace    清除全局 pin（auto）"),
                Line::from("  r            routing 编辑器（策略/顺序/标签）"),
                Line::from("  o            将会话覆盖设置为选中站点"),
                Line::from("  O            清除会话覆盖"),
            ],
            crate::tui::Language::En => vec![
                Line::from("  Enter        set global pin"),
                Line::from("  Backspace    clear global pin (auto)"),
                Line::from("  r            routing editor (policy/order/tags)"),
                Line::from("  o            set session override to selected station"),
                Line::from("  O            clear session override"),
            ],
        });
        lines.extend(match lang {
            crate::tui::Language::Zh => vec![
                Line::from("  h            检查选中站点健康"),
                Line::from("  H            检查全部站点健康"),
                Line::from("  c            取消健康检查（选中）"),
                Line::from("  C            取消健康检查（全部）"),
            ],
            crate::tui::Language::En => vec![
                Line::from("  h            health check selected station"),
                Line::from("  H            health check all stations"),
                Line::from("  c            cancel health check (selected)"),
                Line::from("  C            cancel health check (all)"),
            ],
        });
    } else {
        lines.push(Line::from(Span::styled(
            l("No stations available."),
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
mod tests;
