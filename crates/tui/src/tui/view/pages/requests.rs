use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Table, Wrap,
};
#[cfg(test)]
use unicode_width::UnicodeWidthStr;

use crate::dashboard_core::{OperatorRequestSummary, OperatorRouteAttemptSummary};
use crate::tui::i18n;
use crate::tui::model::{
    Palette, RequestAttemptControlEvidence, RequestControlEvidence, Snapshot, duration_short,
    format_age, format_tok_per_second, now_ms, request_attempt_count, request_cache_hit_rate_label,
    request_page_focus_is_runtime_observed, request_page_focus_session_id,
    request_provider_endpoint, sanitize_upstream_origin, shorten, shorten_middle, status_style,
    usage_line_lang,
};
use crate::tui::state::UiState;
use crate::tui::view::widgets::{master_detail_fits, max_wrapped_vertical_scroll};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestTableDensity {
    Compact,
    Regular,
    Wide,
}

impl RequestTableDensity {
    fn for_width(width: u16) -> Self {
        if width >= 116 {
            Self::Wide
        } else if width >= 60 {
            Self::Regular
        } else {
            Self::Compact
        }
    }
}

pub(super) fn render_requests_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let (direction, constraints) = if master_detail_fits(area, 60, 70, 48) {
        (
            Direction::Horizontal,
            [Constraint::Percentage(60), Constraint::Percentage(40)],
        )
    } else {
        (
            Direction::Vertical,
            [Constraint::Percentage(42), Constraint::Percentage(58)],
        )
    };
    let columns = Layout::default()
        .direction(direction)
        .constraints(constraints)
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

    let table_density = RequestTableDensity::for_width(columns[0].width);
    let header = Row::new(match table_density {
        RequestTableDensity::Compact => {
            vec![l("Age"), "St", l("Dur"), "Att", l("Route")]
        }
        RequestTableDensity::Regular => {
            vec![l("Age"), "St", l("Dur"), "Att", l("Hit%"), l("Route")]
        }
        RequestTableDensity::Wide => vec![
            l("Age"),
            "St",
            l("Dur"),
            "Att",
            "RG",
            l("Hit%"),
            l("Route"),
            l("Path"),
        ],
    })
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
            let attempts_n = request_attempt_count(r);
            let attempts = attempts_n.to_string();
            let reasoning_guard = request_reasoning_guard_table_label(r);
            let cache_hit = request_cache_hit_rate_label(r);
            let cache_hit_style =
                Style::default().fg(if cache_hit != "-" { p.accent } else { p.muted });
            let route = request_route_table_label(
                r,
                match table_density {
                    RequestTableDensity::Compact => 34,
                    RequestTableDensity::Regular => 30,
                    RequestTableDensity::Wide => 28,
                },
            );
            let path = shorten_middle(&r.path, 48);

            let mut cells = vec![
                Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                Cell::from(Line::from(vec![status])),
                Cell::from(Span::styled(dur, Style::default().fg(p.muted))),
                Cell::from(Span::styled(
                    attempts,
                    Style::default().fg(if attempts_n > 1 { p.warn } else { p.muted }),
                )),
            ];
            match table_density {
                RequestTableDensity::Compact => {
                    cells.push(Cell::from(route));
                }
                RequestTableDensity::Regular => {
                    cells.push(Cell::from(Span::styled(cache_hit, cache_hit_style)));
                    cells.push(Cell::from(route));
                }
                RequestTableDensity::Wide => {
                    cells.push(Cell::from(Span::styled(
                        reasoning_guard.clone(),
                        Style::default().fg(if reasoning_guard != "-" {
                            p.warn
                        } else {
                            p.muted
                        }),
                    )));
                    cells.push(Cell::from(Span::styled(cache_hit, cache_hit_style)));
                    cells.push(Cell::from(route));
                    cells.push(Cell::from(path));
                }
            }
            Row::new(cells).style(Style::default().bg(p.panel).fg(p.text))
        })
        .collect::<Vec<_>>();

    let widths = match table_density {
        RequestTableDensity::Compact => vec![
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Length(4),
            Constraint::Min(18),
        ],
        RequestTableDensity::Regular => vec![
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Min(18),
        ],
        RequestTableDensity::Wide => vec![
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(28),
            Constraint::Min(14),
        ],
    };
    let table = Table::new(rows, widths)
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
        let observability = &r.observability;
        let control_evidence = snapshot.request_control_evidence.get(&r.id);
        let can_open_local_history = ui.can_bridge_runtime_sessions_to_local_codex()
            && r.session_key.as_deref().is_some_and(|session_key| {
                snapshot.rows.iter().any(|row| {
                    row.session_id.as_deref() == Some(session_key)
                        && row.local_command_session_id().is_some()
                })
            });
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
        let mut request_identity = vec![
            Span::styled(
                format!("{}: ", l("request id")),
                Style::default().fg(p.muted),
            ),
            Span::styled(format!("request-{}", r.id), Style::default().fg(p.text)),
        ];
        if let Some(trace_key) = r.trace_key.as_deref() {
            request_identity.extend([
                Span::raw("  "),
                Span::styled(format!("{}: ", l("trace")), Style::default().fg(p.muted)),
                Span::styled(trace_key.to_string(), Style::default().fg(p.text)),
            ]);
        }
        lines.push(Line::from(request_identity));
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
                Style::default().fg(if r.observability.fast_mode {
                    p.good
                } else {
                    p.text
                }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", l("provider endpoint")),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                r.provider_endpoint_key
                    .as_deref()
                    .map(|value| shorten_middle(value, 80))
                    .unwrap_or_else(|| "-".to_string()),
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
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("endpoint")), Style::default().fg(p.muted)),
            Span::styled(
                r.endpoint_id.as_deref().unwrap_or("-").to_string(),
                Style::default().fg(p.text),
            ),
        ]));
        if !r.route_path.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("route_path: ", Style::default().fg(p.muted)),
                Span::styled(
                    shorten_middle(&r.route_path.join(" / "), 80),
                    Style::default().fg(p.text),
                ),
            ]));
        }
        if let Some(origin) = r
            .upstream_origin
            .as_deref()
            .and_then(sanitize_upstream_origin)
        {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("upstream")), Style::default().fg(p.muted)),
                Span::styled(shorten_middle(&origin, 80), Style::default().fg(p.text)),
            ]));
        }
        if let Some(control) = control_evidence.and_then(request_control_evidence_line) {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("control")), Style::default().fg(p.muted)),
                Span::styled(control, Style::default().fg(p.warn)),
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
            let cache_hit = request_cache_hit_rate_label(r);
            if cache_hit != "-" {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{}: ", l("cache hit rate")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(cache_hit, Style::default().fg(p.text)),
                ]));
            }
            let mut cost_line = vec![
                Span::styled(format!("{}: ", l("cost")), Style::default().fg(p.muted)),
                Span::styled(
                    r.cost.display_total_with_confidence(),
                    Style::default().fg(p.text),
                ),
            ];
            if let Some(source) = r.cost.pricing_source.as_deref() {
                cost_line.extend([
                    Span::raw("  "),
                    Span::styled(
                        format!("{}: ", l("pricing source")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(source.to_string(), Style::default().fg(p.text)),
                ]);
            }
            lines.push(Line::from(cost_line));
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
                Span::styled(
                    request_attempt_count(r).to_string(),
                    Style::default().fg(p.text),
                ),
            ]));
            let max = 12usize;
            let attempts = &retry.route_attempts;
            if !attempts.is_empty() {
                for (idx, attempt) in attempts.iter().take(max).enumerate() {
                    let attempt_control = control_evidence
                        .and_then(|evidence| evidence.route_attempts.get(&attempt.attempt_index));
                    lines.push(Line::from(vec![
                        Span::styled(format!("{:>2}. ", idx + 1), Style::default().fg(p.muted)),
                        Span::styled(
                            request_route_attempt_line(attempt, attempt_control),
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
            } else if retry.attempts > 1 {
                lines.push(Line::from(Span::styled(
                    match ui.language {
                        crate::tui::Language::Zh => {
                            "旧记录未包含可安全展示的结构化路由明细；已保留尝试次数。"
                        }
                        crate::tui::Language::En => {
                            "Legacy record has no safely displayable structured route details; the attempt count is preserved."
                        }
                    },
                    Style::default().fg(p.muted),
                )));
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
        if can_open_local_history {
            lines.push(Line::from(match ui.language {
                crate::tui::Language::Zh => "  h 在 History 中打开会话",
                crate::tui::Language::En => "  h open session in History",
            }));
        }
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
            format!("{}  PgUp/PgDn", l("Details")),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));
    let inner = right_block.inner(columns[1]);
    let max_scroll = max_wrapped_vertical_scroll(&lines, inner.width, inner.height);
    ui.requests_details_scroll = ui.requests_details_scroll.min(max_scroll);
    let content = Paragraph::new(Text::from(lines))
        .block(right_block)
        .style(Style::default().fg(p.text))
        .scroll((ui.requests_details_scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(content, columns[1]);
    if max_scroll > 0 {
        let mut scrollbar = ScrollbarState::new(usize::from(max_scroll) + 1)
            .position(usize::from(ui.requests_details_scroll));
        let widget = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(p.border));
        f.render_stateful_widget(widget, columns[1], &mut scrollbar);
    }
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

fn request_cost_parts_line(request: &OperatorRequestSummary) -> Option<String> {
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

fn request_reasoning_guard_table_label(request: &OperatorRequestSummary) -> String {
    let attempts = request_reasoning_guard_attempts(request);
    if attempts.is_empty() {
        return "-".to_string();
    }

    if attempts
        .iter()
        .any(|attempt| attempt.code.contains("reasoning_guard_blocked"))
    {
        "blk!".to_string()
    } else if request_attempt_count(request) > 1 {
        "hitR".to_string()
    } else {
        "hit".to_string()
    }
}

fn request_reasoning_guard_detail_line(
    request: &OperatorRequestSummary,
    lang: crate::tui::Language,
) -> Option<String> {
    let attempts = request_reasoning_guard_attempts(request);
    if attempts.is_empty() {
        return None;
    }

    let code = attempts
        .iter()
        .map(|attempt| attempt.code.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let blocked = attempts
        .iter()
        .any(|attempt| attempt.code.contains("reasoning_guard_blocked"));
    let retried = attempts
        .iter()
        .any(|attempt| attempt.code.contains("reasoning_guard_triggered"))
        && request_attempt_count(request) > 1;
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
        "{code}, {action}, guard_attempts={}",
        attempts.len()
    ))
}

fn request_reasoning_guard_attempts(
    request: &OperatorRequestSummary,
) -> Vec<&OperatorRouteAttemptSummary> {
    request
        .retry
        .as_ref()
        .map(|retry| {
            retry
                .route_attempts
                .iter()
                .filter(|attempt| is_reasoning_guard_attempt(attempt))
                .collect()
        })
        .unwrap_or_default()
}

fn is_reasoning_guard_attempt(attempt: &OperatorRouteAttemptSummary) -> bool {
    attempt.code.contains("reasoning_guard")
}

fn clean_route_part(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn request_route_table_label(request: &OperatorRequestSummary, max_width: usize) -> String {
    let provider = request.provider_id.as_deref().and_then(clean_route_part);
    let endpoint = request.endpoint_id.as_deref().and_then(clean_route_part);
    let label = match (provider, endpoint) {
        (Some(provider), Some(endpoint)) => Some(format!("{provider}/{endpoint}")),
        (Some(provider), None) => Some(provider.to_string()),
        (None, Some(endpoint)) => Some(endpoint.to_string()),
        (None, None) => None,
    }
    .or_else(|| {
        request
            .provider_endpoint_key
            .as_deref()
            .and_then(clean_route_part)
            .map(ToOwned::to_owned)
    })
    .or_else(|| request_provider_endpoint(request).map(|endpoint| endpoint.stable_key()))
    .or_else(|| {
        request
            .provider_id
            .as_deref()
            .and_then(clean_route_part)
            .map(ToOwned::to_owned)
    })
    .unwrap_or_else(|| "-".to_string());
    shorten_middle(&label, max_width)
}

fn request_route_attempt_line(
    attempt: &OperatorRouteAttemptSummary,
    control_evidence: Option<&RequestAttemptControlEvidence>,
) -> String {
    let target = attempt
        .provider_endpoint_key
        .as_deref()
        .map(|value| format!("endpoint={}", shorten_middle(value, 36)))
        .unwrap_or_else(|| {
            match (
                attempt.provider_id.as_deref(),
                attempt.endpoint_id.as_deref(),
            ) {
                (Some(provider), Some(endpoint)) => format!(
                    "provider={} endpoint={}",
                    shorten_middle(provider, 24),
                    shorten_middle(endpoint, 24)
                ),
                (Some(provider), None) => {
                    format!("provider={}", shorten_middle(provider, 50))
                }
                (None, Some(endpoint)) => {
                    format!("endpoint={}", shorten_middle(endpoint, 50))
                }
                (None, None) => "target=-".to_string(),
            }
        });
    let mut parts = vec![attempt.code.clone()];
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
    if let Some(avoided_total) = attempt.avoided_total.filter(|value| *value > 0) {
        if let Some(total) = attempt.total_upstreams {
            parts.push(format!("avoided={avoided_total}/{total}"));
        } else {
            parts.push(format!("avoided={avoided_total}"));
        }
    }
    if let Some(provider_control) =
        request_route_attempt_provider_control_line(attempt, control_evidence, 34)
    {
        parts.push(provider_control);
    }
    format!("{target}  {}", parts.join(" "))
}

fn request_control_evidence_line(evidence: &RequestControlEvidence) -> Option<String> {
    if evidence.provider_signal_codes.is_empty() && evidence.policy_action_codes.is_empty() {
        return None;
    }
    Some(format!(
        "signals={} actions={}",
        dash_join(&evidence.provider_signal_codes),
        dash_join(&evidence.policy_action_codes)
    ))
}

fn request_route_attempt_provider_control_line(
    attempt: &OperatorRouteAttemptSummary,
    control_evidence: Option<&RequestAttemptControlEvidence>,
    _endpoint_width: usize,
) -> Option<String> {
    let signals = control_evidence
        .map(|evidence| evidence.provider_signal_codes.clone())
        .unwrap_or_else(|| attempt.provider_signal_codes.clone());
    let actions = control_evidence
        .map(|evidence| evidence.policy_action_codes.clone())
        .unwrap_or_else(|| attempt.policy_action_codes.clone());
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

    use crate::dashboard_core::{
        OperatorRequestObservability, OperatorRetrySummaryView, OperatorRouteAttemptSummary,
        WindowStats,
    };
    use crate::state::UsageRollupView;

    fn empty_snapshot() -> Snapshot {
        Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            request_control_evidence: HashMap::new(),
            usage_day: crate::state::UsageDayView::default(),
            quota_analytics: crate::quota_analytics::QuotaAnalyticsView::default(),
            usage_rollup: UsageRollupView::default(),
            provider_balances: HashMap::new(),
            routing: None,
            pricing_catalog: Default::default(),
            stats_5m: WindowStats::default(),
            stats_1h: WindowStats::default(),
            service_status: None,
            refreshed_at: Instant::now(),
        }
    }

    fn request_fixture(session_id: &str, status_code: u16) -> OperatorRequestSummary {
        OperatorRequestSummary {
            id: u64::from(status_code),
            trace_key: None,
            session_key: Some(session_id.to_string()),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            route_path: Vec::new(),
            upstream_origin: None,
            usage: None,
            cache_accounting_convention: Default::default(),
            cost: crate::pricing::CostBreakdown::default(),
            retry: None,
            provider_signal_codes: Vec::new(),
            policy_action_codes: Vec::new(),
            observability: OperatorRequestObservability {
                duration_ms: Some(120),
                ttfb_ms: None,
                generation_ms: None,
                output_tokens_per_second: None,
                attempt_count: 1,
                route_attempt_count: 0,
                retried: false,
                cross_provider_failover: false,
                same_provider_retry: false,
                fast_mode: false,
                streaming: false,
            },
            service: "codex".to_string(),
            method: "POST".to_string(),
            path: format!("/v1/responses/{session_id}"),
            status_code,
            duration_ms: 120,
            ttfb_ms: None,
            streaming: false,
            ended_at_ms: 1,
        }
    }

    fn route_attempt_fixture(attempt_index: u32) -> OperatorRouteAttemptSummary {
        OperatorRouteAttemptSummary {
            attempt_index,
            code: "failed_status".to_string(),
            provider_endpoint_key: Some(format!("codex/provider-{attempt_index}/default")),
            provider_id: Some(format!("provider-{attempt_index}")),
            endpoint_id: Some("default".to_string()),
            preference_group: Some(0),
            provider_attempt: Some(attempt_index),
            upstream_attempt: Some(attempt_index),
            provider_max_attempts: Some(12),
            upstream_max_attempts: Some(12),
            avoided_total: None,
            total_upstreams: None,
            status_code: Some(429),
            model: Some("gpt-5.6".to_string()),
            upstream_headers_ms: Some(120),
            duration_ms: Some(240),
            cooldown_secs: Some(30),
            skipped: false,
            provider_signal_codes: Vec::new(),
            policy_action_codes: Vec::new(),
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
        snapshot.request_control_evidence.insert(
            controlled.id,
            crate::tui::model::RequestControlEvidence {
                provider_signal_codes: vec!["rate_limit".to_string()],
                ..Default::default()
            },
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
    fn requests_page_shows_redacted_pricing_source() {
        let mut snapshot = empty_snapshot();
        let mut request = request_fixture("priced", 200);
        request.usage = Some(crate::usage::UsageMetrics {
            total_tokens: 10,
            ..Default::default()
        });
        request.cost.pricing_source = Some("remote".to_string());
        snapshot.recent = vec![request];
        let mut ui = UiState {
            page: crate::tui::types::Page::Requests,
            language: crate::tui::Language::En,
            ..UiState::default()
        };

        let text = render_requests_text(140, 28, &mut ui, &snapshot);

        assert!(text.contains("pricing source"), "{text}");
        assert!(text.contains("remote"), "{text}");
    }

    #[test]
    fn requests_page_shows_cache_tokens_and_hit_rate() {
        let mut snapshot = empty_snapshot();
        let mut request = request_fixture("cached", 200);
        request.usage = Some(crate::usage::UsageMetrics {
            input_tokens: 1_000,
            output_tokens: 200,
            total_tokens: 1_200,
            cache_read_input_tokens: 250,
            cache_creation_input_tokens: 50,
            ..Default::default()
        });
        request.cache_accounting_convention =
            crate::usage::CacheAccountingConvention::INCLUDED_IN_INPUT;
        snapshot.recent = vec![request];
        let mut ui = UiState {
            page: crate::tui::types::Page::Requests,
            language: crate::tui::Language::En,
            ..UiState::default()
        };

        let text = render_requests_text(140, 28, &mut ui, &snapshot);

        assert!(text.contains("25.0%"), "{text}");
        assert!(text.contains("cache"), "{text}");
        assert!(text.contains("read/create: 250/50"), "{text}");
    }

    #[test]
    fn requests_page_marks_cache_rate_inferred_from_service_protocol() {
        let mut snapshot = empty_snapshot();
        let mut request = request_fixture("cached-unpriced", 200);
        request.usage = Some(crate::usage::UsageMetrics {
            input_tokens: 1_000,
            output_tokens: 200,
            total_tokens: 1_200,
            cache_read_input_tokens: 250,
            cache_creation_input_tokens: 50,
            ..Default::default()
        });
        request.cache_accounting_convention = crate::usage::CacheAccountingConvention::UNKNOWN;
        request.service = "codex".to_string();
        snapshot.recent = vec![request];
        let mut ui = UiState {
            page: crate::tui::types::Page::Requests,
            language: crate::tui::Language::En,
            ..UiState::default()
        };

        let text = render_requests_text(140, 28, &mut ui, &snapshot);

        assert!(text.contains("~23.8%"), "{text}");
        assert!(text.contains("cache"), "{text}");
        assert!(text.contains("read/create: 250/50"), "{text}");
    }

    #[test]
    fn requests_page_keeps_primary_route_visible_at_supported_terminal_sizes() {
        let mut snapshot = empty_snapshot();
        let mut request = request_fixture("sized", 200);
        request.provider_id = Some("ciii".to_string());
        request.endpoint_id = Some("default".to_string());
        snapshot.recent = vec![request];

        for (width, height) in [(80, 24), (120, 30), (160, 45)] {
            let mut ui = UiState {
                page: crate::tui::types::Page::Requests,
                language: crate::tui::Language::En,
                ..UiState::default()
            };
            let text = render_requests_text(width, height, &mut ui, &snapshot);

            assert!(text.contains("ciii/default"), "{width}x{height}:\n{text}");
            assert!(text.contains("Details"), "{width}x{height}:\n{text}");
            assert!(text.contains("Route"), "{width}x{height}:\n{text}");
        }
    }

    #[test]
    fn requests_page_keeps_cache_hit_visible_at_common_terminal_widths() {
        let mut snapshot = empty_snapshot();
        let mut request = request_fixture("cache-visible", 200);
        request.provider_id = Some("ciii".to_string());
        request.endpoint_id = Some("default".to_string());
        request.usage = Some(crate::usage::UsageMetrics {
            input_tokens: 1_000,
            output_tokens: 200,
            total_tokens: 1_200,
            cache_read_input_tokens: 700,
            ..Default::default()
        });
        snapshot.recent = vec![request];

        for width in [120, 132, 140] {
            let mut ui = UiState {
                page: crate::tui::types::Page::Requests,
                language: crate::tui::Language::En,
                ..UiState::default()
            };
            let text = render_requests_text(width, 40, &mut ui, &snapshot);

            assert!(
                text.lines()
                    .any(|line| line.contains("Hit%") && line.contains("Route")),
                "Requests cache-hit and route columns must remain visible at {width} columns:\n{text}"
            );
        }
    }

    #[test]
    fn requests_details_scroll_makes_bottom_keys_reachable() {
        let mut snapshot = empty_snapshot();
        let mut request = request_fixture("scroll", 429);
        request.provider_id = Some("limited".to_string());
        request.endpoint_id = Some("default".to_string());
        request.usage = Some(crate::usage::UsageMetrics {
            input_tokens: 1_000,
            output_tokens: 200,
            total_tokens: 1_200,
            cache_read_input_tokens: 250,
            ..Default::default()
        });
        request.retry = Some(OperatorRetrySummaryView {
            attempts: 12,
            route_attempts: (1..=12).map(route_attempt_fixture).collect(),
        });
        snapshot.recent = vec![request];
        let mut ui = UiState {
            page: crate::tui::types::Page::Requests,
            language: crate::tui::Language::En,
            requests_details_scroll: u16::MAX,
            ..UiState::default()
        };

        let text = render_requests_text(80, 24, &mut ui, &snapshot);

        assert!(ui.requests_details_scroll < u16::MAX);
        assert!(ui.requests_details_scroll > 0);
        assert!(text.contains("Keys"), "{text}");
        assert!(text.contains("toggle errors-only"), "{text}");
    }

    #[test]
    fn requests_details_explain_legacy_chain_without_structured_attempts() {
        let mut snapshot = empty_snapshot();
        let mut request = request_fixture("legacy", 500);
        request.retry = Some(OperatorRetrySummaryView {
            attempts: 3,
            route_attempts: Vec::new(),
        });
        snapshot.recent = vec![request];
        let mut ui = UiState {
            page: crate::tui::types::Page::Requests,
            language: crate::tui::Language::En,
            ..UiState::default()
        };

        let text = render_requests_text(160, 45, &mut ui, &snapshot);

        assert!(text.contains("Legacy record"), "{text}");
        assert!(text.contains("attempt count is preserved"), "{text}");
    }

    #[test]
    fn request_table_density_preserves_a_route_column_at_each_width() {
        assert_eq!(
            RequestTableDensity::for_width(59),
            RequestTableDensity::Compact
        );
        assert_eq!(
            RequestTableDensity::for_width(60),
            RequestTableDensity::Regular
        );
        assert_eq!(
            RequestTableDensity::for_width(120),
            RequestTableDensity::Wide
        );
    }

    #[test]
    fn request_route_attempt_line_prefers_provider_endpoint_identity() {
        let attempt = OperatorRouteAttemptSummary {
            attempt_index: 1,
            code: "failed_transport".to_string(),
            provider_endpoint_key: Some("codex/right/default".to_string()),
            provider_id: Some("right".to_string()),
            endpoint_id: Some("default".to_string()),
            preference_group: Some(1),
            provider_attempt: Some(2),
            upstream_attempt: Some(1),
            provider_max_attempts: None,
            upstream_max_attempts: None,
            avoided_total: None,
            total_upstreams: None,
            status_code: None,
            model: None,
            upstream_headers_ms: None,
            duration_ms: None,
            cooldown_secs: None,
            skipped: false,
            provider_signal_codes: Vec::new(),
            policy_action_codes: Vec::new(),
        };

        let line = request_route_attempt_line(&attempt, None);

        assert!(line.starts_with("endpoint=codex/right/default"));
        assert!(line.contains("group=1"));
        assert!(line.contains("prov=right"));
    }

    #[test]
    fn request_route_attempt_line_uses_code_only_control_evidence() {
        let attempt = OperatorRouteAttemptSummary {
            attempt_index: 2,
            provider_id: None,
            endpoint_id: None,
            provider_endpoint_key: None,
            preference_group: None,
            provider_attempt: None,
            upstream_attempt: None,
            provider_max_attempts: None,
            upstream_max_attempts: None,
            avoided_total: None,
            total_upstreams: None,
            code: "failed_status".to_string(),
            status_code: Some(429),
            model: None,
            upstream_headers_ms: None,
            duration_ms: None,
            cooldown_secs: None,
            skipped: false,
            provider_signal_codes: Vec::new(),
            policy_action_codes: Vec::new(),
        };
        let evidence = RequestAttemptControlEvidence {
            provider_signal_codes: vec!["rate_limit".to_string()],
            policy_action_codes: vec!["cooldown".to_string()],
        };

        let line = request_route_attempt_line(&attempt, Some(&evidence));

        assert!(line.contains("signals=rate_limit"), "{line}");
        assert!(line.contains("actions=cooldown"), "{line}");
    }

    #[test]
    fn request_route_table_label_prefers_readable_provider_endpoint() {
        let mut request = request_fixture("sid", 200);
        request.provider_id = Some("ciii".to_string());
        request.endpoint_id = Some("input-light".to_string());
        request.provider_endpoint_key = Some("endpoint:sha256:opaque".to_string());
        request.retry = Some(OperatorRetrySummaryView {
            attempts: 1,
            route_attempts: Vec::new(),
        });

        let label = request_route_table_label(&request, 24);

        assert_eq!(label, "ciii/input-light");
        assert!(UnicodeWidthStr::width(label.as_str()) <= 24);
    }

    #[test]
    fn request_details_do_not_present_internal_id_as_trace_id() {
        let mut snapshot = empty_snapshot();
        snapshot.recent = vec![request_fixture("sid", 200)];
        let mut ui = UiState {
            page: crate::tui::types::Page::Requests,
            language: crate::tui::Language::En,
            ..UiState::default()
        };

        let text = render_requests_text(140, 28, &mut ui, &snapshot);

        assert!(text.contains("request id: request-200"), "{text}");
        assert!(!text.contains("trace: request-200"), "{text}");
    }

    #[test]
    fn request_details_present_safe_helper_trace_key() {
        let mut snapshot = empty_snapshot();
        let mut request = request_fixture("sid", 200);
        request.trace_key =
            Some("ch-trace:v1:01234567-89ab-cdef-0123-456789abcdef:codex:200".to_string());
        snapshot.recent = vec![request];
        let mut ui = UiState {
            page: crate::tui::types::Page::Requests,
            language: crate::tui::Language::En,
            ..UiState::default()
        };

        let text = render_requests_text(140, 28, &mut ui, &snapshot);

        assert!(text.contains("trace:"), "{text}");
        assert!(
            text.contains("ch-trace:v1:01234567-89ab-cdef-0123-456789abcdef:codex"),
            "{text}"
        );
        assert!(text.contains(":200"), "{text}");
    }
}
