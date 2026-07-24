use chrono::{DateTime, Local};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Sparkline, Table, Wrap,
};
use unicode_width::UnicodeWidthStr;

use crate::quota_analytics::{
    PoolQuotaAnalytics, QuotaAnalyticsSupport, QuotaFreshnessStatus, QuotaPaceStatus,
    QuotaRateStatus, QuotaReconciliationStatus,
};
use crate::quota_pool::{IdentityConfidence, QuotaQuantity, QuotaUnit, QuotaWindowKind};
use crate::state::{ProviderBalanceSnapshot, UsageBucket, UsageDayDimensionRow, UsageDayView};
use crate::tui::Language;
use crate::tui::ProviderOption;
use crate::tui::model::{
    Palette, Snapshot, duration_short, format_age, now_ms, provider_usage_rate_summary_lang,
    provider_usage_report_is_current, provider_usage_source_label_lang,
    provider_usage_window_brief_lang, provider_usage_window_summary_lang, shorten, tokens_short,
};
use crate::tui::state::UiState;
use crate::tui::types::StatsFocus;

#[derive(Default)]
struct SelectedQuotaUsage<'a> {
    pool: Option<&'a PoolQuotaAnalytics>,
    provider_usage: Option<&'a ProviderBalanceSnapshot>,
    provider_usage_rate: Option<String>,
}

pub(super) fn render_stats_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    _providers: &[ProviderOption],
    area: Rect,
) {
    let compact = area.width < 100 || area.height < 19;
    let selected_pool = ui.selected_quota_pool(snapshot);
    let provider_usage =
        selected_pool.and_then(|pool| provider_usage_snapshot_for_pool(snapshot, pool));
    let provider_usage_rate =
        provider_usage.and_then(|usage| provider_usage_rate_summary_lang(usage, ui.language));
    let selected_usage = SelectedQuotaUsage {
        pool: selected_pool,
        provider_usage,
        provider_usage_rate,
    };
    let desired_quota_row_height = if compact {
        compact_quota_kpi_height(&selected_usage)
    } else if area.height >= 30 {
        8
    } else {
        7
    };
    let quota_row_height = desired_quota_row_height.min(area.height);
    let compact_local_usage_height = area.height.saturating_sub(quota_row_height).min(6);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if compact {
            vec![
                Constraint::Length(quota_row_height),
                Constraint::Length(compact_local_usage_height),
                Constraint::Min(0),
            ]
        } else {
            vec![
                Constraint::Length(quota_row_height),
                Constraint::Length(6),
                Constraint::Length(5),
                Constraint::Min(0),
            ]
        })
        .split(area);

    render_quota_kpi_row(f, p, ui, snapshot, &selected_usage, rows[0], compact);
    render_local_usage_row(f, p, ui.language, &snapshot.usage_day, rows[1]);
    if compact {
        render_dimension_area(f, p, ui, snapshot, rows[2], true);
    } else {
        render_activity_row(f, p, ui.language, &snapshot.usage_day, rows[2]);
        render_dimension_area(f, p, ui, snapshot, rows[3], false);
    }
}

