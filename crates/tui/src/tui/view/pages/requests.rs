use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::tui::i18n;
use crate::tui::model::{
    Palette, Snapshot, duration_short, format_age, format_tok_per_second, now_ms,
    request_cache_hit_rate_label, request_page_focus_is_runtime_observed,
    request_page_focus_session_id, shorten, shorten_middle, status_style, usage_line_lang,
};
use crate::tui::state::UiState;

pub(super) fn render_requests_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    let focused_sid = request_page_focus_session_id(
        snapshot,
        ui.focused_request_session_id.as_deref(),
        ui.selected_session_idx,
    );
    let focused_sid_observed =
        request_page_focus_is_runtime_observed(snapshot, focused_sid.as_deref());

    let filtered = ui.request_page_filtered_indices(snapshot);

    let scope_label = if ui.request_page_scope_session {
        focused_sid
            .as_deref()
            .map(|sid| format!("session {}", sid))
            .unwrap_or_else(|| l("session").to_string())
    } else {
        l("all").to_string()
    };
    let left_title = format!(
        "{}  ({}: {}, {}: {}, {}: {})",
        l("Requests"),
        l("scope"),
        scope_label,
        l("errors_only"),
        if ui.request_page_errors_only {
            l("on")
        } else {
            l("off")
        },
        l("control"),
        ui.request_page_control_filter.label(lang)
    );
    let left_block = Block::default()
        .title(Span::styled(
            left_title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new([
        l("Age"),
        "St",
        l("Dur"),
        "Att",
        "RG",
        l("Hit%"),
        l("Route"),
        l("Path"),
    ])
    .style(Style::default().fg(p.muted))
    .height(1);

    let now = now_ms();
    let rows = filtered
        .iter()
        .filter_map(|idx| snapshot.recent.get(*idx))
        .map(|r| {
            let age = format_age(now, Some(r.ended_at_ms));
            let status = Span::styled(
                r.status_code.to_string(),
                status_style(p, Some(r.status_code)),
            );
            let dur = duration_short(r.duration_ms);
            let attempts_n = r.attempt_count();
            let attempts = attempts_n.to_string();
            let reasoning_guard = request_reasoning_guard_table_label(r);
            let cache_hit = request_cache_hit_rate_label(r);
            let cache_hit_style =
                Style::default().fg(if cache_hit != "-" { p.accent } else { p.muted });
            let route = request_route_table_label(r, 28);
            let path = shorten_middle(&r.path, 48);

            Row::new(vec![
                Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                Cell::from(Line::from(vec![status])),
                Cell::from(Span::styled(dur, Style::default().fg(p.muted))),
                Cell::from(Span::styled(
                    attempts,
                    Style::default().fg(if attempts_n > 1 { p.warn } else { p.muted }),
                )),
                Cell::from(Span::styled(
                    reasoning_guard.clone(),
                    Style::default().fg(if reasoning_guard != "-" {
                        p.warn
                    } else {
                        p.muted
                    }),
                )),
                Cell::from(Span::styled(cache_hit, cache_hit_style)),
                Cell::from(route),
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
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(28),
            Constraint::Min(14),
        ],
    )
    .header(header)
    .block(left_block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, columns[0], &mut ui.request_page_table);

    let selected = filtered
        .get(ui.selected_request_page_idx)
        .and_then(|idx| snapshot.recent.get(*idx));
    let mut lines = Vec::new();
    if let Some(r) = selected {
        let observability = r.observability_view();
        let focus_mode = if ui.focused_request_session_id.is_some() {
            l("explicit session focus")
        } else {
            l("follow selected session")
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("scope")), Style::default().fg(p.muted)),
            Span::styled(
                if ui.request_page_scope_session {
                    focused_sid
                        .as_deref()
                        .map(|sid| sid.to_string())
                        .unwrap_or_else(|| "-".to_string())
                } else {
                    l("all requests").to_string()
                },
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled(format!("{}: ", l("mode")), Style::default().fg(p.muted)),
            Span::styled(focus_mode, Style::default().fg(p.muted)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("status")), Style::default().fg(p.muted)),
            Span::styled(
                r.status_code.to_string(),
                status_style(p, Some(r.status_code)),
            ),
            Span::raw("  "),
            Span::styled(format!("{}: ", l("Dur")), Style::default().fg(p.muted)),
            Span::styled(duration_short(r.duration_ms), Style::default().fg(p.muted)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("trace")), Style::default().fg(p.muted)),
            Span::styled(
                r.trace_id
                    .as_deref()
                    .map(|value| shorten_middle(value, 52))
                    .unwrap_or_else(|| format!("request-{}", r.id)),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("method")), Style::default().fg(p.muted)),
            Span::styled(r.method.clone(), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("path")), Style::default().fg(p.muted)),
            Span::styled(shorten_middle(&r.path, 80), Style::default().fg(p.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("model")), Style::default().fg(p.muted)),
            Span::styled(
                r.model.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.text),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("effort")), Style::default().fg(p.muted)),
            Span::styled(
                r.reasoning_effort.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled(format!("{}: ", l("tier")), Style::default().fg(p.muted)),
            Span::styled(
                request_service_tier_label(r.service_tier.as_deref()),
                Style::default().fg(if r.is_fast_mode() { p.good } else { p.text }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("station")), Style::default().fg(p.muted)),
            Span::styled(
                r.station_name.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.accent),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("provider")), Style::default().fg(p.muted)),
            Span::styled(
                r.provider_id.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.text),
            ),
        ]));
        if let Some(u) = r.upstream_base_url.as_deref() {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("upstream")), Style::default().fg(p.muted)),
                Span::styled(shorten_middle(u, 80), Style::default().fg(p.text)),
            ]));
        }
        if let Some(reasoning_guard) = request_reasoning_guard_detail_line(r, lang) {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}: ", l("reasoning guard")),
                    Style::default().fg(p.muted),
                ),
                Span::styled(reasoning_guard, Style::default().fg(p.warn)),
            ]));
        }

        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("ttfb")), Style::default().fg(p.muted)),
            Span::styled(
                observability
                    .ttfb_ms
                    .map(duration_short)
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{}: ", l("generation")),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                observability
                    .generation_ms
                    .map(duration_short)
                    .unwrap_or_else(|| "-".to_string()),
                Style::default().fg(p.text),
            ),
            Span::raw("  "),
            Span::styled(format!("{}: ", l("stream")), Style::default().fg(p.muted)),
            Span::styled(
                if observability.streaming {
                    l("yes")
                } else {
                    l("no")
                },
                Style::default().fg(p.text),
            ),
        ]));

        if let Some(u) = r.usage.as_ref().filter(|u| u.total_tokens > 0) {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("usage")), Style::default().fg(p.muted)),
                Span::styled(usage_line_lang(u, lang), Style::default().fg(p.accent)),
            ]));
            if let Some(rate) = r.cache_hit_rate() {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{}: ", l("cache hit rate")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(format!("{:.1}%", rate * 100.0), Style::default().fg(p.text)),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("cost")), Style::default().fg(p.muted)),
                Span::styled(
                    r.cost.display_total_with_confidence(),
                    Style::default().fg(p.text),
                ),
            ]));
            if let Some(cost_parts) = request_cost_parts_line(r) {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{}: ", l("cost_parts")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(cost_parts, Style::default().fg(p.muted)),
                ]));
            }

            if let Some(rate) = observability.output_tokens_per_second {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{}: ", l("out_tok/s")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(
                        format_tok_per_second(Some(rate)),
                        Style::default().fg(p.text),
                    ),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Retry / route chain"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if let Some(retry) = r.retry.as_ref() {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("attempts")), Style::default().fg(p.muted)),
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
                match ui.language {
                    crate::tui::Language::Zh => "（无重试）",
                    crate::tui::Language::En => "(no retries)",
                },
                Style::default().fg(p.muted),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Keys"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(match ui.language {
            crate::tui::Language::Zh => "  e 切换仅错误",
            crate::tui::Language::En => "  e toggle errors-only",
        }));
        lines.push(Line::from(match ui.language {
            crate::tui::Language::Zh => "  c 切换控制证据过滤",
            crate::tui::Language::En => "  c cycle provider-control filter",
        }));
        lines.push(Line::from(match ui.language {
            crate::tui::Language::Zh => "  s 切换会话范围",
            crate::tui::Language::En => "  s toggle session scope",
        }));
        lines.push(Line::from(match ui.language {
            crate::tui::Language::Zh => "  x 清除显式会话聚焦",
            crate::tui::Language::En => "  x clear explicit session focus",
        }));
        lines.push(Line::from(match ui.language {
            crate::tui::Language::Zh => "  o 在 Sessions 中打开会话",
            crate::tui::Language::En => "  o open session in Sessions",
        }));
        lines.push(Line::from(match ui.language {
            crate::tui::Language::Zh => "  h 在 History 中打开会话",
            crate::tui::Language::En => "  h open session in History",
        }));
    } else {
        lines.push(Line::from(Span::styled(
            request_page_empty_message(
                lang,
                ui.request_page_scope_session,
                focused_sid.as_deref(),
                focused_sid_observed,
            ),
            Style::default().fg(p.muted),
        )));
    }

    let right_block = Block::default()
        .title(Span::styled(
            l("Details"),
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

fn request_page_empty_message(
    lang: crate::tui::Language,
    scope_session: bool,
    focused_sid: Option<&str>,
    focused_sid_observed: bool,
) -> &'static str {
    if scope_session && focused_sid.is_some() && !focused_sid_observed {
        return match lang {
            crate::tui::Language::Zh => {
                "该会话来自 Codex 历史；当前 proxy runtime 尚未观测到请求。"
            }
            crate::tui::Language::En => {
                "This session came from Codex history; the current proxy runtime has not observed requests for it."
            }
        };
    }
    i18n::label(lang, "No requests match the current filters.")
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

fn request_reasoning_guard_table_label(request: &crate::state::FinishedRequest) -> String {
    let attempts = request_reasoning_guard_attempts(request);
    if attempts.is_empty() {
        return "-".to_string();
    }

    let token = attempts
        .iter()
        .filter_map(|attempt| attempt.reason.as_deref())
        .find_map(reasoning_guard_token_from_reason)
        .map(|token| token.to_string())
        .unwrap_or_else(|| "hit".to_string());
    if attempts
        .iter()
        .any(|attempt| attempt.error_class.as_deref() == Some("reasoning_guard_blocked"))
    {
        format!("{token}!")
    } else if request.attempt_count() > 1 {
        format!("{token}r")
    } else {
        token
    }
}

fn request_reasoning_guard_detail_line(
    request: &crate::state::FinishedRequest,
    lang: crate::tui::Language,
) -> Option<String> {
    let attempts = request_reasoning_guard_attempts(request);
    if attempts.is_empty() {
        return None;
    }

    let token = attempts
        .iter()
        .filter_map(|attempt| attempt.reason.as_deref())
        .find_map(reasoning_guard_token_from_reason)
        .map(|token| format!("reasoning_tokens={token}"))
        .unwrap_or_else(|| "reasoning_tokens matched".to_string());
    let blocked = attempts
        .iter()
        .any(|attempt| attempt.error_class.as_deref() == Some("reasoning_guard_blocked"));
    let retried = attempts
        .iter()
        .any(|attempt| attempt.error_class.as_deref() == Some("reasoning_guard_triggered"))
        && request.attempt_count() > 1;
    let action = match lang {
        crate::tui::Language::Zh => {
            if blocked {
                "已阻断/预算耗尽"
            } else if retried {
                "已重试"
            } else {
                "已命中"
            }
        }
        crate::tui::Language::En => {
            if blocked {
                "blocked/budget exhausted"
            } else if retried {
                "retried"
            } else {
                "matched"
            }
        }
    };

    Some(format!(
        "{token}, {action}, guard_attempts={}",
        attempts.len()
    ))
}

fn request_reasoning_guard_attempts(
    request: &crate::state::FinishedRequest,
) -> Vec<crate::logging::RouteAttemptLog> {
    request
        .retry
        .as_ref()
        .map(|retry| {
            retry
                .route_attempts_or_derived()
                .into_iter()
                .filter(is_reasoning_guard_attempt)
                .collect()
        })
        .unwrap_or_default()
}

fn is_reasoning_guard_attempt(attempt: &crate::logging::RouteAttemptLog) -> bool {
    matches!(
        attempt.error_class.as_deref(),
        Some("reasoning_guard_triggered" | "reasoning_guard_blocked")
    ) || attempt.decision == "failed_reasoning_guard"
}

fn reasoning_guard_token_from_reason(reason: &str) -> Option<i64> {
    reason
        .strip_prefix("reasoning_tokens=")
        .and_then(|value| value.parse::<i64>().ok())
}

fn clean_route_part(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn request_route_table_label(request: &crate::state::FinishedRequest, max_width: usize) -> String {
    let provider = clean_route_part(request.provider_id.as_deref());
    let station = clean_route_part(request.station_name.as_deref());
    let full = match (provider, station) {
        (Some(provider), Some(station)) => format!("{provider} -> {station}"),
        (Some(provider), None) => provider.to_string(),
        (None, Some(station)) => station.to_string(),
        (None, None) => "-".to_string(),
    };
    if UnicodeWidthStr::width(full.as_str()) <= max_width {
        return full;
    }

    let Some(provider) = provider else {
        return shorten_middle(full.as_str(), max_width);
    };
    let Some(station) = station else {
        return shorten_middle(provider, max_width);
    };
    let sep_width = UnicodeWidthStr::width(" -> ");
    if max_width <= sep_width + 3 {
        return shorten_middle(provider, max_width);
    }

    let budget = max_width.saturating_sub(sep_width);
    let station_width = UnicodeWidthStr::width(station);
    if station_width < budget.saturating_sub(3) {
        let provider_width = budget.saturating_sub(station_width);
        return format!("{} -> {station}", shorten_middle(provider, provider_width));
    }

    let provider_width = UnicodeWidthStr::width(provider);
    if provider_width < budget.saturating_sub(3) {
        let station_width = budget.saturating_sub(provider_width);
        return format!("{provider} -> {}", shorten_middle(station, station_width));
    }

    let provider_width = ((budget * 2) / 3).max(3);
    let station_width = budget.saturating_sub(provider_width).max(3);
    format!(
        "{} -> {}",
        shorten_middle(provider, provider_width),
        shorten_middle(station, station_width)
    )
}

fn request_route_attempt_line(attempt: &crate::logging::RouteAttemptLog) -> String {
    let target = attempt
        .provider_endpoint_key
        .as_deref()
        .map(|value| format!("endpoint={}", shorten_middle(value, 36)))
        .unwrap_or_else(|| {
            match (
                attempt.station_name.as_deref(),
                attempt.upstream_base_url.as_deref(),
            ) {
                (Some(station), Some(upstream)) => {
                    format!(
                        "station={station} upstream={}",
                        shorten_middle(upstream, 50)
                    )
                }
                (Some(station), None) => format!("station={station}"),
                (None, Some(upstream)) => format!("upstream={}", shorten_middle(upstream, 58)),
                (None, None) => "target=-".to_string(),
            }
        });
    let mut parts = vec![attempt.decision.clone()];
    if let Some(provider_id) = attempt.provider_id.as_deref() {
        parts.push(format!("prov={}", shorten_middle(provider_id, 18)));
    }
    if let Some(group) = attempt.preference_group {
        parts.push(format!("group={group}"));
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
    if !attempt.avoided_candidate_indices.is_empty() {
        let avoid = attempt
            .avoided_candidate_indices
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("avoid_candidates=[{avoid}]"));
    } else if !attempt.avoid_for_station.is_empty() {
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
    if let Some(provider_control) = request_route_attempt_provider_control_line(attempt, 34) {
        parts.push(provider_control);
    }
    format!("{target}  {}", parts.join(" "))
}

fn request_route_attempt_provider_control_line(
    attempt: &crate::logging::RouteAttemptLog,
    endpoint_width: usize,
) -> Option<String> {
    let signals = attempt
        .provider_signals
        .iter()
        .map(|signal| format!("{:?}", signal.kind).to_ascii_lowercase())
        .collect::<Vec<_>>();
    let actions = attempt
        .policy_actions
        .iter()
        .map(|action| {
            format!(
                "{:?}:{}",
                action.kind,
                shorten_middle(&action.provider_endpoint_key.stable_key(), endpoint_width)
            )
            .to_ascii_lowercase()
        })
        .collect::<Vec<_>>();
    if signals.is_empty() && actions.is_empty() {
        return None;
    }
    Some(format!(
        "control signals={} actions={}",
        dash_join(&signals),
        dash_join(&actions)
    ))
}

fn dash_join(items: &[String]) -> String {
    if items.is_empty() {
        "-".to_string()
    } else {
        items.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::time::Instant;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use crate::dashboard_core::WindowStats;
    use crate::state::UsageRollupView;

    fn empty_snapshot() -> Snapshot {
        Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            model_overrides: HashMap::new(),
            overrides: HashMap::new(),
            station_overrides: HashMap::new(),
            route_target_overrides: HashMap::new(),
            service_tier_overrides: HashMap::new(),
            global_station_override: None,
            global_route_target_override: None,
            station_meta_overrides: HashMap::new(),
            usage_day: crate::state::UsageDayView::default(),
            usage_rollup: UsageRollupView::default(),
            provider_balances: HashMap::new(),
            provider_balance_history: HashMap::new(),
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            provider_endpoint_policy_actions: HashMap::new(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            service_status: None,
            pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
            refreshed_at: Instant::now(),
        }
    }

    fn request_fixture(session_id: &str, status_code: u16) -> crate::state::FinishedRequest {
        crate::state::FinishedRequest {
            id: u64::from(status_code),
            trace_id: None,
            session_id: Some(session_id.to_string()),
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            upstream_base_url: None,
            route_decision: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            observability: crate::state::RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: format!("/v1/responses/{session_id}"),
            status_code,
            duration_ms: 120,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: 1,
            provider_id: None,
            station_name: None,
        }
    }

    fn buffer_text(buffer: &Buffer) -> String {
        let mut out = String::new();
        for y in buffer.area.y..buffer.area.y.saturating_add(buffer.area.height) {
            for x in buffer.area.x..buffer.area.x.saturating_add(buffer.area.width) {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn render_requests_text(
        width: u16,
        height: u16,
        ui: &mut UiState,
        snapshot: &Snapshot,
    ) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let frame = terminal
            .draw(|frame| {
                render_requests_page(frame, Palette::default(), ui, snapshot, frame.area());
            })
            .expect("draw");
        buffer_text(frame.buffer)
    }

    #[test]
    fn requests_empty_state_distinguishes_history_only_session_focus() {
        let snapshot = empty_snapshot();
        let mut ui = UiState {
            page: crate::tui::types::Page::Requests,
            language: crate::tui::Language::En,
            request_page_scope_session: true,
            focused_request_session_id: Some("history-only-session".to_string()),
            ..UiState::default()
        };

        let text = render_requests_text(120, 18, &mut ui, &snapshot);

        assert!(text.contains("came from Codex history"), "{text}");
        assert!(text.contains("current proxy runtime"), "{text}");
    }

    #[test]
    fn requests_page_can_filter_to_provider_control_evidence() {
        let mut snapshot = empty_snapshot();
        let mut ordinary_error = request_fixture("ordinary", 500);
        ordinary_error.provider_id = Some("plain".to_string());
        let mut controlled = request_fixture("controlled", 429);
        controlled.provider_id = Some("limited".to_string());
        controlled.provider_signals.push(
            codex_helper_core::provider_signals::ProviderSignal::high_confidence_route_facing(
                codex_helper_core::provider_signals::ProviderSignalKind::RateLimit,
                codex_helper_core::provider_signals::ProviderSignalSource::UpstreamResponse,
                codex_helper_core::provider_signals::ProviderSignalTarget::ProviderEndpoint {
                    provider_endpoint_key:
                        codex_helper_core::runtime_identity::ProviderEndpointKey::new(
                            "codex", "limited", "default",
                        ),
                },
                1_000,
            ),
        );
        snapshot.recent = vec![ordinary_error, controlled];
        let mut ui = UiState {
            page: crate::tui::types::Page::Requests,
            language: crate::tui::Language::En,
            request_page_control_filter: crate::tui::state::RequestControlFilter::AnyEvidence,
            ..UiState::default()
        };

        let text = render_requests_text(140, 24, &mut ui, &snapshot);

        assert!(text.contains("limited"), "{text}");
        assert!(!text.contains("plain"), "{text}");
        assert!(text.contains("control: evidence"), "{text}");
    }

    #[test]
    fn request_route_attempt_line_prefers_provider_endpoint_identity() {
        let attempt = crate::logging::RouteAttemptLog {
            decision: "failed_transport".to_string(),
            provider_endpoint_key: Some("codex/right/default".to_string()),
            provider_id: Some("right".to_string()),
            preference_group: Some(1),
            provider_attempt: Some(2),
            upstream_attempt: Some(1),
            upstream_base_url: Some("https://right.example/v1".to_string()),
            ..Default::default()
        };

        let line = request_route_attempt_line(&attempt);

        assert!(line.starts_with("endpoint=codex/right/default"));
        assert!(line.contains("group=1"));
        assert!(line.contains("prov=right"));
    }

    #[test]
    fn request_route_table_label_preserves_provider_identity() {
        let request = crate::state::FinishedRequest {
            id: 1,
            trace_id: None,
            session_id: None,
            session_identity_source: None,
            client_name: None,
            client_addr: None,
            cwd: None,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            upstream_base_url: None,
            route_decision: None,
            usage: None,
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            provider_signals: Vec::new(),
            policy_actions: Vec::new(),
            observability: crate::state::RequestObservability::default(),
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: "/v1/responses".to_string(),
            status_code: 200,
            duration_ms: 120,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: 1,
            provider_id: Some("very-long-provider-name".to_string()),
            station_name: Some("input-light".to_string()),
        };

        let label = request_route_table_label(&request, 24);

        assert!(label.contains("input-light"), "{label}");
        assert!(label.contains("->"), "{label}");
        assert!(UnicodeWidthStr::width(label.as_str()) <= 24);
    }
}
