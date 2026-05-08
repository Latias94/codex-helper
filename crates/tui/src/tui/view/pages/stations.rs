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
use crate::state::BalanceSnapshotStatus;
use crate::tui::ProviderOption;
use crate::tui::model::{Palette, Snapshot, format_age, now_ms, shorten, shorten_middle};
use crate::tui::state::UiState;

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

fn format_routing_source(source: &StationRoutingSource) -> String {
    match source {
        StationRoutingSource::SessionPin(station) => format!("session pin={station}"),
        StationRoutingSource::GlobalPin(station) => format!("global pin={station}"),
        StationRoutingSource::ConfiguredActiveStation(station) => {
            format!("configured active={station}")
        }
        StationRoutingSource::Auto => "auto".to_string(),
    }
}

fn format_routing_mode(mode: StationRoutingMode) -> &'static str {
    match mode {
        StationRoutingMode::PinnedStation => "pinned",
        StationRoutingMode::AutoLevelFallback => "auto(level fallback)",
        StationRoutingMode::AutoSingleLevelFallback => "auto(single-level fallback)",
    }
}

fn format_routing_order_hint(mode: StationRoutingMode) -> &'static str {
    match mode {
        StationRoutingMode::PinnedStation => {
            "pinned target only; breaker_open / empty upstreams block."
        }
        StationRoutingMode::AutoLevelFallback => {
            "known fully exhausted stations are demoted by default; provider-level exceptions only show balance."
        }
        StationRoutingMode::AutoSingleLevelFallback => {
            "known fully exhausted stations are demoted by default unless a provider opts out of routing trust."
        }
    }
}

fn format_retry_boundary(boundary: StationRetryBoundary) -> String {
    match boundary {
        StationRetryBoundary::Unknown => "resolved policy unavailable".to_string(),
        StationRetryBoundary::CrossStationBeforeFirstOutput {
            provider_max_attempts,
        } => {
            format!("provider failover x{provider_max_attempts}; cross-station before first output")
        }
        StationRetryBoundary::CurrentStationFirst {
            provider_strategy,
            provider_max_attempts,
        } => format!(
            "provider {provider_strategy:?} x{provider_max_attempts}; selected station first"
        )
        .to_ascii_lowercase(),
        StationRetryBoundary::NextRequestOnly => {
            "provider x1; auto switch on next routed request".to_string()
        }
    }
}

fn format_routing_candidate(candidate: &StationRoutingCandidate) -> String {
    let mut parts = vec![format!("L{}", candidate.level.clamp(1, 10))];
    if candidate.active {
        parts.push("active".to_string());
    }
    match candidate.upstreams {
        Some(upstreams) => parts.push(format!("upstreams={upstreams}")),
        None => parts.push("upstreams=?".to_string()),
    }
    if candidate.has_cooldown {
        parts.push("cooldown".to_string());
    }
    if candidate.all_usage_exhausted {
        parts.push("quota=all_exhausted".to_string());
    } else if candidate.any_usage_exhausted {
        parts.push("quota=partial_exhausted".to_string());
    }
    if !candidate.balance.is_empty() {
        parts.push(format_routing_balance(candidate));
    }

    format!("{} [{}]", candidate.name, parts.join(", "))
}

fn format_routing_balance(candidate: &StationRoutingCandidate) -> String {
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
    if balance.error > 0 {
        parts.push(format!("error={}", balance.error));
    }
    if balance.stale > 0 {
        parts.push(format!("stale={}", balance.stale));
    }
    if balance.unknown > 0 {
        parts.push(format!("unknown={}", balance.unknown));
    }
    if parts.is_empty() {
        format!("balance=ok({})", balance.snapshots)
    } else {
        format!("balance={}", parts.join("/"))
    }
}

fn balance_amount_brief(snapshot: &crate::state::ProviderBalanceSnapshot) -> Option<String> {
    if let Some(total) = snapshot
        .total_balance_usd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(format!("${total}"));
    }

    match (
        snapshot
            .monthly_spent_usd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        snapshot
            .monthly_budget_usd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    ) {
        (Some(spent), Some(budget)) => Some(format!("${spent}/${budget}")),
        _ => snapshot
            .subscription_balance_usd
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("sub ${value}"))
            .or_else(|| {
                snapshot
                    .paygo_balance_usd
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| format!("paygo ${value}"))
            }),
    }
}