fn render_quota_kpi_row(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    selected_usage: &SelectedQuotaUsage<'_>,
    area: Rect,
    compact: bool,
) {
    let lang = ui.language;
    let usage = &snapshot.usage_day;
    let quota = &snapshot.quota_analytics;

    if compact {
        let (title, lines) = quota_summary_lines(
            p,
            lang,
            ui,
            quota.support,
            usage,
            selected_usage,
            usize::from(area.width.saturating_sub(16).max(1)),
        );
        render_info_block(f, p, title, lines, area);
        return;
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(26),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(24),
        ])
        .split(area);

    let Some(pool) = selected_usage.pool else {
        let (title, lines) =
            quota_summary_lines(p, lang, ui, quota.support, usage, selected_usage, 80);
        render_info_block(f, p, title, lines, area);
        return;
    };

    render_info_block(
        f,
        p,
        match lang {
            Language::Zh => format!("额度池 {}", ui.selected_stats_pool_idx + 1),
            Language::En => format!("Quota Pool {}", ui.selected_stats_pool_idx + 1),
        },
        vec![
            panel_kv_line(
                p,
                "source",
                &shorten(provider_usage_source_label_lang(&pool.source, lang), 24),
                p.text,
                cols[0],
            ),
            panel_kv_line(
                p,
                "scope",
                &format!(
                    "{} / {}",
                    pool.identity.scope.as_key(),
                    identity_confidence(pool)
                ),
                identity_color(p, pool),
                cols[0],
            ),
            panel_kv_line(
                p,
                "state",
                &pool_state_label(pool, ui, lang),
                pool_state_color(p, pool, ui),
                cols[0],
            ),
            panel_kv_line(p, "age", &pool_age(pool), p.muted, cols[0]),
        ],
        cols[0],
    );

    let remote_usage = remote_usage_display(pool, lang);
    let provider_usage_window = selected_usage
        .provider_usage
        .and_then(|snapshot| snapshot.usage_windows.first());
    render_info_block(
        f,
        p,
        match lang {
            Language::Zh => "上游额度池",
            Language::En => "Upstream Quota Pool",
        },
        vec![
            panel_kv_line(
                p,
                &remote_usage.label,
                &quantity_text(remote_usage.quantity),
                p.accent,
                cols[1],
            ),
            panel_kv_line(
                p,
                "remaining",
                &quantity_text(pool.remote_remaining.as_ref()),
                p.text,
                cols[1],
            ),
            panel_kv_line(
                p,
                "limit",
                &quantity_text(pool.remote_limit.as_ref()),
                p.muted,
                cols[1],
            ),
            panel_kv_line(
                p,
                "unit",
                pool.unit.as_str(),
                unit_color(p, pool.unit),
                cols[1],
            ),
            panel_kv_line(
                p,
                "win",
                &provider_usage_window
                    .map(|window| provider_usage_window_brief_lang(window, lang))
                    .unwrap_or_else(|| "-".to_string()),
                provider_usage_window.map_or(p.muted, |_| p.text),
                cols[1],
            ),
        ],
        cols[1],
    );

    let mut observed_rate_lines = vec![
        panel_kv_line(
            p,
            "15m",
            &rate_text(&pool.rate_15m, lang),
            rate_color(p, pool.rate_15m.status),
            cols[2],
        ),
        panel_kv_line(
            p,
            "60m",
            &rate_text(&pool.rate_60m, lang),
            rate_color(p, pool.rate_60m.status),
            cols[2],
        ),
        panel_kv_line(
            p,
            "API rate",
            &provider_usage_rpm_tpm_text(selected_usage.provider_usage),
            selected_usage.provider_usage.map_or(p.muted, |_| p.accent),
            cols[2],
        ),
    ];
    if area.height >= 8 {
        observed_rate_lines.push(panel_kv_line(
            p,
            "API avg",
            &provider_usage_average_duration_text(selected_usage.provider_usage),
            selected_usage.provider_usage.map_or(p.muted, |_| p.text),
            cols[2],
        ));
    }
    observed_rate_lines.extend([
        panel_kv_line(
            p,
            "required",
            &hourly_quantity_text(pool.pacing.required_rate_per_hour.as_ref()),
            p.muted,
            cols[2],
        ),
        panel_kv_line(
            p,
            "ETA",
            &pool
                .pacing
                .exhaustion_eta_ms
                .map(duration_short)
                .unwrap_or_else(|| "-".to_string()),
            p.text,
            cols[2],
        ),
    ]);
    render_info_block(
        f,
        p,
        match lang {
            Language::Zh => "上游观测速率",
            Language::En => "Upstream Observed Rate",
        },
        observed_rate_lines,
        cols[2],
    );

    let summary = &usage.summary;
    render_info_block(
        f,
        p,
        match lang {
            Language::Zh => "上游节奏 / 本机今日",
            Language::En => "Upstream Pace / Local Today",
        },
        vec![
            panel_kv_line(
                p,
                "pace",
                &pace_label(pool.pacing.status, lang),
                pace_color(p, pool.pacing.status),
                cols[3],
            ),
            panel_kv_line(
                p,
                "reset",
                &reset_text(pool, now_ms(), lang),
                p.muted,
                cols[3],
            ),
            panel_kv_line(
                p,
                "local cost",
                &summary.cost.display_total(),
                p.text,
                cols[3],
            ),
            panel_kv_line(
                p,
                "local tokens",
                &tokens_short(summary.usage.total_tokens),
                p.accent,
                cols[3],
            ),
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

fn render_local_usage_row(
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
    let summary = &usage.summary;
    let metrics = &summary.usage;
    let ok = summary
        .requests_total
        .saturating_sub(summary.requests_error);

    render_info_block(
        f,
        p,
        match lang {
            Language::Zh => "本机用量",
            Language::En => "Local Usage",
        },
        vec![
            Line::from(vec![
                muted(p, "tokens "),
                Span::styled(
                    tokens_short(metrics.total_tokens),
                    Style::default().fg(p.accent),
                ),
                Span::raw("  "),
                muted(p, "cost "),
                Span::styled(summary.cost.display_total(), Style::default().fg(p.text)),
            ]),
            Line::from(vec![
                muted(p, "in "),
                Span::styled(
                    tokens_short(metrics.input_tokens),
                    Style::default().fg(p.text),
                ),
                Span::raw("  "),
                muted(p, "out "),
                Span::styled(
                    tokens_short(metrics.output_tokens),
                    Style::default().fg(p.text),
                ),
                Span::raw("  "),
                muted(p, "reasoning "),
                Span::styled(
                    tokens_short(metrics.reasoning_output_tokens_total()),
                    Style::default().fg(p.text),
                ),
            ]),
            Line::from(vec![
                muted(p, "cache read "),
                Span::styled(
                    tokens_short(metrics.cache_read_tokens_total()),
                    Style::default().fg(p.text),
                ),
                Span::raw("  "),
                muted(p, "cache write "),
                Span::styled(
                    tokens_short(metrics.cache_creation_tokens_total()),
                    Style::default().fg(p.text),
                ),
            ]),
            Line::from(vec![
                muted(p, "requests "),
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
                    Style::default().fg(if summary.requests_error > 0 {
                        p.warn
                    } else {
                        p.muted
                    }),
                ),
            ]),
        ],
        cols[0],
    );

    let gate = &usage.retry_gate;
    let reasons = if gate.reasons.is_empty() {
        "-".to_string()
    } else {
        gate.reasons
            .iter()
            .map(|row| format!("{}:{}", shorten(&row.reason, 20), row.active))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let coverage = &usage.coverage;
    let coverage_status = if coverage.day_may_be_partial {
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
    render_info_block(
        f,
        p,
        match lang {
            Language::Zh => "Retry Gate / 覆盖范围",
            Language::En => "Retry Gate / Coverage",
        },
        vec![
            Line::from(vec![
                muted(p, "active "),
                Span::styled(
                    gate.active.to_string(),
                    Style::default().fg(if gate.active > 0 { p.warn } else { p.good }),
                ),
                Span::raw("  "),
                muted(p, "cooldown "),
                Span::styled(
                    gate.active_cooldowns.to_string(),
                    Style::default().fg(if gate.active_cooldowns > 0 {
                        p.warn
                    } else {
                        p.good
                    }),
                ),
                Span::raw("  "),
                muted(p, "max "),
                Span::styled(
                    gate.max_remaining_secs
                        .map(|secs| duration_short(secs.saturating_mul(1000)))
                        .unwrap_or_else(|| "-".to_string()),
                    Style::default().fg(p.muted),
                ),
            ]),
            kv_line(p, "reason", &shorten(&reasons, 32), p.muted),
            Line::from(vec![
                muted(p, "coverage "),
                Span::styled(
                    coverage_status,
                    Style::default().fg(if coverage.day_may_be_partial {
                        p.warn
                    } else {
                        p.good
                    }),
                ),
                Span::raw("  "),
                muted(p, "loaded "),
                Span::styled(
                    coverage.loaded_requests.to_string(),
                    Style::default().fg(p.text),
                ),
            ]),
            kv_line(p, "source", &shorten(&coverage.source, 24), p.text),
        ],
        cols[1],
    );
}

fn render_dimension_area(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
    compact: bool,
) {
    if compact {
        render_focus_table(f, p, ui, snapshot, area);
        return;
    }
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
        .split(area);
    render_focus_table(f, p, ui, snapshot, cols[0]);
    render_side_lists(f, p, ui.language, &snapshot.usage_day, cols[1]);
}

fn render_focus_table(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    match ui.stats_focus {
        StatsFocus::Pools => {
            render_pool_table(f, p, ui, snapshot, area);
            return;
        }
        StatsFocus::Projects => {
            render_project_table(f, p, ui, snapshot, area);
            return;
        }
        StatsFocus::Providers | StatsFocus::ProviderEndpoints => {}
    }
    let usage = &snapshot.usage_day;
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
        StatsFocus::Pools | StatsFocus::Projects => unreachable!("handled above"),
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

fn render_pool_table(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let lang = ui.language;
    let rows = snapshot
        .quota_analytics
        .pools
        .iter()
        .map(|pool| {
            let displayed_used = remote_usage_display(pool, lang).quantity;
            Row::new(vec![
                Cell::from(Span::styled(pool_name(pool), Style::default().fg(p.text))),
                Cell::from(Span::styled(
                    quantity_text(displayed_used),
                    Style::default().fg(p.accent),
                )),
                Cell::from(Span::styled(
                    quantity_text(pool.remote_remaining.as_ref()),
                    Style::default().fg(p.text),
                )),
                Cell::from(Span::styled(
                    rate_text(&pool.rate_60m, lang),
                    Style::default().fg(rate_color(p, pool.rate_60m.status)),
                )),
                Cell::from(Span::styled(
                    pace_label(pool.pacing.status, lang),
                    Style::default().fg(pace_color(p, pool.pacing.status)),
                )),
                Cell::from(Span::styled(pool_age(pool), Style::default().fg(p.muted))),
                Cell::from(Span::styled(
                    format!(
                        "{} / {}",
                        pool.identity.scope.as_key(),
                        identity_confidence(pool)
                    ),
                    Style::default().fg(identity_color(p, pool)),
                )),
            ])
            .style(Style::default().bg(p.panel).fg(p.text))
        })
        .collect::<Vec<_>>();
    let title = match snapshot.quota_analytics.support {
        QuotaAnalyticsSupport::Unsupported => match lang {
            Language::Zh => "额度池（当前 read model 不支持）".to_string(),
            Language::En => "Quota Pools (unsupported read model)".to_string(),
        },
        QuotaAnalyticsSupport::Supported => match lang {
            Language::Zh => format!(
                "额度池 {}（省略 {}）",
                snapshot.quota_analytics.pools.len(),
                snapshot.quota_analytics.omitted_pools
            ),
            Language::En => format!(
                "Quota Pools {} ({} omitted)",
                snapshot.quota_analytics.pools.len(),
                snapshot.quota_analytics.omitted_pools
            ),
        },
    };
    let table = Table::new(
        rows,
        [
            Constraint::Min(18),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(11),
            Constraint::Length(8),
            Constraint::Length(18),
        ],
    )
    .header(Row::new(vec![
        Cell::from(muted(p, "pool")),
        Cell::from(muted(p, "remote total")),
        Cell::from(muted(p, "remaining")),
        Cell::from(muted(p, "60m")),
        Cell::from(muted(p, "pace")),
        Cell::from(muted(p, "age")),
        Cell::from(muted(p, "scope / confidence")),
    ]))
    .block(panel_block(p, title))
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, area, &mut ui.stats_pools_table);
}

fn render_project_table(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let lang = ui.language;
    let selected_pool = ui.selected_quota_pool(snapshot).cloned();
    let summary_height = if area.height >= 7 { 3 } else { 0 };
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(summary_height)])
        .split(area);
    let mut rows = selected_pool
        .as_ref()
        .map(|pool| {
            pool.reconciliation
                .projects
                .iter()
                .map(|row| {
                    Row::new(vec![
                        Cell::from(Span::styled("project", Style::default().fg(p.muted))),
                        Cell::from(Span::styled(
                            shorten(row.project.display_key(), 64),
                            Style::default().fg(p.text),
                        )),
                        Cell::from(Span::styled(
                            quantity_text(Some(&row.local_cost)),
                            Style::default().fg(p.accent),
                        )),
                        Cell::from(Span::styled(
                            row.requests.to_string(),
                            Style::default().fg(p.text),
                        )),
                    ])
                    .style(Style::default().bg(p.panel).fg(p.text))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(pool) = selected_pool.as_ref() {
        let reconciliation = &pool.reconciliation;
        if reconciliation.omitted_projects > 0 {
            let label = match lang {
                Language::Zh => format!("其余 {} 个项目", reconciliation.omitted_projects),
                Language::En => format!("{} more projects", reconciliation.omitted_projects),
            };
            rows.push(
                Row::new(vec![
                    Cell::from(Span::styled("omitted", Style::default().fg(p.muted))),
                    Cell::from(Span::styled(label, Style::default().fg(p.muted))),
                    Cell::from(Span::styled(
                        quantity_text(reconciliation.omitted_local_known.as_ref()),
                        Style::default().fg(p.muted),
                    )),
                    Cell::from(Span::styled("-", Style::default().fg(p.muted))),
                ])
                .style(Style::default().bg(p.panel)),
            );
        }
        for (kind, label, value, color) in [
            (
                "unknown",
                match lang {
                    Language::Zh => "本机未知项目",
                    Language::En => "local unknown project",
                },
                reconciliation.local_unknown.as_ref(),
                p.warn,
            ),
            (
                "external",
                match lang {
                    Language::Zh => "外部 / 未归因",
                    Language::En => "external / unattributed",
                },
                reconciliation.external_unattributed.as_ref(),
                p.muted,
            ),
        ] {
            if value.is_some() {
                rows.push(
                    Row::new(vec![
                        Cell::from(Span::styled(kind, Style::default().fg(color))),
                        Cell::from(Span::styled(label, Style::default().fg(color))),
                        Cell::from(Span::styled(
                            quantity_text(value),
                            Style::default().fg(color),
                        )),
                        Cell::from(Span::styled("-", Style::default().fg(p.muted))),
                    ])
                    .style(Style::default().bg(p.panel)),
                );
            }
        }
        if let Some(gap) = reconciliation.signed_delta {
            rows.push(
                Row::new(vec![
                    Cell::from(Span::styled("gap", Style::default().fg(gap_color(p, gap)))),
                    Cell::from(Span::styled(
                        match lang {
                            Language::Zh => "远端 - 本机",
                            Language::En => "remote - local",
                        },
                        Style::default().fg(gap_color(p, gap)),
                    )),
                    Cell::from(Span::styled(
                        signed_usd_text(gap),
                        Style::default().fg(gap_color(p, gap)),
                    )),
                    Cell::from(Span::styled("-", Style::default().fg(p.muted))),
                ])
                .style(Style::default().bg(p.panel)),
            );
        }
    }
    let title = match lang {
        Language::Zh => "项目归因",
        Language::En => "Project Attribution",
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Min(22),
            Constraint::Length(16),
            Constraint::Length(8),
        ],
    )
    .header(Row::new(vec![
        Cell::from(muted(p, "kind")),
        Cell::from(muted(p, "project")),
        Cell::from(muted(p, "local cost")),
        Cell::from(muted(p, "requests")),
    ]))
    .block(panel_block(p, title))
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, areas[0], &mut ui.stats_projects_table);

    if summary_height > 0 {
        let line = selected_pool
            .as_ref()
            .map(|pool| reconciliation_summary(pool, lang))
            .unwrap_or_else(|| match lang {
                Language::Zh => "未选择额度池".to_string(),
                Language::En => "no quota pool selected".to_string(),
            });
        f.render_widget(
            Paragraph::new(line)
                .style(Style::default().fg(p.muted).bg(p.panel))
                .wrap(Wrap { trim: true }),
            areas[1],
        );
    }
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

fn kv_line(p: Palette, key: &str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        muted(p, key),
        Span::raw(" "),
        Span::styled(value.to_string(), Style::default().fg(color)),
    ])
}

fn panel_kv_line(p: Palette, key: &str, value: &str, color: Color, area: Rect) -> Line<'static> {
    let inner_width = usize::from(area.width.saturating_sub(2));
    let value_width = inner_width
        .saturating_sub(UnicodeWidthStr::width(key))
        .saturating_sub(1);
    compact_kv_line(p, key, value, color, value_width)
}

fn muted(p: Palette, value: &str) -> Span<'static> {
    Span::styled(value.to_string(), Style::default().fg(p.muted))
}

