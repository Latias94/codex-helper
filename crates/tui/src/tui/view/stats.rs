use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Sparkline, Table, Wrap,
};

use crate::state::{UsageBucket, UsageDayDimensionRow, UsageDayView};
use crate::tui::Language;
use crate::tui::ProviderOption;
use crate::tui::model::{
    Palette, Snapshot, duration_short, format_age, now_ms, shorten, tokens_short,
};
use crate::tui::state::UiState;
use crate::tui::types::StatsFocus;

pub(super) fn render_stats_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    _providers: &[ProviderOption],
    area: Rect,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(area);

    render_kpi_row(f, p, ui.language, &snapshot.usage_day, rows[0]);
    render_activity_row(f, p, ui.language, &snapshot.usage_day, rows[1]);
    render_dimension_area(f, p, ui, &snapshot.usage_day, rows[2]);
}

fn render_kpi_row(f: &mut Frame<'_>, p: Palette, lang: Language, usage: &UsageDayView, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(28),
            Constraint::Percentage(24),
            Constraint::Percentage(24),
            Constraint::Percentage(24),
        ])
        .split(area);

    let summary = &usage.summary;
    let ok = summary
        .requests_total
        .saturating_sub(summary.requests_error);
    render_info_block(
        f,
        p,
        match lang {
            Language::Zh => format!("今日用量 {}", usage.label),
            Language::En => format!("Today {}", usage.label),
        },
        vec![
            kv_line(
                p,
                "tokens",
                &tokens_short(summary.usage.total_tokens),
                p.accent,
            ),
            kv_line(p, "cost", &summary.cost.display_total(), p.text),
            kv_line(p, "coverage", &cost_coverage_label(summary, lang), p.muted),
        ],
        cols[0],
    );

    render_info_block(
        f,
        p,
        match lang {
            Language::Zh => "请求",
            Language::En => "Requests",
        },
        vec![
            Line::from(vec![
                muted(p, "total "),
                Span::styled(
                    summary.requests_total.to_string(),
                    Style::default().fg(p.text),
                ),
                Span::raw("  "),
                muted(p, "ok "),
                Span::styled(ok.to_string(), Style::default().fg(p.good)),
                Span::raw("  "),
                muted(p, "err "),
                Span::styled(
                    summary.requests_error.to_string(),
                    Style::default().fg(p.warn),
                ),
            ]),
            kv_line(p, "success", &success_pct(summary), p.good),
            kv_line(p, "avg latency", &avg_duration(summary), p.text),
        ],
        cols[1],
    );

    render_info_block(
        f,
        p,
        match lang {
            Language::Zh => "Token 结构",
            Language::En => "Token Mix",
        },
        vec![
            Line::from(vec![
                muted(p, "in "),
                Span::styled(
                    tokens_short(summary.usage.input_tokens),
                    Style::default().fg(p.text),
                ),
                Span::raw("  "),
                muted(p, "out "),
                Span::styled(
                    tokens_short(summary.usage.output_tokens),
                    Style::default().fg(p.text),
                ),
            ]),
            kv_line(p, "cache read", "-", p.muted),
            kv_line(
                p,
                "reasoning",
                &tokens_short(summary.usage.reasoning_output_tokens_total()),
                p.muted,
            ),
        ],
        cols[2],
    );

    let gate = &usage.retry_gate;
    let reasons = if gate.reasons.is_empty() {
        "-".to_string()
    } else {
        gate.reasons
            .iter()
            .map(|row| format!("{}:{}", shorten(&row.reason, 18), row.active))
            .collect::<Vec<_>>()
            .join("  ")
    };
    render_info_block(
        f,
        p,
        match lang {
            Language::Zh => "Retry Gate",
            Language::En => "Retry Gate",
        },
        vec![
            kv_line(
                p,
                "active",
                &gate.active.to_string(),
                gate_style(p, gate.active),
            ),
            kv_line(
                p,
                "cooldown",
                &gate.active_cooldowns.to_string(),
                gate_style(p, gate.active_cooldowns),
            ),
            kv_line(
                p,
                "max left",
                &gate
                    .max_remaining_secs
                    .map(|secs| duration_short(secs.saturating_mul(1000)))
                    .unwrap_or_else(|| "-".to_string()),
                p.muted,
            ),
            kv_line(p, "reason", &shorten(&reasons, 56), p.muted),
        ],
        cols[3],
    );
}