fn station_balance_cell(
    provider_balances: &HashMap<String, Vec<crate::state::ProviderBalanceSnapshot>>,
    station_name: &str,
) -> String {
    let Some(balances) = provider_balances.get(station_name) else {
        return "-".to_string();
    };
    if balances.is_empty() {
        return "-".to_string();
    }

    if balances.len() == 1 {
        let snapshot = &balances[0];
        let amount = balance_amount_brief(snapshot);
        return match snapshot.status {
            BalanceSnapshotStatus::Ok => amount.unwrap_or_else(|| "ok".to_string()),
            BalanceSnapshotStatus::Exhausted => amount
                .map(|value| format!("exh {value}"))
                .unwrap_or_else(|| "exh".to_string()),
            BalanceSnapshotStatus::Stale => amount
                .map(|value| format!("stale {value}"))
                .unwrap_or_else(|| "stale".to_string()),
            BalanceSnapshotStatus::Error => "err".to_string(),
            BalanceSnapshotStatus::Unknown => amount
                .map(|value| format!("unk {value}"))
                .unwrap_or_else(|| "unk".to_string()),
        };
    }

    let mut ok = 0usize;
    let mut stale = 0usize;
    let mut exhausted = 0usize;
    let mut error = 0usize;
    let mut unknown = 0usize;
    for snapshot in balances {
        match snapshot.status {
            BalanceSnapshotStatus::Ok => ok += 1,
            BalanceSnapshotStatus::Stale => stale += 1,
            BalanceSnapshotStatus::Exhausted => exhausted += 1,
            BalanceSnapshotStatus::Error => error += 1,
            BalanceSnapshotStatus::Unknown => unknown += 1,
        }
    }

    let total = balances.len();
    if error > 0 {
        return format!("err {error}/{total}");
    }
    if exhausted > 0 {
        return format!("exh {exhausted}/{total}");
    }
    if stale > 0 && ok == 0 {
        return format!("stale {stale}/{total}");
    }
    if ok > 0 {
        if let Some(amount) = balances
            .iter()
            .find(|snapshot| snapshot.status == BalanceSnapshotStatus::Ok)
            .and_then(balance_amount_brief)
        {
            return amount;
        }
        return format!("ok {ok}/{total}");
    }
    if stale > 0 {
        return format!("stale {stale}/{total}");
    }
    if unknown > 0 {
        return format!("unk {unknown}/{total}");
    }

    "-".to_string()
}