fn quota_summary_lines(
    p: Palette,
    lang: Language,
    ui: &UiState,
    support: QuotaAnalyticsSupport,
    usage: &UsageDayView,
    selected_usage: &SelectedQuotaUsage<'_>,
    max_value_width: usize,
) -> (String, Vec<Line<'static>>) {
    if support == QuotaAnalyticsSupport::Unsupported {
        return (
            match lang {
                Language::Zh => "上游额度池（不支持）".to_string(),
                Language::En => "Upstream Quota Pool (unsupported)".to_string(),
            },
            vec![
                kv_line(
                    p,
                    "status",
                    match lang {
                        Language::Zh => "当前 operator read model 未提供额度分析",
                        Language::En => "current operator read model has no quota analytics",
                    },
                    p.warn,
                ),
                kv_line(p, "local cost", &usage.summary.cost.display_total(), p.text),
                kv_line(
                    p,
                    "local tokens",
                    &tokens_short(usage.summary.usage.total_tokens),
                    p.accent,
                ),
            ],
        );
    }
    let Some(pool) = selected_usage.pool else {
        return (
            match lang {
                Language::Zh => "上游额度池".to_string(),
                Language::En => "Upstream Quota Pool".to_string(),
            },
            vec![
                kv_line(
                    p,
                    "status",
                    match lang {
                        Language::Zh => "没有支持的额度适配器或样本",
                        Language::En => "no supported quota adapter or sample",
                    },
                    p.muted,
                ),
                kv_line(
                    p,
                    "refresh",
                    if ui.needs_snapshot_refresh {
                        match lang {
                            Language::Zh => "同步中",
                            Language::En => "syncing",
                        }
                    } else {
                        "-"
                    },
                    if ui.needs_snapshot_refresh {
                        p.accent
                    } else {
                        p.muted
                    },
                ),
                kv_line(p, "local cost", &usage.summary.cost.display_total(), p.text),
            ],
        );
    };

    let remote_usage = remote_usage_display(pool, lang);
    let remaining_label = match (lang, max_value_width < 42) {
        (Language::Zh, _) => "剩余",
        (Language::En, true) => "left",
        (Language::En, false) => "remaining",
    };
    let quota_text = format!(
        "{} {}  {remaining_label} {}",
        remote_usage.label,
        quantity_text(remote_usage.quantity),
        quantity_text(pool.remote_remaining.as_ref())
    );
    let mut lines = vec![compact_kv_line(
        p,
        "quota",
        &quota_text,
        p.accent,
        max_value_width,
    )];
    lines.push(compact_kv_line(
        p,
        "state",
        &pool_state_label(pool, ui, lang),
        pool_state_color(p, pool, ui),
        max_value_width,
    ));
    if let Some(summary) = selected_usage.provider_usage_rate.as_deref() {
        lines.push(compact_kv_line(
            p,
            "upstream API",
            summary,
            p.accent,
            max_value_width,
        ));
    }
    if let Some(window) = selected_usage
        .provider_usage
        .and_then(|snapshot| snapshot.usage_windows.first())
    {
        let window_text = if max_value_width < 42 {
            provider_usage_window_brief_lang(window, lang)
        } else {
            provider_usage_window_summary_lang(window, lang)
        };
        lines.push(compact_kv_line(
            p,
            "API window",
            &window_text,
            p.text,
            max_value_width,
        ));
    }
    lines.extend([
        compact_kv_line(
            p,
            "rates",
            &format!(
                "15m {}  60m {}  required {}",
                rate_text(&pool.rate_15m, lang),
                rate_text(&pool.rate_60m, lang),
                hourly_quantity_text(pool.pacing.required_rate_per_hour.as_ref())
            ),
            rate_color(p, pool.rate_15m.status),
            max_value_width,
        ),
        compact_kv_line(
            p,
            "pace",
            &format!(
                "{}  ETA {}  reset {}",
                pace_label(pool.pacing.status, lang),
                pool.pacing
                    .exhaustion_eta_ms
                    .map(duration_short)
                    .unwrap_or_else(|| "-".to_string()),
                reset_text(pool, now_ms(), lang)
            ),
            pace_color(p, pool.pacing.status),
            max_value_width,
        ),
        compact_kv_line(
            p,
            "source",
            &format!(
                "{}  scope {}  confidence {}",
                provider_usage_source_label_lang(&pool.source, lang),
                pool.identity.scope.as_key(),
                identity_confidence(pool)
            ),
            identity_color(p, pool),
            max_value_width,
        ),
    ]);

    (
        match lang {
            Language::Zh => format!("上游额度池 · {}", pool_name(pool)),
            Language::En => format!("Upstream Quota Pool · {}", pool_name(pool)),
        },
        lines,
    )
}

fn compact_kv_line(
    p: Palette,
    key: &str,
    value: &str,
    color: Color,
    max_value_width: usize,
) -> Line<'static> {
    kv_line(p, key, &shorten(value, max_value_width), color)
}

fn compact_quota_kpi_height(selected_usage: &SelectedQuotaUsage<'_>) -> u16 {
    let extra_lines = usize::from(selected_usage.provider_usage_rate.is_some())
        + usize::from(
            selected_usage
                .provider_usage
                .is_some_and(|usage| !usage.usage_windows.is_empty()),
        );
    7_u16.saturating_add(extra_lines as u16)
}

