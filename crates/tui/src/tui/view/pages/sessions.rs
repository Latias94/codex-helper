use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::state::{ResolvedRouteValue, RouteValueSource};
use crate::tui::i18n;
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_status_style, basename, format_age,
    format_observed_client_identity, format_tok_per_second, now_ms, session_control_posture_lang,
    session_observation_scope_label_lang, session_observed_provider_balance_brief_lang,
    session_observed_provider_balance_snapshot, session_transcript_host_status_lang, short_sid,
    shorten, shorten_middle, status_style, tokens_short, usage_line_lang,
};
use crate::tui::state::UiState;
use crate::tui::view::widgets::kv_line;

pub(super) fn render_sessions_page(
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
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let filtered = ui.filtered_sessions_page_indices(snapshot);

    let title = format!(
        "{}  ({}: {}, {}: {})",
        l("Sessions"),
        l("active_only"),
        if ui.sessions_page_active_only {
            l("on")
        } else {
            l("off")
        },
        l("errors_only"),
        if ui.sessions_page_errors_only {
            l("on")
        } else {
            l("off")
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

    let header = Row::new([
        l("Session"),
        l("CWD"),
        "A",
        "St",
        l("Last"),
        l("turns"),
        "Tok",
        "tok/s",
        l("profile"),
    ])
    .style(Style::default().fg(p.muted))
    .height(1);

    let now = now_ms();
    let rows = filtered
        .iter()
        .filter_map(|idx| snapshot.rows.get(*idx))
        .map(|row| {
            let sid = row
                .session_id
                .as_deref()
                .map(|s| short_sid(s, 18))
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
            let tok_per_second = format_tok_per_second(row.last_output_tokens_per_second);
            let profile = row
                .binding_profile_name
                .as_deref()
                .map(|s| shorten(s, 12))
                .unwrap_or_else(|| "-".to_string());

            let mut style = Style::default().fg(p.text);
            if row.last_status.is_some_and(|s| s >= 500) {
                style = style.fg(p.bad);
            } else if row.last_status.is_some_and(|s| s >= 400) {
                style = style.fg(p.warn);
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
                Cell::from(Span::styled(tok_per_second, Style::default().fg(p.accent))),
                Cell::from(Span::styled(
                    profile,
                    Style::default().fg(if row.binding_profile_name.is_some() {
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
            Constraint::Length(7),
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
        .and_then(|idx| snapshot.rows.get(*idx));
    let mut lines = Vec::new();
    if let Some(row) = selected {
        let sid_full = row.session_id.as_deref().unwrap_or("-");
        let cwd_full = row
            .cwd
            .as_deref()
            .map(|s| shorten_middle(s, 80))
            .unwrap_or_else(|| "-".to_string());
        let identity_source = session_observation_scope_label_lang(row.observation_scope, lang);
        let transcript_status = session_transcript_host_status_lang(row, lang);
        let transcript_path = row
            .host_local_transcript_path
            .as_deref()
            .map(|path| shorten_middle(path, 80));
        let client_full = format_observed_client_identity(
            row.last_client_name.as_deref(),
            row.last_client_addr.as_deref(),
        )
        .unwrap_or_else(|| "-".to_string());
        let observed_model = row.last_model.as_deref().unwrap_or("-");
        let provider = row.observed_provider_id().unwrap_or("-");
        let observed_endpoint = row
            .observed_provider_id()
            .zip(row.observed_endpoint_id())
            .map(|(provider_id, endpoint_id)| format!("{provider_id}/{endpoint_id}"))
            .unwrap_or_else(|| "-".to_string());
        let observed_route_path = row
            .last_route_decision
            .as_ref()
            .map(|decision| decision.route_path.join(" / "))
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| "-".to_string());
        let balance = session_observed_provider_balance_brief_lang(
            row,
            &snapshot.provider_balances,
            64,
            lang,
        )
        .unwrap_or_else(|| "-".to_string());
        let balance_style =
            session_observed_provider_balance_snapshot(row, &snapshot.provider_balances)
                .map(|snapshot| balance_snapshot_status_style(p, snapshot))
                .unwrap_or_else(|| Style::default().fg(p.muted));
        let observed_upstream = row
            .observed_upstream_origin()
            .unwrap_or_else(|| "-".to_string());
        let observed_effort = row.last_reasoning_effort.as_deref().unwrap_or("-");
        let observed_service_tier = row.last_service_tier.as_deref().unwrap_or("-");
        let binding_profile = row.binding_profile_name.as_deref().unwrap_or("-");
        let binding_mode = row
            .binding_continuity_mode
            .map(|mode| format!("{mode:?}").to_ascii_lowercase())
            .unwrap_or_else(|| "-".to_string());
        let effective_model = format_resolved_route_value(row.effective_model.as_ref(), lang);
        let effective_effort =
            format_resolved_route_value(row.effective_reasoning_effort.as_ref(), lang);
        let effective_service_tier =
            format_resolved_route_value(row.effective_service_tier.as_ref(), lang);
        let posture = session_control_posture_lang(row, lang);
        let route_affinity = row
            .route_affinity
            .as_ref()
            .map(|affinity| {
                format!(
                    "endpoint={} upstream={} reason={}",
                    format_args!("{}/{}", affinity.provider_id, affinity.endpoint_id),
                    shorten_middle(&affinity.upstream_origin, 64),
                    affinity.change_reason
                )
            })
            .unwrap_or_else(|| "-".to_string());

        lines.push(kv_line(
            p,
            l("session"),
            sid_full.to_string(),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ));
        lines.push(kv_line(
            p,
            l("identity"),
            identity_source.to_string(),
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            l("transcript"),
            transcript_status,
            Style::default().fg(if row.host_local_transcript_path.is_some() {
                p.good
            } else {
                p.muted
            }),
        ));
        if let Some(path) = transcript_path {
            lines.push(kv_line(p, "tx_path", path, Style::default().fg(p.muted)));
        }
        lines.push(kv_line(
            p,
            l("client(last)"),
            client_full,
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(p, l("cwd"), cwd_full, Style::default().fg(p.text)));
        lines.push(kv_line(
            p,
            "binding",
            format!("{binding_profile} ({binding_mode})"),
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            l("control"),
            posture.headline,
            Style::default().fg(posture.color),
        ));
        lines.push(kv_line(
            p,
            l("explain"),
            posture.detail,
            Style::default().fg(p.muted),
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Observed route"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(kv_line(
            p,
            "model(last)",
            observed_model.to_string(),
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
            "endpoint",
            observed_endpoint,
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            "route_path",
            observed_route_path,
            Style::default().fg(p.muted),
        ));
        lines.push(kv_line(p, "balance", balance, balance_style));
        lines.push(kv_line(
            p,
            "upstream_origin",
            observed_upstream,
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            "effort(last)",
            observed_effort.to_string(),
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            "service_tier(last)",
            observed_service_tier.to_string(),
            Style::default().fg(p.text),
        ));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Effective route"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(kv_line(
            p,
            "model",
            effective_model,
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            "effort",
            effective_effort,
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            "service_tier",
            effective_service_tier,
            Style::default().fg(p.text),
        ));
        lines.push(kv_line(
            p,
            "session_affinity",
            route_affinity,
            Style::default().fg(if row.route_affinity.is_some() {
                p.accent
            } else {
                p.muted
            }),
        ));

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
            .map(|usage| usage_line_lang(usage, lang))
            .unwrap_or_else(|| "tok in/out/rsn/ttl: -".to_string());
        let last_tok_per_second = format_tok_per_second(row.last_output_tokens_per_second);
        let avg_tok_per_second = format_tok_per_second(row.avg_output_tokens_per_second);
        lines.push(kv_line(
            p,
            l("usage"),
            format!(
                "{total_usage} | turns {turns_total}/{turns_with_usage} | out_tok/s last={last_tok_per_second} avg={avg_tok_per_second}"
            ),
            Style::default().fg(p.muted),
        ));

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Keys"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(match lang {
            crate::tui::Language::Zh => "  a 切换仅活跃",
            crate::tui::Language::En => "  a toggle active-only",
        }));
        lines.push(Line::from(match lang {
            crate::tui::Language::Zh => "  e 切换仅错误",
            crate::tui::Language::En => "  e toggle errors-only",
        }));
        lines.push(Line::from(match lang {
            crate::tui::Language::Zh => "  r 重置筛选",
            crate::tui::Language::En => "  r reset filters",
        }));
        lines.push(Line::from(match lang {
            crate::tui::Language::Zh => "  t 打开对话记录（全屏）",
            crate::tui::Language::En => "  t transcript (full-screen)",
        }));
        lines.push(Line::from(match lang {
            crate::tui::Language::Zh => "  o 在 Requests 中打开会话",
            crate::tui::Language::En => "  o open session in Requests",
        }));
        lines.push(Line::from(match lang {
            crate::tui::Language::Zh => "  H 在 History 中打开会话",
            crate::tui::Language::En => "  H open session in History",
        }));
    } else {
        lines.push(Line::from(Span::styled(
            l("No sessions match the current filters."),
            Style::default().fg(p.muted),
        )));
    }

    let right_block = Block::default()
        .title(Span::styled(
            l("Session details"),
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

fn route_value_source_label(source: RouteValueSource, lang: crate::tui::Language) -> &'static str {
    match source {
        RouteValueSource::RequestPayload => i18n::label(lang, "request payload"),
        RouteValueSource::SessionOverride => i18n::label(lang, "session override"),
        RouteValueSource::GlobalOverride => i18n::label(lang, "global override"),
        RouteValueSource::ProfileDefault => i18n::label(lang, "profile default"),
        RouteValueSource::ProviderMapping => i18n::label(lang, "provider mapping"),
        RouteValueSource::RuntimeFallback => i18n::label(lang, "runtime fallback"),
    }
}

fn format_resolved_route_value(
    value: Option<&ResolvedRouteValue>,
    lang: crate::tui::Language,
) -> String {
    match value {
        Some(value) => format!(
            "{} [{}]",
            value.value,
            route_value_source_label(value.source, lang)
        ),
        None => "-".to_string(),
    }
}