fn format_skipped_station(skipped: &crate::dashboard_core::StationRoutingSkipped) -> String {
    format!(
        "{}: {}",
        skipped.station_name,
        skipped
            .reasons
            .iter()
            .map(format_skip_reason)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn format_skip_reason(reason: &StationRoutingSkipReason) -> String {
    match reason {
        StationRoutingSkipReason::Disabled => "disabled".to_string(),
        StationRoutingSkipReason::RuntimeState(state) => {
            format!("state={state:?}").to_ascii_lowercase()
        }
        StationRoutingSkipReason::NoRoutableUpstreams => "no_upstreams".to_string(),
        StationRoutingSkipReason::MissingPinnedTarget => "missing_pinned_station".to_string(),
        StationRoutingSkipReason::BreakerOpenBlocksPinned => "breaker_open_blocks_pin".to_string(),
    }
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
            format!("Stations  (session: {selected_session})"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new(["Lvl", "Name", "On", "Up", "Balance", "Health"])
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

            let on = if enabled { "on" } else { "off" };
            let up = cfg.upstreams.len().to_string();
            let balance = station_balance_cell(&snapshot.provider_balances, cfg.name.as_str());
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
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(10),
            Constraint::Length(10),
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
            Span::styled("routing: ", Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "{} · {}",
                    format_routing_source(&routing.source),
                    format_routing_mode(routing.mode)
                ),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("retry: ", Style::default().fg(p.muted)),
            Span::styled(
                shorten_middle(&format_retry_boundary(routing.retry_boundary), 96),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("order_rule: ", Style::default().fg(p.muted)),
            Span::styled(
                shorten_middle(format_routing_order_hint(routing.mode), 96),
                Style::default().fg(p.muted),
            ),
        ]));
        let observations = summarize_recent_retry_observations(&snapshot.recent);
        lines.push(Line::from(vec![
            Span::styled("recent: ", Style::default().fg(p.muted)),
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
                Span::styled("order: ", Style::default().fg(p.muted)),
                Span::styled(
                    shorten_middle(
                        &routing
                            .eligible_candidates
                            .iter()
                            .map(format_routing_candidate)
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
                Span::styled("skipped: ", Style::default().fg(p.muted)),
                Span::styled(
                    shorten_middle(
                        &routing
                            .skipped
                            .iter()
                            .map(format_skipped_station)
                            .collect::<Vec<_>>()
                            .join(" | "),
                        96,
                    ),
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
            let age = format_age(now, Some(health.checked_at_ms));
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
                    lines.push(Line::from(vec![
                        Span::styled(format!("{idx:>2}. "), Style::default().fg(p.muted)),
                        Span::styled(
                            shorten_middle(&balance.provider_id, 20),
                            Style::default().fg(p.text),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            balance.status.as_str(),
                            balance_status_style(p, balance.status),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            shorten_middle(&balance.amount_summary(), 56),
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
        let lb_view = HashMap::new();

        let provider_balances = HashMap::new();

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );

        assert_eq!(preview.mode, StationRoutingMode::AutoSingleLevelFallback);
        assert_eq!(preview.eligible_candidates[0].name, "beta");
        assert_eq!(preview.eligible_candidates[1].name, "alpha");
        assert_eq!(preview.skipped[0].station_name, "disabled");
        assert_eq!(
            preview.skipped[0].reasons,
            vec![StationRoutingSkipReason::Disabled]
        );
    }

    #[test]
    fn station_routing_preview_sorts_multi_level_and_active_tiebreak() {
        let providers = vec![
            provider("alpha", true, 2, false, 1),
            provider("beta", true, 1, false, 1),
            provider("zeta", true, 2, true, 1),
        ];
        let lb_view = HashMap::new();

        let provider_balances = HashMap::new();

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );

        assert_eq!(preview.mode, StationRoutingMode::AutoLevelFallback);
        assert_eq!(preview.eligible_candidates[0].name, "beta");
        assert_eq!(preview.eligible_candidates[1].name, "zeta");
        assert_eq!(preview.eligible_candidates[2].name, "alpha");
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
        let lb_view = HashMap::new();

        let provider_balances = HashMap::new();

        let preview = station_routing_posture(
            &providers,
            &overrides,
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );

        assert_eq!(preview.eligible_candidates[0].name, "beta");
        assert_eq!(preview.eligible_candidates[0].level, 2);
        assert_eq!(preview.skipped[0].station_name, "alpha");
        assert_eq!(
            preview.skipped[0].reasons,
            vec![StationRoutingSkipReason::Disabled]
        );
    }

    #[test]
    fn station_routing_preview_marks_pinned_targets() {
        let providers = vec![provider("alpha", false, 1, false, 0)];
        let lb_view = HashMap::new();
        let provider_balances = HashMap::new();

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            Some("alpha"),
            None,
            None,
        );

        assert_eq!(preview.mode, StationRoutingMode::PinnedStation);
        assert!(matches!(
            preview.source,
            StationRoutingSource::SessionPin(ref station) if station == "alpha"
        ));
        assert!(preview.eligible_candidates.is_empty());
        assert_eq!(
            preview.skipped[0].reasons,
            vec![StationRoutingSkipReason::NoRoutableUpstreams]
        );
    }

    #[test]
    fn station_routing_preview_marks_balance_warnings() {
        let providers = vec![provider("alpha", true, 1, true, 1)];
        let lb_view = HashMap::new();
        let provider_balances = HashMap::from([(
            "alpha".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Exhausted,
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]);

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );
        let label = format_routing_candidate(&preview.eligible_candidates[0]);

        assert!(label.contains("balance=exhausted_all"));
    }

    #[test]
    fn station_routing_preview_marks_ignored_routing_exhaustion() {
        let providers = vec![provider("alpha", true, 1, true, 1)];
        let lb_view = HashMap::new();
        let provider_balances = HashMap::from([(
            "alpha".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Exhausted,
                exhaustion_affects_routing: false,
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]);

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );
        let label = format_routing_candidate(&preview.eligible_candidates[0]);

        assert!(label.contains("balance=exhausted_untrusted"));
        assert!(label.contains("ignored_for_routing=1"));
    }

    #[test]
    fn station_routing_preview_does_not_treat_unknown_balance_as_ok() {
        let providers = vec![provider("alpha", true, 1, true, 1)];
        let lb_view = HashMap::new();
        let provider_balances = HashMap::from([(
            "alpha".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Unknown,
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]);

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );
        let label = format_routing_candidate(&preview.eligible_candidates[0]);

        assert!(label.contains("balance=unknown=1"));
        assert!(!label.contains("balance=ok"));
    }

    #[test]
    fn routing_order_hint_explains_balance_demotion() {
        let text = format_routing_order_hint(StationRoutingMode::AutoLevelFallback);

        assert!(text.contains("demoted by default"));
        assert!(text.contains("provider-level exceptions"));
    }

    #[test]
    fn station_balance_cell_shows_single_amount() {
        let provider_balances = HashMap::from([(
            "alpha".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Ok,
                total_balance_usd: Some("3.50".to_string()),
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]);

        assert_eq!(station_balance_cell(&provider_balances, "alpha"), "$3.50");
    }

    #[test]
    fn station_balance_cell_summarizes_multi_snapshot_states() {
        let provider_balances = HashMap::from([(
            "alpha".to_string(),
            vec![
                crate::state::ProviderBalanceSnapshot {
                    status: BalanceSnapshotStatus::Exhausted,
                    ..crate::state::ProviderBalanceSnapshot::default()
                },
                crate::state::ProviderBalanceSnapshot {
                    status: BalanceSnapshotStatus::Ok,
                    total_balance_usd: Some("1.00".to_string()),
                    ..crate::state::ProviderBalanceSnapshot::default()
                },
            ],
        )]);

        assert_eq!(station_balance_cell(&provider_balances, "alpha"), "exh 1/2");
    }
}