fn provider_usage_snapshot_for_pool<'a>(
    snapshot: &'a Snapshot,
    pool: &PoolQuotaAnalytics,
) -> Option<&'a ProviderBalanceSnapshot> {
    let endpoint = pool.endpoint.as_ref()?;
    let observation_provider_id = pool.observation_provider_id.trim();
    let source = pool.source.trim();
    if observation_provider_id.is_empty() || source.is_empty() {
        return None;
    }
    snapshot
        .provider_balances
        .get(&endpoint.provider_id)?
        .iter()
        .find(|balance| {
            balance.provider_endpoint == *endpoint
                && balance.observation_provider_id == observation_provider_id
                && balance.source == source
                && balance.quota_pool_key.as_deref() == Some(pool.identity.key.as_str())
                && balance.quota_pool_revision == Some(pool.identity.revision)
                && provider_usage_report_is_current(balance)
        })
}

fn provider_usage_rpm_tpm_text(provider_usage: Option<&ProviderBalanceSnapshot>) -> String {
    let Some(rate) = provider_usage.and_then(|snapshot| snapshot.usage_rate.as_ref()) else {
        return "-".to_string();
    };
    let rpm = rate
        .rpm
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("-");
    let tpm = rate
        .tpm
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("-");
    format!("RPM {rpm} / TPM {tpm}")
}

fn provider_usage_average_duration_text(
    provider_usage: Option<&ProviderBalanceSnapshot>,
) -> String {
    provider_usage
        .and_then(|snapshot| snapshot.usage_rate.as_ref())
        .and_then(|rate| rate.average_duration_ms.as_deref())
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "-".to_string())
}

struct RemoteUsageDisplay<'a> {
    label: String,
    quantity: Option<&'a QuotaQuantity>,
}

fn remote_usage_display<'a>(
    pool: &'a PoolQuotaAnalytics,
    lang: Language,
) -> RemoteUsageDisplay<'a> {
    if let Some(quantity) = pool.remote_direct_total.as_ref() {
        let label = if pool.window.allows_today_label() {
            match lang {
                Language::Zh => "今日已用",
                Language::En => "today used",
            }
        } else {
            match lang {
                Language::Zh => "远端窗口",
                Language::En => "remote window",
            }
        };
        return RemoteUsageDisplay {
            label: label.to_string(),
            quantity: Some(quantity),
        };
    }

    if let Some(quantity) = pool.observed_burn.as_ref() {
        let since = local_clock_text(pool.epoch_start_ms);
        let label = match lang {
            Language::Zh => format!("自 {since} 观测"),
            Language::En => format!("observed since {since}"),
        };
        return RemoteUsageDisplay {
            label,
            quantity: Some(quantity),
        };
    }

    let label = if pool.capabilities.cumulative {
        match lang {
            Language::Zh => "累计已用",
            Language::En => "cumulative used",
        }
    } else {
        match lang {
            Language::Zh => "已用",
            Language::En => "used",
        }
    };
    RemoteUsageDisplay {
        label: label.to_string(),
        quantity: pool.remote_used.as_ref(),
    }
}

fn local_clock_text(timestamp_ms: u64) -> String {
    i64::try_from(timestamp_ms)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
        .map(|timestamp| timestamp.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "?".to_string())
}

fn hourly_quantity_text(quantity: Option<&QuotaQuantity>) -> String {
    let value = quantity_text(quantity);
    if value == "-" {
        value
    } else {
        format!("{value}/h")
    }
}

fn pool_name(pool: &PoolQuotaAnalytics) -> String {
    let origin = pool
        .identity
        .origin
        .strip_prefix("https://")
        .or_else(|| pool.identity.origin.strip_prefix("http://"))
        .unwrap_or(pool.identity.origin.as_str());
    if origin.is_empty() {
        shorten(&pool.source, 30)
    } else {
        shorten(origin, 30)
    }
}

fn identity_confidence(pool: &PoolQuotaAnalytics) -> &'static str {
    match pool.identity.confidence {
        IdentityConfidence::High => "high",
        IdentityConfidence::Medium => "medium",
        IdentityConfidence::Low => "low",
        IdentityConfidence::Unknown => "unknown",
    }
}

fn identity_color(p: Palette, pool: &PoolQuotaAnalytics) -> Color {
    if pool.identity.aggregation_eligible {
        match pool.identity.confidence {
            IdentityConfidence::High => p.good,
            IdentityConfidence::Medium => p.accent,
            IdentityConfidence::Low | IdentityConfidence::Unknown => p.warn,
        }
    } else {
        p.warn
    }
}

fn unit_color(p: Palette, unit: QuotaUnit) -> Color {
    match unit {
        QuotaUnit::Usd | QuotaUnit::Tokens => p.text,
        QuotaUnit::Raw | QuotaUnit::Unknown => p.warn,
    }
}

fn quantity_text(quantity: Option<&QuotaQuantity>) -> String {
    let Some(quantity) = quantity else {
        return "-".to_string();
    };
    let Some(decimal) = decimal_text(quantity.value, quantity.scale) else {
        return "-".to_string();
    };
    match quantity.unit {
        QuotaUnit::Usd => format!("${decimal}"),
        QuotaUnit::Tokens => format!("{decimal} tok"),
        QuotaUnit::Raw => format!("{decimal} raw"),
        QuotaUnit::Unknown => format!("{decimal} ?"),
    }
}

fn decimal_text(value: i128, scale: u32) -> Option<String> {
    let scale = usize::try_from(scale).ok()?;
    if scale > 38 {
        return None;
    }
    let negative = value < 0;
    let mut digits = value.unsigned_abs().to_string();
    if scale > 0 {
        if digits.len() <= scale {
            digits.insert_str(0, &"0".repeat(scale + 1 - digits.len()));
        }
        digits.insert(digits.len() - scale, '.');
    }
    Some(if negative {
        format!("-{digits}")
    } else {
        digits
    })
}

fn signed_usd_text(value: crate::quota_analytics::SignedUsdDelta) -> String {
    let decimal = value.format_usd();
    decimal.strip_prefix('-').map_or_else(
        || format!("${decimal}"),
        |magnitude| format!("-${magnitude}"),
    )
}

fn rate_text(rate: &crate::quota_analytics::QuotaRateWindow, lang: Language) -> String {
    if rate.status == QuotaRateStatus::Available {
        let value = quantity_text(rate.rate_per_hour.as_ref());
        return if rate.lower_bound {
            format!(">={value}/h")
        } else {
            format!("{value}/h")
        };
    }
    match (rate.status, lang) {
        (QuotaRateStatus::InsufficientSamples | QuotaRateStatus::ShortSpan, Language::Zh) => {
            "样本不足".to_string()
        }
        (QuotaRateStatus::InsufficientSamples | QuotaRateStatus::ShortSpan, Language::En) => {
            "low sample".to_string()
        }
        (QuotaRateStatus::Stale, Language::Zh) => "已过期".to_string(),
        (QuotaRateStatus::Stale, Language::En) => "stale".to_string(),
        (QuotaRateStatus::Gap, Language::Zh) => "采样中断".to_string(),
        (QuotaRateStatus::Gap, Language::En) => "sample gap".to_string(),
        (QuotaRateStatus::Adjustment | QuotaRateStatus::NegativeDelta, Language::Zh) => {
            "额度调整".to_string()
        }
        (QuotaRateStatus::Adjustment | QuotaRateStatus::NegativeDelta, Language::En) => {
            "adjustment".to_string()
        }
        _ => "-".to_string(),
    }
}

fn rate_color(p: Palette, status: QuotaRateStatus) -> Color {
    match status {
        QuotaRateStatus::Available => p.accent,
        QuotaRateStatus::Stale
        | QuotaRateStatus::Gap
        | QuotaRateStatus::Adjustment
        | QuotaRateStatus::NegativeDelta => p.warn,
        _ => p.muted,
    }
}

fn pace_label(status: QuotaPaceStatus, lang: Language) -> String {
    match (status, lang) {
        (QuotaPaceStatus::Unlimited, Language::Zh) => "无限额度",
        (QuotaPaceStatus::Unlimited, Language::En) => "unlimited",
        (QuotaPaceStatus::Faster, Language::Zh) => "偏快",
        (QuotaPaceStatus::Faster, Language::En) => "faster",
        (QuotaPaceStatus::OnPace, Language::Zh) => "正常",
        (QuotaPaceStatus::OnPace, Language::En) => "on pace",
        (QuotaPaceStatus::Slower, Language::Zh) => "偏慢",
        (QuotaPaceStatus::Slower, Language::En) => "slower",
        (QuotaPaceStatus::NoReset, Language::Zh) => "不重置",
        (QuotaPaceStatus::NoReset, Language::En) => "no reset",
        (QuotaPaceStatus::ResetUnknown, Language::Zh) => "重置未知",
        (QuotaPaceStatus::ResetUnknown, Language::En) => "reset unknown",
        (QuotaPaceStatus::LowSample, Language::Zh) => "样本不足",
        (QuotaPaceStatus::LowSample, Language::En) => "low sample",
        (QuotaPaceStatus::Stale, Language::Zh) => "已过期",
        (QuotaPaceStatus::Stale, Language::En) => "stale",
        (QuotaPaceStatus::Unavailable, _) => "-",
    }
    .to_string()
}

