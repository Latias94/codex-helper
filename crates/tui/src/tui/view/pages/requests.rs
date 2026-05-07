use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::tui::model::{
    Palette, Snapshot, format_age, now_ms, request_matches_page_filters,
    request_page_focus_session_id, short_sid, shorten, shorten_middle, status_style, usage_line,
};
use crate::tui::state::UiState;

pub(super) fn render_requests_page(
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

    let focused_sid = request_page_focus_session_id(
        snapshot,
        ui.focused_request_session_id.as_deref(),
        ui.selected_session_idx,
    );

    let filtered = snapshot
        .recent
        .iter()
        .filter(|r| {
            request_matches_page_filters(
                r,
                ui.request_page_errors_only,
                ui.request_page_scope_session,
                focused_sid.as_deref(),
            )
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

    let scope_label = if ui.request_page_scope_session {
        focused_sid
            .as_deref()
            .map(|sid| format!("session {}", short_sid(sid, 16)))
            .unwrap_or_else(|| "session".to_string())
    } else {
        "all".to_string()
    };
    let left_title = format!(
        "Requests  (scope: {}, errors_only: {})",
        scope_label,
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

    let header = Row::new(["Age", "St", "Dur", "Att", "Model", "Stn", "Pid", "Path"])
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
            let attempts_n = r.attempt_count();
            let attempts = attempts_n.to_string();
            let model = r.model.as_deref().unwrap_or("-").to_string();
            let cfg = r.station_name.as_deref().unwrap_or("-").to_string();
            let pid = r.provider_id.as_deref().unwrap_or("-").to_string();
            let path = shorten_middle(&r.path, 60);

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
        let observability = r.observability_view();
        let focus_mode = if ui.focused_request_session_id.is_some() {
            "explicit session focus"
        } else {
            "follow selected session"
        };
        lines.push(Line::from(vec![
            Span::styled("scope: ", Style::default().fg(p.muted)),
            Span::styled(
                if ui.request_page_scope_session {
                    focused_sid
                        .as_deref()
                        .map(|sid| short_sid(sid, 20))
                        .unwrap_or_else(|| "-".to_string())
                } else {
                    "all requests".to_string()
                },
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled("mode: ", Style::default().fg(p.muted)),
            Span::styled(focus_mode, Style::default().fg(p.muted)),
        ]));
        lines.push(Line::from(""));
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
            Span::styled("trace: ", Style::default().fg(p.muted)),
            Span::styled(
                r.trace_id
                    .as_deref()
                    .map(|value| shorten_middle(value, 52))
                    .unwrap_or_else(|| format!("request-{}", r.id)),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("method: ", Style::default().fg(p.muted)),
            Span::styled(r.method.clone(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("path: ", Style::default().fg(p.muted)),
            Span::styled(shorten_middle(&r.path, 80), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("model: ", Style::default().fg(p.muted)),
            Span::styled(
                r.model.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("effort: ", Style::default().fg(p.muted)),
            Span::styled(
                r.reasoning_effort.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled("tier: ", Style::default().fg(p.muted)),
            Span::styled(
                request_service_tier_label(r.service_tier.as_deref()),
                Style::default().fg(if r.is_fast_mode() { p.good } else { p.text }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("station: ", Style::default().fg(p.muted)),
            Span::styled(
                r.station_name.as_deref().unwrap_or("-").to_string(),
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
                Span::styled(shorten_middle(u, 80), Style::default().fg(p.text)),
            ]));
        }

        lines.push(Line::from(vec![
            Span::styled("ttfb: ", Style::default().fg(p.muted)),
            Span::styled(
                observability
                    .ttfb_ms
                    .map(|value| format!("{value}ms"))
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled("generation: ", Style::default().fg(p.muted)),
            Span::styled(
                observability
                    .generation_ms
                    .map(|value| format!("{value}ms"))
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled("stream: ", Style::default().fg(p.muted)),
            Span::styled(
                if observability.streaming { "yes" } else { "no" },
                Style::default().fg(p.text),
            ),
        ]));

        if let Some(u) = r.usage.as_ref().filter(|u| u.total_tokens > 0) {
            lines.push(Line::from(vec![
                Span::styled("usage: ", Style::default().fg(p.muted)),
                Span::styled(usage_line(u), Style::default().fg(p.accent)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("cost: ", Style::default().fg(p.muted)),
                Span::styled(
                    r.cost.display_total_with_confidence(),
                    Style::default().fg(p.text),
                ),
            ]));
            if let Some(cost_parts) = request_cost_parts_line(r) {
                lines.push(Line::from(vec![
                    Span::styled("cost_parts: ", Style::default().fg(p.muted)),
                    Span::styled(cost_parts, Style::default().fg(p.muted)),
                ]));
            }

            if let Some(rate) = observability.output_tokens_per_second {
                lines.push(Line::from(vec![
                    Span::styled("out_tok/s: ", Style::default().fg(p.muted)),
                    Span::styled(format!("{rate:.1}"), Style::default().fg(p.text)),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Retry / route chain",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if let Some(retry) = r.retry.as_ref() {
            lines.push(Line::from(vec![
                Span::styled("attempts: ", Style::default().fg(p.muted)),
                Span::styled(r.attempt_count().to_string(), Style::default().fg(p.text)),
            ]));
            let max = 12usize;
            let attempts = retry.route_attempts_or_derived();
            if !attempts.is_empty() {
                for (idx, attempt) in attempts.iter().take(max).enumerate() {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{:>2}. ", idx + 1), Style::default().fg(p.muted)),
                        Span::styled(
                            request_route_attempt_line(attempt),
                            Style::default().fg(p.muted),
                        ),
                    ]));
                }
                if attempts.len() > max {
                    lines.push(Line::from(Span::styled(
                        format!("... +{} more", attempts.len() - max),
                        Style::default().fg(p.muted),
                    )));
                }
            } else {
                for (idx, entry) in retry.upstream_chain.iter().take(max).enumerate() {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{:>2}. ", idx + 1), Style::default().fg(p.muted)),
                        Span::styled(shorten_middle(entry, 120), Style::default().fg(p.muted)),
                    ]));
                }
                if retry.upstream_chain.len() > max {
                    lines.push(Line::from(Span::styled(
                        format!("... +{} more", retry.upstream_chain.len() - max),
                        Style::default().fg(p.muted),
                    )));
                }
            }
        } else {
            lines.push(Line::from(Span::styled(
                "(no retries)",
                Style::default().fg(p.muted),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Keys",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from("  e toggle errors-only"));
        lines.push(Line::from("  s toggle session scope"));
        lines.push(Line::from("  x clear explicit session focus"));
        lines.push(Line::from("  o open session in Sessions"));
        lines.push(Line::from("  h open session in History"));
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

fn request_service_tier_label(value: Option<&str>) -> String {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    match value {
        Some(tier) if tier.eq_ignore_ascii_case("priority") => format!("{tier} (fast)"),
        Some(tier) => tier.to_string(),
        None => "-".to_string(),
    }
}

fn request_cost_parts_line(request: &crate::state::FinishedRequest) -> Option<String> {
    let cost = &request.cost;
    let mut parts = Vec::new();
    if let Some(value) = cost.input_cost_usd.as_deref() {
        parts.push(format!("in=${value}"));
    }
    if let Some(value) = cost.output_cost_usd.as_deref() {
        parts.push(format!("out=${value}"));
    }
    if let Some(value) = cost.cache_read_cost_usd.as_deref() {
        parts.push(format!("read=${value}"));
    }
    if let Some(value) = cost.cache_creation_cost_usd.as_deref() {
        parts.push(format!("create=${value}"));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn request_route_attempt_line(attempt: &crate::logging::RouteAttemptLog) -> String {
    let target = match (
        attempt.station_name.as_deref(),
        attempt.upstream_base_url.as_deref(),
    ) {
        (Some(station), Some(upstream)) => format!("{station}:{}", shorten_middle(upstream, 54)),
        (Some(station), None) => station.to_string(),
        (None, Some(upstream)) => shorten_middle(upstream, 62),
        (None, None) => "-".to_string(),
    };
    let mut parts = vec![attempt.decision.clone()];
    if let Some(provider_id) = attempt.provider_id.as_deref() {
        parts.push(format!("prov={}", shorten_middle(provider_id, 18)));
    }
    if let Some(provider_attempt) = attempt.provider_attempt {
        if let Some(max) = attempt.provider_max_attempts {
            parts.push(format!("p={provider_attempt}/{max}"));
        } else {
            parts.push(format!("p={provider_attempt}"));
        }
    }
    if let Some(upstream_attempt) = attempt.upstream_attempt {
        if let Some(max) = attempt.upstream_max_attempts {
            parts.push(format!("u={upstream_attempt}/{max}"));
        } else {
            parts.push(format!("u={upstream_attempt}"));
        }
    }
    if attempt.skipped {
        parts.push("skipped".to_string());
    }
    if let Some(status_code) = attempt.status_code {
        parts.push(format!("status={status_code}"));
    }
    if let Some(error_class) = attempt.error_class.as_deref() {
        parts.push(format!("class={error_class}"));
    }
    if let Some(model) = attempt.model.as_deref() {
        parts.push(format!("model={}", shorten(model, 22)));
    }
    if let Some(ttfb_ms) = attempt.upstream_headers_ms {
        parts.push(format!("ttfb={ttfb_ms}ms"));
    }
    if let Some(duration_ms) = attempt.duration_ms {
        parts.push(format!("dur={duration_ms}ms"));
    }
    if let Some(cooldown_secs) = attempt.cooldown_secs {
        parts.push(format!("cd={cooldown_secs}s"));
    }
    if !attempt.avoid_for_station.is_empty() {
        let avoid = attempt
            .avoid_for_station
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("avoid=[{avoid}]"));
    } else if let Some(avoided_total) = attempt.avoided_total.filter(|value| *value > 0) {
        if let Some(total) = attempt.total_upstreams {
            parts.push(format!("avoided={avoided_total}/{total}"));
        } else {
            parts.push(format!("avoided={avoided_total}"));
        }
    }
    if let Some(reason) = attempt.reason.as_deref() {
        parts.push(format!("reason={}", shorten_middle(reason, 42)));
    }
    format!("{target}  {}", parts.join(" "))
}