fn render_activity_row(
    f: &mut Frame<'_>,
    p: Palette,
    lang: Language,
    usage: &UsageDayView,
    area: Rect,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    let data = usage
        .hourly
        .iter()
        .map(|row| row.bucket.requests_total)
        .collect::<Vec<_>>();
    let title = match lang {
        Language::Zh => "24 小时请求量",
        Language::En => "24h Requests",
    };
    f.render_widget(
        Sparkline::default()
            .block(panel_block(p, title))
            .data(&data)
            .style(Style::default().fg(p.accent)),
        cols[0],
    );

    let coverage = &usage.coverage;
    let coverage_style = if coverage.day_may_be_partial {
        Style::default().fg(p.warn)
    } else {
        Style::default().fg(p.good)
    };
    let status = if coverage.day_may_be_partial {
        match lang {
            Language::Zh => "可能不完整",
            Language::En => "partial",
        }
    } else {
        match lang {
            Language::Zh => "已加载窗口",
            Language::En => "loaded window",
        }
    };
    let first = format_age(now_ms(), coverage.loaded_first_ms);
    let last = format_age(now_ms(), coverage.loaded_last_ms);
    let mut lines = vec![
        Line::from(vec![
            muted(p, "status "),
            Span::styled(status, coverage_style),
        ]),
        Line::from(vec![
            muted(p, "source "),
            Span::styled(shorten(&coverage.source, 20), Style::default().fg(p.text)),
            Span::raw("  "),
            muted(p, "loaded "),
            Span::styled(
                coverage.loaded_requests.to_string(),
                Style::default().fg(p.text),
            ),
        ]),
        Line::from(vec![
            muted(p, "first "),
            Span::styled(first, Style::default().fg(p.muted)),
            Span::raw("  "),
            muted(p, "last "),
            Span::styled(last, Style::default().fg(p.muted)),
        ]),
    ];
    if let Some(reason) = coverage.partial_reason.as_deref() {
        lines.push(Line::from(vec![
            muted(p, "note "),
            Span::styled(shorten(reason, 72), Style::default().fg(p.warn)),
        ]));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(panel_block(
                p,
                match lang {
                    Language::Zh => "覆盖范围",
                    Language::En => "Coverage",
                },
            ))
            .wrap(Wrap { trim: true }),
        cols[1],
    );
}

fn render_dimension_area(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    usage: &UsageDayView,
    area: Rect,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    render_focus_table(f, p, ui, usage, cols[0]);
    render_side_lists(f, p, ui.language, usage, cols[1]);
}

fn render_focus_table(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    usage: &UsageDayView,
    area: Rect,
) {
    let (title, rows, table_state) = match ui.stats_focus {
        StatsFocus::Providers => (
            match ui.language {
                Language::Zh => "提供商排行",
                Language::En => "Providers",
            },
            &usage.provider_rows,
            &mut ui.stats_providers_table,
        ),
        StatsFocus::ProviderEndpoints => (
            match ui.language {
                Language::Zh => "提供商端点排行",
                Language::En => "Provider endpoints",
            },
            &usage.provider_endpoint_rows,
            &mut ui.stats_provider_endpoints_table,
        ),
    };

    let table_rows = rows
        .iter()
        .map(|row| dimension_table_row(p, row))
        .collect::<Vec<_>>();
    let table = Table::new(
        table_rows,
        [
            Constraint::Min(18),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
    )
    .header(Row::new(vec![
        Cell::from(muted(p, "name")),
        Cell::from(muted(p, "req")),
        Cell::from(muted(p, "tokens")),
        Cell::from(muted(p, "cost")),
        Cell::from(muted(p, "err")),
        Cell::from(muted(p, "avg")),
    ]))
    .block(panel_block(p, title))
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, area, table_state);
}