fn pace_color(p: Palette, status: QuotaPaceStatus) -> Color {
    match status {
        QuotaPaceStatus::Unlimited | QuotaPaceStatus::OnPace | QuotaPaceStatus::Slower => p.good,
        QuotaPaceStatus::Faster | QuotaPaceStatus::Stale => p.warn,
        _ => p.muted,
    }
}

fn pool_state_label(pool: &PoolQuotaAnalytics, ui: &UiState, lang: Language) -> String {
    if ui.needs_snapshot_refresh {
        return match lang {
            Language::Zh => "同步中",
            Language::En => "syncing",
        }
        .to_string();
    }
    if !pool.identity.aggregation_eligible {
        return match lang {
            Language::Zh => "身份模糊",
            Language::En => "ambiguous identity",
        }
        .to_string();
    }
    if pool.capabilities.unlimited {
        return pace_label(QuotaPaceStatus::Unlimited, lang);
    }
    if pool
        .remote_remaining
        .as_ref()
        .is_some_and(QuotaQuantity::is_zero)
    {
        return match lang {
            Language::Zh => "已用尽",
            Language::En => "exhausted",
        }
        .to_string();
    }
    if pool.latest_adjustment
        == Some(crate::quota_pool::QuotaAdjustmentKind::CounterResetOrRollback)
    {
        return match lang {
            Language::Zh => "刚重置 / 回滚",
            Language::En => "just reset / rollback",
        }
        .to_string();
    }
    match (pool.freshness, lang) {
        (QuotaFreshnessStatus::Fresh, Language::Zh) => "新鲜",
        (QuotaFreshnessStatus::Fresh, Language::En) => "fresh",
        (QuotaFreshnessStatus::Stale, Language::Zh) => "已过期（缓存）",
        (QuotaFreshnessStatus::Stale, Language::En) => "stale (cached)",
        (QuotaFreshnessStatus::Offline, Language::Zh) => "离线（缓存）",
        (QuotaFreshnessStatus::Offline, Language::En) => "offline (cached)",
        (QuotaFreshnessStatus::Unknown, Language::Zh) => "未知",
        (QuotaFreshnessStatus::Unknown, Language::En) => "unknown",
    }
    .to_string()
}

fn pool_state_color(p: Palette, pool: &PoolQuotaAnalytics, ui: &UiState) -> Color {
    if ui.needs_snapshot_refresh {
        return p.accent;
    }
    if !pool.identity.aggregation_eligible
        || matches!(
            pool.freshness,
            QuotaFreshnessStatus::Stale | QuotaFreshnessStatus::Offline
        )
    {
        return p.warn;
    }
    if pool
        .remote_remaining
        .as_ref()
        .is_some_and(QuotaQuantity::is_zero)
    {
        return p.bad;
    }
    p.good
}

fn pool_age(pool: &PoolQuotaAnalytics) -> String {
    duration_short(now_ms().saturating_sub(pool.observed_at_ms))
}

fn reset_text(pool: &PoolQuotaAnalytics, now_ms: u64, lang: Language) -> String {
    if pool.pacing.status == QuotaPaceStatus::Unlimited {
        return pace_label(pool.pacing.status, lang);
    }
    if pool.pacing.status == QuotaPaceStatus::NoReset
        || pool.window.kind == QuotaWindowKind::Resetless
    {
        return pace_label(QuotaPaceStatus::NoReset, lang);
    }
    let Some(reset_at_ms) = pool.pacing.reset_at_ms else {
        return match lang {
            Language::Zh => "未知",
            Language::En => "unknown",
        }
        .to_string();
    };
    let duration = duration_short(reset_at_ms.saturating_sub(now_ms));
    if pool.window.allows_midnight_label() {
        match lang {
            Language::Zh => format!("午夜前 {duration}"),
            Language::En => format!("midnight in {duration}"),
        }
    } else {
        match lang {
            Language::Zh => format!("{duration} 后重置"),
            Language::En => format!("reset in {duration}"),
        }
    }
}

fn gap_color(p: Palette, gap: crate::quota_analytics::SignedUsdDelta) -> Color {
    if gap.femto_usd() < 0 { p.warn } else { p.muted }
}

fn reconciliation_summary(pool: &PoolQuotaAnalytics, lang: Language) -> String {
    let reconciliation = &pool.reconciliation;
    let status = match (reconciliation.status, lang) {
        (QuotaReconciliationStatus::Available, Language::Zh) => "可对账",
        (QuotaReconciliationStatus::Available, Language::En) => "reconciled",
        (QuotaReconciliationStatus::IncompleteCoverage, Language::Zh) => "覆盖不完整",
        (QuotaReconciliationStatus::IncompleteCoverage, Language::En) => "incomplete coverage",
        (QuotaReconciliationStatus::IncompatibleUnit, Language::Zh) => "单位不兼容",
        (QuotaReconciliationStatus::IncompatibleUnit, Language::En) => "incompatible unit",
        (QuotaReconciliationStatus::IncompatibleGeneration, Language::Zh) => "换算版本不兼容",
        (QuotaReconciliationStatus::IncompatibleGeneration, Language::En) => {
            "incompatible conversion"
        }
        (QuotaReconciliationStatus::StaleRemote, Language::Zh) => "远端已过期",
        (QuotaReconciliationStatus::StaleRemote, Language::En) => "stale remote",
        (_, Language::Zh) => "暂不可对账",
        (_, Language::En) => "reconciliation unavailable",
    };
    format!(
        "{} | local={} unknown={} external={} gap={} | {}",
        status,
        quantity_text(reconciliation.local_known.as_ref()),
        quantity_text(reconciliation.local_unknown.as_ref()),
        quantity_text(reconciliation.external_unattributed.as_ref()),
        reconciliation
            .signed_delta
            .map(signed_usd_text)
            .unwrap_or_else(|| "-".to_string()),
        attribution_coverage_label(&reconciliation.coverage, lang)
    )
}

fn attribution_coverage_label(
    coverage: &crate::state::AttributionCoverage,
    lang: Language,
) -> &'static str {
    if coverage.time_truncated || coverage.count_truncated || coverage.leading_boundary_partial {
        return match lang {
            Language::Zh => "已截断",
            Language::En => "truncated",
        };
    }
    if !coverage.complete_for_reconciliation() {
        return match lang {
            Language::Zh => "覆盖不完整",
            Language::En => "incomplete coverage",
        };
    }
    match lang {
        Language::Zh => "覆盖完整",
        Language::En => "complete coverage",
    }
}

fn avg_duration(bucket: &UsageBucket) -> String {
    bucket
        .duration_ms_total
        .checked_div(bucket.requests_total)
        .map(duration_short)
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Instant;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;
    use crate::quota_analytics::{
        QuotaAnalyticsView, QuotaPacingView, QuotaProjectRow, QuotaReconciliationView,
    };
    use crate::quota_pool::{
        IdentityEvidence, PoolIdentity, QuotaCapabilities, QuotaResetKind, QuotaScope,
        QuotaWindowSemantics,
    };
    use crate::runtime_identity::ProviderEndpointKey;
    use crate::sessions::{ProjectIdentity, ProjectIdentityKind};
    use crate::state::{
        BalanceSnapshotStatus, ProviderBalanceSnapshot, UsageDayCoverage, UsageDayDimensionRow,
        UsageDayHourRow, UsageDayView, UsageRetryGateReasonRow, UsageRetryGateSummary,
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

    fn sample_quota_analytics() -> QuotaAnalyticsView {
        let generated_at_ms = 10 * 60 * 60_000;
        let usd = |value| QuotaQuantity::from_integer(value, QuotaUnit::Usd);
        QuotaAnalyticsView {
            support: QuotaAnalyticsSupport::Supported,
            generated_at_ms,
            registry_generation: 7,
            pools: vec![PoolQuotaAnalytics {
                identity: PoolIdentity {
                    key: "explicit:relay.example:account:input20".to_string(),
                    origin: "https://relay.example".to_string(),
                    scope: QuotaScope::Account,
                    revision: 2,
                    evidence: IdentityEvidence::ExplicitPoolId,
                    confidence: IdentityConfidence::Medium,
                    aggregation_eligible: true,
                    conflicting_evidence: false,
                },
                observed_at_ms: generated_at_ms - 60_000,
                last_success_at_ms: Some(generated_at_ms - 60_000),
                last_attempt_at_ms: Some(generated_at_ms - 60_000),
                freshness: QuotaFreshnessStatus::Fresh,
                source: "sub2api".to_string(),
                unit: QuotaUnit::Usd,
                capabilities: QuotaCapabilities {
                    used: true,
                    remaining: true,
                    direct_total: true,
                    limit: true,
                    reset: true,
                    window: true,
                    cumulative: true,
                    ..QuotaCapabilities::default()
                },
                window: QuotaWindowSemantics {
                    kind: QuotaWindowKind::CalendarDay,
                    reset: QuotaResetKind::ExplicitTimestamp,
                    reset_timezone: Some("Asia/Shanghai".to_string()),
                    ..QuotaWindowSemantics::default()
                },
                epoch_start_ms: 0,
                epoch_end_ms: Some(generated_at_ms + 8 * 60 * 60_000),
                remote_used: Some(usd(100)),
                remote_direct_total: Some(usd(100)),
                remote_remaining: Some(usd(40)),
                remote_limit: Some(usd(140)),
                observed_burn: Some(usd(100)),
                rate_15m: crate::quota_analytics::QuotaRateWindow {
                    status: QuotaRateStatus::Available,
                    rate_per_hour: Some(usd(12)),
                    sample_count: 4,
                    span_ms: 15 * 60_000,
                    ..crate::quota_analytics::QuotaRateWindow::default()
                },
                rate_60m: crate::quota_analytics::QuotaRateWindow {
                    status: QuotaRateStatus::Available,
                    rate_per_hour: Some(usd(10)),
                    sample_count: 12,
                    span_ms: 60 * 60_000,
                    ..crate::quota_analytics::QuotaRateWindow::default()
                },
                pacing: QuotaPacingView {
                    status: QuotaPaceStatus::OnPace,
                    required_rate_per_hour: Some(usd(5)),
                    pace_ratio_basis_points: Some(10_000),
                    exhaustion_eta_ms: Some(4 * 60 * 60_000),
                    projected_remaining_at_reset: Some(usd(0)),
                    reset_at_ms: Some(generated_at_ms + 8 * 60 * 60_000),
                },
                reconciliation: QuotaReconciliationView {
                    status: QuotaReconciliationStatus::Available,
                    remote_total: Some(usd(100)),
                    local_known: Some(usd(55)),
                    local_unknown: Some(usd(5)),
                    external_unattributed: Some(usd(40)),
                    signed_delta: Some(crate::quota_analytics::SignedUsdDelta::from_femto_usd(
                        40 * 10_i128.pow(15),
                    )),
                    projects: vec![QuotaProjectRow {
                        project: ProjectIdentity {
                            kind: ProjectIdentityKind::GitRoot,
                            path: Some("F:/SourceCodes/Rust/codex-helper".to_string()),
                        },
                        local_cost: usd(55),
                        requests: 11,
                    }],
                    ..QuotaReconciliationView::default()
                },
                ..PoolQuotaAnalytics::default()
            }],
            omitted_pools: 0,
        }
    }

    fn sample_snapshot() -> Snapshot {
        let mut summary = bucket(12, 4_159);
        summary.requests_error = 2;
        summary.usage.input_tokens = 2_101;
        summary.usage.output_tokens = 905;
        summary.usage.reasoning_tokens = 321;
        summary.usage.cache_read_input_tokens = 777;
        summary.usage.cache_creation_input_tokens = 55;
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
                summary,
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
            quota_analytics: sample_quota_analytics(),
            usage_rollup: crate::state::UsageRollupView::default(),
            provider_balances: HashMap::new(),
            routing: None,
            pricing_catalog: crate::pricing::ModelPriceCatalogSnapshot {
                source: "remote-catalog".to_string(),
                ..Default::default()
            },
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
    fn stats_render_prioritizes_remote_pool_and_pace() {
        let snapshot = sample_snapshot();
        let mut ui = UiState {
            page: crate::tui::types::Page::Stats,
            stats_focus: StatsFocus::Pools,
            ..UiState::default()
        };

        let text = render_text(128, 26, &mut ui, &snapshot);

        assert!(text.contains("Upstream Quota Pool"));
        assert!(text.contains("relay.example"));
        assert!(text.contains("on pace"));
        assert!(text.contains("$100"));
        assert!(text.contains("Retry Gate"));
        assert!(text.contains("Coverage"));
    }

    #[test]
    fn stats_render_surfaces_exact_matching_upstream_usage_report() {
        let mut snapshot = sample_snapshot();
        let endpoint = ProviderEndpointKey::new("codex", "input", "default");
        let (quota_pool_key, quota_pool_revision) = {
            let pool = snapshot
                .quota_analytics
                .pools
                .first_mut()
                .expect("sample quota pool");
            pool.endpoint = Some(endpoint.clone());
            pool.observation_provider_id = "sub2api-usage".to_string();
            pool.source = "usage_provider:sub2api_usage".to_string();
            (pool.identity.key.clone(), pool.identity.revision)
        };
        snapshot.provider_balances.insert(
            "input".to_string(),
            vec![ProviderBalanceSnapshot {
                observation_provider_id: "sub2api-usage".to_string(),
                provider_endpoint: endpoint,
                source: "usage_provider:sub2api_usage".to_string(),
                quota_pool_key: Some(quota_pool_key),
                quota_pool_revision: Some(quota_pool_revision),
                status: BalanceSnapshotStatus::Ok,
                usage_rate: Some(codex_helper_core::balance::ProviderUsageRateSnapshot {
                    average_duration_ms: Some("842.7".to_string()),
                    rpm: Some("0.7".to_string()),
                    tpm: Some("85.3".to_string()),
                }),
                usage_windows: vec![codex_helper_core::balance::ProviderUsageWindow {
                    period: "daily".to_string(),
                    used_usd: Some("95".to_string()),
                    limit_usd: Some("100".to_string()),
                    remaining_usd: Some("5".to_string()),
                    unlimited: Some(false),
                }],
                ..ProviderBalanceSnapshot::default()
            }],
        );

        for (width, height, expected) in [
            (
                128,
                30,
                vec![
                    "API rate RPM 0.7 / TPM 85.3",
                    "842.7ms",
                    "win daily $95/$100 left $5",
                ],
            ),
            (
                76,
                26,
                vec![
                    "RPM 0.7",
                    "842.7ms",
                    "daily used $95.00 left $5.00 / $100.00",
                ],
            ),
            (100, 19, vec!["API rate", "required", "ETA"]),
            (100, 30, vec!["API rate", "API avg", "required", "ETA"]),
            (48, 18, vec!["RPM 0.7", "daily $95/$100 left $5"]),
        ] {
            let mut ui = UiState {
                page: crate::tui::types::Page::Stats,
                stats_focus: StatsFocus::Pools,
                ..UiState::default()
            };
            let text = render_text(width, height, &mut ui, &snapshot);

            for expected in expected {
                assert!(
                    text.contains(expected),
                    "missing {expected:?} at {width}x{height}:\n{text}"
                );
            }
        }

        let rate = snapshot
            .provider_balances
            .get_mut("input")
            .and_then(|balances| balances.first_mut())
            .and_then(|balance| balance.usage_rate.as_mut())
            .expect("upstream usage rate");
        rate.rpm = Some("9".repeat(64));
        rate.tpm = Some("8".repeat(64));
        rate.average_duration_ms = Some("7".repeat(64));

        for (width, height, expected) in [
            (100, 19, &["API rate", "required", "ETA"][..]),
            (100, 30, &["API rate", "API avg", "required", "ETA"][..]),
        ] {
            let mut ui = UiState {
                page: crate::tui::types::Page::Stats,
                stats_focus: StatsFocus::Pools,
                ..UiState::default()
            };
            let text = render_text(width, height, &mut ui, &snapshot);

            for expected in expected {
                assert!(
                    text.contains(expected),
                    "long rate value hid {expected:?} at {width}x{height}:\n{text}"
                );
            }
        }
    }

    #[test]
    fn provider_usage_report_does_not_cross_service_boundaries() {
        let mut snapshot = sample_snapshot();
        let (quota_pool_key, quota_pool_revision) = {
            let pool = snapshot
                .quota_analytics
                .pools
                .first_mut()
                .expect("sample quota pool");
            pool.endpoint = Some(ProviderEndpointKey::new("codex", "input", "default"));
            pool.observation_provider_id = "sub2api-usage".to_string();
            pool.source = "usage_provider:sub2api_usage".to_string();
            (pool.identity.key.clone(), pool.identity.revision)
        };
        snapshot.provider_balances.insert(
            "input".to_string(),
            vec![ProviderBalanceSnapshot {
                observation_provider_id: "sub2api-usage".to_string(),
                provider_endpoint: ProviderEndpointKey::new("responses", "input", "default"),
                source: "usage_provider:sub2api_usage".to_string(),
                quota_pool_key: Some(quota_pool_key),
                quota_pool_revision: Some(quota_pool_revision),
                status: BalanceSnapshotStatus::Ok,
                usage_rate: Some(codex_helper_core::balance::ProviderUsageRateSnapshot {
                    rpm: Some("999".to_string()),
                    ..Default::default()
                }),
                ..ProviderBalanceSnapshot::default()
            }],
        );

        let pool = snapshot
            .quota_analytics
            .pools
            .first()
            .expect("sample quota pool")
            .clone();
        assert!(provider_usage_snapshot_for_pool(&snapshot, &pool).is_none());
    }

    #[test]
    fn provider_usage_report_requires_matching_pool_observer_and_source() {
        let mut snapshot = sample_snapshot();
        let endpoint = ProviderEndpointKey::new("codex", "input", "default");
        let (quota_pool_key, quota_pool_revision) = {
            let pool = snapshot
                .quota_analytics
                .pools
                .first_mut()
                .expect("sample quota pool");
            pool.endpoint = Some(endpoint.clone());
            pool.observation_provider_id = "observer-a".to_string();
            pool.source = "usage_provider:sub2api_usage".to_string();
            (pool.identity.key.clone(), pool.identity.revision)
        };
        snapshot.provider_balances.insert(
            "input".to_string(),
            vec![
                ProviderBalanceSnapshot {
                    observation_provider_id: "observer-b".to_string(),
                    provider_endpoint: endpoint.clone(),
                    source: "usage_provider:sub2api_usage".to_string(),
                    quota_pool_key: Some(quota_pool_key.clone()),
                    quota_pool_revision: Some(quota_pool_revision),
                    status: BalanceSnapshotStatus::Ok,
                    usage_rate: Some(codex_helper_core::balance::ProviderUsageRateSnapshot {
                        rpm: Some("999".to_string()),
                        ..Default::default()
                    }),
                    ..ProviderBalanceSnapshot::default()
                },
                ProviderBalanceSnapshot {
                    observation_provider_id: "observer-a".to_string(),
                    provider_endpoint: endpoint.clone(),
                    source: "usage_provider:sub2api_usage".to_string(),
                    quota_pool_key: Some("credential:sha256:old".to_string()),
                    quota_pool_revision: Some(quota_pool_revision),
                    status: BalanceSnapshotStatus::Ok,
                    usage_rate: Some(codex_helper_core::balance::ProviderUsageRateSnapshot {
                        rpm: Some("998".to_string()),
                        ..Default::default()
                    }),
                    ..ProviderBalanceSnapshot::default()
                },
            ],
        );

        let pool = snapshot
            .quota_analytics
            .pools
            .first()
            .expect("sample quota pool")
            .clone();
        assert!(provider_usage_snapshot_for_pool(&snapshot, &pool).is_none());

        snapshot
            .provider_balances
            .get_mut("input")
            .expect("provider balances")
            .push(ProviderBalanceSnapshot {
                observation_provider_id: "observer-a".to_string(),
                provider_endpoint: endpoint,
                source: "usage_provider:sub2api_usage".to_string(),
                quota_pool_key: Some(quota_pool_key),
                quota_pool_revision: Some(quota_pool_revision),
                status: BalanceSnapshotStatus::Ok,
                usage_rate: Some(codex_helper_core::balance::ProviderUsageRateSnapshot {
                    rpm: Some("0.7".to_string()),
                    ..Default::default()
                }),
                ..ProviderBalanceSnapshot::default()
            });

        assert_eq!(
            provider_usage_snapshot_for_pool(&snapshot, &pool)
                .and_then(|balance| balance.usage_rate.as_ref())
                .and_then(|rate| rate.rpm.as_deref()),
            Some("0.7")
        );
    }

    #[test]
    fn provider_usage_report_rejects_retained_error_snapshot() {
        let mut snapshot = sample_snapshot();
        let endpoint = ProviderEndpointKey::new("codex", "input", "default");
        let (quota_pool_key, quota_pool_revision) = {
            let pool = snapshot
                .quota_analytics
                .pools
                .first_mut()
                .expect("sample quota pool");
            pool.endpoint = Some(endpoint.clone());
            pool.observation_provider_id = "observer-a".to_string();
            pool.source = "usage_provider:sub2api_usage".to_string();
            (pool.identity.key.clone(), pool.identity.revision)
        };
        snapshot.provider_balances.insert(
            "input".to_string(),
            vec![ProviderBalanceSnapshot {
                observation_provider_id: "observer-a".to_string(),
                provider_endpoint: endpoint,
                source: "usage_provider:sub2api_usage".to_string(),
                quota_pool_key: Some(quota_pool_key),
                quota_pool_revision: Some(quota_pool_revision),
                status: BalanceSnapshotStatus::Error,
                usage_rate: Some(codex_helper_core::balance::ProviderUsageRateSnapshot {
                    rpm: Some("0.7".to_string()),
                    ..Default::default()
                }),
                ..ProviderBalanceSnapshot::default()
            }],
        );

        let pool = snapshot
            .quota_analytics
            .pools
            .first()
            .expect("sample quota pool");
        assert!(provider_usage_snapshot_for_pool(&snapshot, pool).is_none());
    }

    #[test]
    fn stats_render_preserves_local_usage_and_retry_diagnostics_across_terminal_sizes() {
        let snapshot = sample_snapshot();

        for (width, height) in [(80, 24), (100, 30), (160, 45)] {
            let mut ui = UiState {
                page: crate::tui::types::Page::Stats,
                stats_focus: StatsFocus::Pools,
                ..UiState::default()
            };
            let text = render_text(width, height, &mut ui, &snapshot);

            for expected in [
                "Local Usage",
                "2.1k",
                "905",
                "reasoning",
                "321",
                "cache read",
                "777",
                "cache write",
                "55",
                "Retry Gate",
                "runtime_store",
            ] {
                assert!(
                    text.contains(expected),
                    "missing {expected:?} at {width}x{height}:\n{text}"
                );
            }
        }
    }

    #[test]
    fn stats_render_keeps_narrow_layout_bounded() {
        let snapshot = sample_snapshot();
        let mut ui = UiState {
            page: crate::tui::types::Page::Stats,
            stats_focus: StatsFocus::Projects,
            ..UiState::default()
        };

        let text = render_text(76, 22, &mut ui, &snapshot);

        assert!(text.contains("Project Attribution"));
        assert!(text.contains("F:/SourceCodes/Rust/codex-helper"));
        assert!(text.contains("external"));
        assert!(text.contains("required"), "{text}");
        assert!(text.contains("ETA"), "{text}");
        assert!(text.contains("confidence"), "{text}");
    }

    #[test]
    fn remote_usage_label_never_calls_partial_observation_today() {
        let mut pool = sample_quota_analytics().pools.remove(0);
        pool.remote_direct_total = None;
        pool.remote_used = None;
        pool.observed_burn = Some(QuotaQuantity::from_integer(12, QuotaUnit::Usd));
        pool.epoch_start_ms = 10 * 60 * 60_000;

        let display = remote_usage_display(&pool, Language::En);

        assert!(display.label.starts_with("observed since "));
        assert!(!display.label.contains("today"));
        assert_eq!(display.quantity, pool.observed_burn.as_ref());
    }

    #[test]
    fn coverage_label_uses_the_complete_reconciliation_contract() {
        let mut coverage = crate::state::AttributionCoverage {
            unpriced_requests: 1,
            ..crate::state::AttributionCoverage::default()
        };

        assert_eq!(
            attribution_coverage_label(&coverage, Language::En),
            "incomplete coverage"
        );
        coverage.unpriced_requests = 0;
        assert_eq!(
            attribution_coverage_label(&coverage, Language::En),
            "complete coverage"
        );
    }

    #[test]
    fn stats_render_distinguishes_unsupported_read_model_from_empty_supported() {
        let mut snapshot = sample_snapshot();
        snapshot.quota_analytics = QuotaAnalyticsView::default();
        let mut ui = UiState {
            page: crate::tui::types::Page::Stats,
            stats_focus: StatsFocus::Pools,
            ..UiState::default()
        };

        let unsupported = render_text(76, 22, &mut ui, &snapshot);
        assert!(unsupported.contains("unsupported"));

        snapshot.quota_analytics.support = QuotaAnalyticsSupport::Supported;
        let empty = render_text(76, 22, &mut ui, &snapshot);
        assert!(empty.contains("no supported quota adapter"));
        assert!(!empty.contains("unsupported read model"));
    }

    struct QuotaPageScenario {
        name: &'static str,
        visible_label: &'static str,
        compact_kpis: &'static [&'static str],
        configure: fn(&mut Snapshot, &mut UiState),
    }

    #[test]
    fn quota_page_state_matrix_keeps_compact_kpis_and_selected_row_visible() {
        let scenarios = [
            QuotaPageScenario {
                name: "syncing",
                visible_label: "syncing",
                compact_kpis: &["today used $100", "remaining $40", "15m $12/h", "60m $10/h"],
                configure: |_, ui| ui.needs_snapshot_refresh = true,
            },
            QuotaPageScenario {
                name: "offline cached",
                visible_label: "offline (cached)",
                compact_kpis: &["today used $100", "remaining $40", "pace on pace"],
                configure: |snapshot, _| {
                    snapshot.quota_analytics.pools[0].freshness = QuotaFreshnessStatus::Offline;
                },
            },
            QuotaPageScenario {
                name: "unsupported",
                visible_label: "unsupported",
                compact_kpis: &[
                    "current operator read model has no quota analytics",
                    "local cost",
                    "local tokens",
                ],
                configure: |snapshot, _| {
                    snapshot.quota_analytics.support = QuotaAnalyticsSupport::Unsupported;
                },
            },
            QuotaPageScenario {
                name: "ambiguous identity",
                visible_label: "ambiguous identity",
                compact_kpis: &["today used $100", "remaining $40", "confidence medium"],
                configure: |snapshot, _| {
                    snapshot.quota_analytics.pools[0]
                        .identity
                        .aggregation_eligible = false;
                },
            },
            QuotaPageScenario {
                name: "unlimited",
                visible_label: "unlimited",
                compact_kpis: &["today used $100", "pace unlimited", "reset unlimited"],
                configure: |snapshot, _| {
                    let pool = &mut snapshot.quota_analytics.pools[0];
                    pool.capabilities.unlimited = true;
                    pool.pacing.status = QuotaPaceStatus::Unlimited;
                    pool.pacing.required_rate_per_hour = None;
                    pool.pacing.exhaustion_eta_ms = None;
                },
            },
            QuotaPageScenario {
                name: "exhausted",
                visible_label: "exhausted",
                compact_kpis: &["today used $100", "remaining $0", "required $5/h"],
                configure: |snapshot, _| {
                    snapshot.quota_analytics.pools[0].remote_remaining =
                        Some(QuotaQuantity::from_integer(0, QuotaUnit::Usd));
                },
            },
            QuotaPageScenario {
                name: "just reset",
                visible_label: "just reset / rollback",
                compact_kpis: &["today used $100", "remaining $40", "15m $12/h"],
                configure: |snapshot, _| {
                    snapshot.quota_analytics.pools[0].latest_adjustment =
                        Some(crate::quota_pool::QuotaAdjustmentKind::CounterResetOrRollback);
                },
            },
            QuotaPageScenario {
                name: "low sample",
                visible_label: "low sample",
                compact_kpis: &["15m low sample", "60m low sample", "pace low sample"],
                configure: |snapshot, _| {
                    let pool = &mut snapshot.quota_analytics.pools[0];
                    pool.rate_15m = crate::quota_analytics::QuotaRateWindow {
                        status: QuotaRateStatus::InsufficientSamples,
                        sample_count: 2,
                        ..crate::quota_analytics::QuotaRateWindow::default()
                    };
                    pool.rate_60m = crate::quota_analytics::QuotaRateWindow {
                        status: QuotaRateStatus::ShortSpan,
                        sample_count: 2,
                        ..crate::quota_analytics::QuotaRateWindow::default()
                    };
                    pool.pacing = QuotaPacingView {
                        status: QuotaPaceStatus::LowSample,
                        ..QuotaPacingView::default()
                    };
                },
            },
        ];

        for scenario in scenarios {
            let mut snapshot = sample_snapshot();
            let mut ui = UiState {
                page: crate::tui::types::Page::Stats,
                stats_focus: StatsFocus::Pools,
                ..UiState::default()
            };
            ui.stats_pools_table.select(Some(0));
            (scenario.configure)(&mut snapshot, &mut ui);

            let text = render_text(76, 22, &mut ui, &snapshot);
            let lines = text.lines().collect::<Vec<_>>();
            assert_eq!(
                lines.len(),
                22,
                "{} rendered outside 76x22: {text}",
                scenario.name
            );
            let compact = lines[..6].join("\n");
            let table = lines[6..].join("\n");

            assert!(
                compact.contains(scenario.visible_label),
                "{} did not show `{}` in compact summary: {text}",
                scenario.name,
                scenario.visible_label
            );
            for expected in scenario.compact_kpis {
                assert!(
                    compact.contains(expected),
                    "{} did not show compact KPI `{expected}`: {text}",
                    scenario.name
                );
            }
            assert!(
                table.contains("relay.example"),
                "{} overwrote or clipped the selected pool row: {text}",
                scenario.name
            );
        }
    }

    #[test]
    fn stats_render_preserves_negative_gap_and_external_floor() {
        let mut snapshot = sample_snapshot();
        let reconciliation = &mut snapshot.quota_analytics.pools[0].reconciliation;
        reconciliation.remote_total = Some(QuotaQuantity::from_integer(50, QuotaUnit::Usd));
        reconciliation.local_known = Some(QuotaQuantity::from_integer(60, QuotaUnit::Usd));
        reconciliation.local_unknown = None;
        reconciliation.external_unattributed = Some(QuotaQuantity::from_integer(0, QuotaUnit::Usd));
        reconciliation.signed_delta = Some(crate::quota_analytics::SignedUsdDelta::from_femto_usd(
            -10 * 10_i128.pow(15),
        ));
        let mut ui = UiState {
            page: crate::tui::types::Page::Stats,
            stats_focus: StatsFocus::Projects,
            ..UiState::default()
        };

        let text = render_text(128, 26, &mut ui, &snapshot);

        assert!(text.contains("external / unattributed"), "{text}");
        assert!(text.contains("$0"), "{text}");
        assert!(text.contains("-$10"), "{text}");
        assert!(!text.contains("$-10"), "{text}");
    }

    #[test]
    fn stats_render_keeps_raw_remote_and_mismatched_local_values_separate() {
        let mut snapshot = sample_snapshot();
        let pool = &mut snapshot.quota_analytics.pools[0];
        pool.unit = QuotaUnit::Raw;
        pool.remote_used = None;
        pool.remote_direct_total = Some(QuotaQuantity::from_integer(500_000, QuotaUnit::Raw));
        pool.remote_remaining = Some(QuotaQuantity::from_integer(250_000, QuotaUnit::Raw));
        pool.remote_limit = Some(QuotaQuantity::from_integer(750_000, QuotaUnit::Raw));
        pool.observed_burn = None;
        pool.reconciliation.status = QuotaReconciliationStatus::IncompatibleGeneration;
        pool.reconciliation.remote_total =
            Some(QuotaQuantity::from_integer(500_000, QuotaUnit::Raw));
        pool.reconciliation.local_known = Some(
            QuotaQuantity::from_integer(1, QuotaUnit::Usd).with_conversion_generation(Some(42)),
        );
        pool.reconciliation.local_unknown = None;
        pool.reconciliation.external_unattributed = None;
        pool.reconciliation.signed_delta = None;
        let mut ui = UiState {
            page: crate::tui::types::Page::Stats,
            stats_focus: StatsFocus::Projects,
            ..UiState::default()
        };

        let text = render_text(128, 26, &mut ui, &snapshot);

        assert!(text.contains("500000 raw"), "{text}");
        assert!(text.contains("$1"), "{text}");
        assert!(text.contains("incompatible conversion"), "{text}");
        assert!(!text.contains("external / unattributed"), "{text}");
    }

    #[test]
    fn resetless_pool_does_not_claim_midnight_reset() {
        let mut snapshot = sample_snapshot();
        let pool = &mut snapshot.quota_analytics.pools[0];
        pool.window.kind = QuotaWindowKind::Resetless;
        pool.window.reset = QuotaResetKind::NoReset;
        pool.rate_60m = crate::quota_analytics::QuotaRateWindow {
            status: QuotaRateStatus::NoCounter,
            ..crate::quota_analytics::QuotaRateWindow::default()
        };
        pool.pacing = QuotaPacingView {
            status: QuotaPaceStatus::NoReset,
            ..QuotaPacingView::default()
        };
        assert_eq!(rate_text(&pool.rate_60m, Language::En), "-");
        let mut ui = UiState {
            page: crate::tui::types::Page::Stats,
            stats_focus: StatsFocus::Pools,
            ..UiState::default()
        };

        let text = render_text(76, 22, &mut ui, &snapshot);

        assert!(text.contains("no reset"), "{text}");
        assert!(!text.contains("midnight"), "{text}");
    }
}