fn render_side_lists(
    f: &mut Frame<'_>,
    p: Palette,
    lang: Language,
    usage: &UsageDayView,
    area: Rect,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(area);
    render_compact_rows(
        f,
        p,
        match lang {
            Language::Zh => "模型",
            Language::En => "Models",
        },
        &usage.model_rows,
        rows[0],
    );
    render_compact_rows(
        f,
        p,
        match lang {
            Language::Zh => "会话",
            Language::En => "Sessions",
        },
        &usage.session_rows,
        rows[1],
    );
    render_compact_rows(
        f,
        p,
        match lang {
            Language::Zh => "项目",
            Language::En => "Projects",
        },
        &usage.project_rows,
        rows[2],
    );
}

fn render_compact_rows(
    f: &mut Frame<'_>,
    p: Palette,
    title: &'static str,
    rows: &[UsageDayDimensionRow],
    area: Rect,
) {
    let lines = if rows.is_empty() {
        vec![Line::from(Span::styled("-", Style::default().fg(p.muted)))]
    } else {
        rows.iter()
            .take(6)
            .map(|row| {
                Line::from(vec![
                    Span::styled(shorten(&row.name, 28), Style::default().fg(p.text)),
                    Span::raw("  "),
                    muted(p, &tokens_short(row.bucket.usage.total_tokens)),
                    Span::raw("  "),
                    muted(p, &format!("n={}", row.bucket.requests_total)),
                ])
            })
            .collect::<Vec<_>>()
    };
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(panel_block(p, title))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn dimension_table_row(p: Palette, row: &UsageDayDimensionRow) -> Row<'static> {
    Row::new(vec![
        Cell::from(Span::styled(
            shorten(&row.name, 48),
            Style::default().fg(p.text),
        )),
        Cell::from(Span::styled(
            row.bucket.requests_total.to_string(),
            Style::default().fg(p.text),
        )),
        Cell::from(Span::styled(
            tokens_short(row.bucket.usage.total_tokens),
            Style::default().fg(p.accent),
        )),
        Cell::from(Span::styled(
            row.bucket.cost.display_total(),
            Style::default().fg(p.text),
        )),
        Cell::from(Span::styled(
            row.bucket.requests_error.to_string(),
            Style::default().fg(if row.bucket.requests_error > 0 {
                p.warn
            } else {
                p.muted
            }),
        )),
        Cell::from(Span::styled(
            avg_duration(&row.bucket),
            Style::default().fg(p.muted),
        )),
    ])
    .style(Style::default().bg(p.panel).fg(p.text))
}

fn render_info_block(
    f: &mut Frame<'_>,
    p: Palette,
    title: impl Into<String>,
    lines: Vec<Line<'static>>,
    area: Rect,
) {
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(panel_block(p, title))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn panel_block(p: Palette, title: impl Into<String>) -> Block<'static> {
    Block::default()
        .title(Span::styled(
            title.into(),
            Style::default().fg(p.muted).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel))
}

fn kv_line(p: Palette, key: &'static str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        muted(p, key),
        Span::raw(" "),
        Span::styled(value.to_string(), Style::default().fg(color)),
    ])
}

fn muted(p: Palette, value: &str) -> Span<'static> {
    Span::styled(value.to_string(), Style::default().fg(p.muted))
}

fn gate_style(p: Palette, value: u64) -> Color {
    if value > 0 { p.warn } else { p.good }
}

fn success_pct(bucket: &UsageBucket) -> String {
    if bucket.requests_total == 0 {
        return "-".to_string();
    }
    let ok = bucket.requests_total.saturating_sub(bucket.requests_error);
    format!("{:.0}%", (ok as f64 / bucket.requests_total as f64) * 100.0)
}

fn avg_duration(bucket: &UsageBucket) -> String {
    bucket
        .duration_ms_total
        .checked_div(bucket.requests_total)
        .map(duration_short)
        .unwrap_or_else(|| "-".to_string())
}

fn cost_coverage_label(bucket: &UsageBucket, lang: Language) -> String {
    match (
        bucket.cost.priced_requests,
        bucket.cost.unpriced_requests,
        lang,
    ) {
        (0, 0, _) => "-".to_string(),
        (priced, 0, Language::Zh) => format!("估算 {priced}"),
        (priced, 0, Language::En) => format!("estimated {priced}"),
        (priced, unpriced, Language::Zh) => format!("估算 {priced} / 未知 {unpriced}"),
        (priced, unpriced, Language::En) => format!("estimated {priced} / unknown {unpriced}"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Instant;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::state::{
        UsageDayCoverage, UsageDayDimensionRow, UsageDayHourRow, UsageDayView,
        UsageRetryGateReasonRow, UsageRetryGateSummary,
    };
    use crate::tui::model::Snapshot;
    use crate::tui::state::UiState;

    fn bucket(requests: u64, tokens: i64) -> UsageBucket {
        let mut bucket = UsageBucket {
            requests_total: requests,
            duration_ms_total: requests.saturating_mul(100),
            ..UsageBucket::default()
        };
        bucket.usage.total_tokens = tokens;
        bucket.usage.input_tokens = tokens / 2;
        bucket.usage.output_tokens = tokens / 2;
        bucket
    }

    fn row(name: &str, requests: u64, tokens: i64) -> UsageDayDimensionRow {
        UsageDayDimensionRow {
            name: name.to_string(),
            bucket: bucket(requests, tokens),
        }
    }

    fn sample_snapshot() -> Snapshot {
        let hourly = (0..24)
            .map(|hour| UsageDayHourRow {
                hour,
                bucket: bucket(u64::from(hour % 4), i64::from(hour) * 10),
            })
            .collect::<Vec<_>>();
        Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            request_control_evidence: HashMap::new(),
            usage_day: UsageDayView {
                day: crate::usage_day::current_local_day(),
                label: "2026-07-07".to_string(),
                start_ms: 1,
                end_ms: 2,
                generated_at_ms: 2,
                summary: bucket(12, 4096),
                hourly,
                provider_rows: vec![row("input", 7, 3000), row("input1", 5, 1096)],
                provider_endpoint_rows: vec![row("routing", 12, 4096)],
                model_rows: vec![row("gpt-5", 12, 4096)],
                session_rows: vec![row("sid-main", 8, 3000)],
                project_rows: vec![row("F:/SourceCodes/Rust/codex-helper", 12, 4096)],
                retry_gate: UsageRetryGateSummary {
                    active: 2,
                    active_cooldowns: 2,
                    max_remaining_secs: Some(60),
                    reasons: vec![UsageRetryGateReasonRow {
                        reason: "reasoning_tokens=516".to_string(),
                        active: 2,
                    }],
                },
                coverage: UsageDayCoverage {
                    source: "runtime_store".to_string(),
                    loaded_first_ms: Some(1),
                    loaded_last_ms: Some(2),
                    loaded_requests: 12,
                    day_may_be_partial: true,
                    partial_reason: Some("loaded data starts after local day start".to_string()),
                },
            },
            usage_rollup: crate::state::UsageRollupView::default(),
            provider_balances: HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            service_status: None,
            refreshed_at: Instant::now(),
        }
    }

    fn render_text(width: u16, height: u16, ui: &mut UiState, snapshot: &Snapshot) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let frame = terminal
            .draw(|frame| {
                render_stats_page(frame, Palette::default(), ui, snapshot, &[], frame.area());
            })
            .expect("draw");
        buffer_text(frame.buffer)
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

    #[test]
    fn stats_render_uses_usage_day_and_global_retry_gate() {
        let snapshot = sample_snapshot();
        let mut ui = UiState {
            page: crate::tui::types::Page::Stats,
            stats_focus: StatsFocus::Providers,
            ..UiState::default()
        };

        let text = render_text(128, 26, &mut ui, &snapshot);

        assert!(text.contains("Today"));
        assert!(text.contains("Retry Gate"));
        assert!(text.contains("active"));
        assert!(text.contains("reasoning_tokens"));
        assert!(text.contains("input"));
        assert!(text.contains("Coverage"));
    }

    #[test]
    fn stats_render_keeps_narrow_layout_bounded() {
        let snapshot = sample_snapshot();
        let mut ui = UiState {
            page: crate::tui::types::Page::Stats,
            stats_focus: StatsFocus::ProviderEndpoints,
            ..UiState::default()
        };

        let text = render_text(76, 22, &mut ui, &snapshot);

        assert!(text.contains("Provider endpoints") || text.contains("提供商端点"));
        assert!(!text.contains("15d"));
    }
}
