use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{
    Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Table, Wrap,
};

use crate::tui::ProviderOption;
use crate::tui::i18n;
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_status_style, basename, duration_short, format_age,
    format_observed_client_identity, now_ms, session_balance_brief_lang,
    session_control_posture_lang, session_observation_scope_label_lang,
    session_primary_balance_snapshot, session_row_has_any_override,
    session_transcript_host_status_lang, short_sid, shorten, shorten_middle, status_style,
    tokens_short, usage_line_lang,
};
use crate::tui::state::UiState;
use crate::tui::types::{Focus, Overlay};
use crate::tui::view::widgets::kv_line;

pub(super) fn render_dashboard(
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
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let title = Span::styled(
        l("Sessions"),
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
        Cell::from(Span::styled(l("Session"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("CWD"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("A"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("Last"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("Age"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("ΣTok"), Style::default().fg(p.muted))),
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
                .map(|s| short_sid(s, 18))
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
            if r.override_station_name.is_some() {
                badges.push(Span::styled("C", Style::default().fg(p.accent)));
            }
            if r.override_model.is_some() {
                badges.push(Span::styled("M", Style::default().fg(p.accent)));
            }
            if r.override_service_tier.is_some() {
                badges.push(Span::styled("T", Style::default().fg(p.accent)));
            }

            let mut session_spans = vec![Span::styled(sid, Style::default().fg(p.text))];
            for b in badges {
                session_spans.push(Span::raw(" "));
                session_spans.push(Span::raw("["));
                session_spans.push(b);
                session_spans.push(Span::raw("]"));
            }

            let mut row_style = Style::default().fg(p.text).bg(p.panel);
            if session_row_has_any_override(r) {
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
        .constraints([Constraint::Length(15), Constraint::Min(0)])
        .split(area);

    render_session_details(f, p, ui, snapshot, chunks[0]);
    render_requests_panel(f, p, ui, snapshot, providers, chunks[1]);
}

fn render_session_details(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let selected = snapshot.rows.get(ui.selected_session_idx);
    let sid = selected
        .and_then(|r| r.session_id.as_deref())
        .unwrap_or("-");
    let cwd = selected
        .and_then(|r| r.cwd.as_deref())
        .map(|s| shorten_middle(s, 64))
        .unwrap_or_else(|| "-".to_string());
    let identity = selected
        .map(|r| session_observation_scope_label_lang(r.observation_scope, lang).to_string())
        .unwrap_or_else(|| "-".to_string());
    let transcript = selected
        .map(|row| session_transcript_host_status_lang(row, lang))
        .unwrap_or_else(|| "-".to_string());
    let client = selected
        .and_then(|r| {
            format_observed_client_identity(
                r.last_client_name.as_deref(),
                r.last_client_addr.as_deref(),
            )
        })
        .unwrap_or_else(|| "-".to_string());

    let override_effort = selected
        .and_then(|r| r.override_effort.as_deref())
        .unwrap_or("-");
    let override_cfg = selected
        .and_then(|r| r.override_station_name.as_deref())
        .unwrap_or("-");
    let override_model = selected
        .and_then(|r| r.override_model.as_deref())
        .unwrap_or("-");
    let override_service_tier = selected
        .and_then(|r| r.override_service_tier.as_deref())
        .unwrap_or("-");
    let binding = selected
        .and_then(|r| r.binding_profile_name.as_deref())
        .unwrap_or("-");
    let model = selected
        .and_then(|r| r.last_model.as_deref())
        .unwrap_or("-");
    let provider = selected
        .and_then(|r| r.last_provider_id.as_deref())
        .unwrap_or("-");
    let balance =
        selected.and_then(|r| session_balance_brief_lang(r, &snapshot.provider_balances, 56, lang));
    let balance_snapshot =
        selected.and_then(|r| session_primary_balance_snapshot(r, &snapshot.provider_balances));
    let provider_line = match balance.as_deref() {
        Some(balance) if provider != "-" => format!("{provider} | {balance}"),
        Some(balance) => balance.to_string(),
        None => provider.to_string(),
    };
    let cfg = selected
        .and_then(|r| r.last_station_name.as_deref())
        .unwrap_or("-");
    let effort = selected
        .and_then(|r| r.override_effort.as_deref())
        .or_else(|| selected.and_then(|r| r.last_reasoning_effort.as_deref()))
        .unwrap_or("-");
    let service_tier = selected
        .and_then(|r| {
            r.override_service_tier
                .as_deref()
                .or(r
                    .effective_service_tier
                    .as_ref()
                    .map(|value| value.value.as_str()))
                .or(r.last_service_tier.as_deref())
        })
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
        .map(|usage| usage_line_lang(usage, lang))
        .unwrap_or_else(|| format!("{}: -", l("tok in/out/rsn/ttl")));

    let total_usage = selected
        .and_then(|r| r.total_usage.as_ref())
        .filter(|u| u.total_tokens > 0)
        .map(|usage| usage_line_lang(usage, lang))
        .unwrap_or_else(|| format!("{}: -", l("tok in/out/rsn/ttl")));
    let posture = selected.map(|row| {
        session_control_posture_lang(row, snapshot.global_station_override.as_deref(), lang)
    });

    let lines = vec![
        kv_line(
            p,
            l("session"),
            sid.to_string(),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
        kv_line(p, l("identity"), identity, Style::default().fg(p.text)),
        kv_line(
            p,
            l("transcript"),
            transcript,
            Style::default().fg(
                if selected
                    .and_then(|r| r.host_local_transcript_path.as_ref())
                    .is_some()
                {
                    p.good
                } else {
                    p.muted
                },
            ),
        ),
        kv_line(p, l("client"), client, Style::default().fg(p.text)),
        kv_line(p, l("cwd"), cwd, Style::default().fg(p.text)),
        kv_line(
            p,
            l("binding"),
            binding.to_string(),
            Style::default().fg(if binding == "-" { p.muted } else { p.text }),
        ),
        kv_line(
            p,
            l("control"),
            posture
                .as_ref()
                .map(|posture| posture.headline.clone())
                .unwrap_or_else(|| "-".to_string()),
            Style::default().fg(posture
                .as_ref()
                .map(|posture| posture.color)
                .unwrap_or(p.muted)),
        ),
        kv_line(
            p,
            l("model"),
            model.to_string(),
            Style::default().fg(p.text),
        ),
        kv_line(
            p,
            l("provider"),
            provider_line,
            balance_snapshot
                .map(|snapshot| balance_snapshot_status_style(p, snapshot))
                .unwrap_or_else(|| Style::default().fg(p.text)),
        ),
        kv_line(
            p,
            l("station"),
            cfg.to_string(),
            Style::default().fg(p.text),
        ),
        kv_line(
            p,
            l("effort"),
            effort.to_string(),
            Style::default().fg(if override_effort != "-" {
                p.accent
            } else {
                p.text
            }),
        ),
        kv_line(
            p,
            l("service_tier"),
            service_tier.to_string(),
            Style::default().fg(if override_service_tier != "-" {
                p.accent
            } else {
                p.text
            }),
        ),
        kv_line(
            p,
            l("override"),
            format!(
                "model={override_model}, effort={override_effort}, station={override_cfg}, tier={override_service_tier}"
            ),
            Style::default().fg(
                if override_model != "-"
                    || override_effort != "-"
                    || override_cfg != "-"
                    || override_service_tier != "-"
                {
                    p.accent
                } else {
                    p.muted
                },
            ),
        ),
        kv_line(
            p,
            l("activity"),
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
            l("usage"),
            format!("{last_usage} | sum {total_usage} | turns {turns_total}/{turns_with_usage}"),
            Style::default().fg(p.muted),
        ),
    ];

    let title = Span::styled(
        l("Details"),
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
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let focused = ui.focus == Focus::Requests && ui.overlay == Overlay::None;
    let title = Span::styled(
        snapshot
            .rows
            .get(ui.selected_session_idx)
            .and_then(|r| r.session_id.as_deref())
            .map(|sid| format!("{} [{}]", l("Requests"), sid))
            .unwrap_or_else(|| l("Requests").to_string()),
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
        Cell::from(Span::styled(l("Age"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("St"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("TTFB"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("Total"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("In"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("Out"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("CRead"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("CNew"), Style::default().fg(p.muted))),
        Cell::from(Span::styled(l("Tok"), Style::default().fg(p.muted))),
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
            let ttfb = r
                .ttfb_ms
                .map(duration_short)
                .unwrap_or_else(|| "-".to_string());
            let total_dur = duration_short(r.duration_ms);
            let usage = r.usage.as_ref();
            let input = usage
                .map(|u| tokens_short(u.input_tokens))
                .unwrap_or_else(|| "-".to_string());
            let output = usage
                .map(|u| tokens_short(u.output_tokens))
                .unwrap_or_else(|| "-".to_string());
            let cache_read = usage
                .map(|u| tokens_short(u.cache_read_tokens_total()))
                .unwrap_or_else(|| "-".to_string());
            let cache_new = usage
                .map(|u| tokens_short(u.cache_creation_tokens_total()))
                .unwrap_or_else(|| "-".to_string());
            let total_tokens = usage
                .map(|u| tokens_short(u.total_tokens))
                .unwrap_or_else(|| "-".to_string());

            Row::new(vec![
                Cell::from(Span::styled(age, Style::default().fg(p.muted))),
                Cell::from(Line::from(vec![status])),
                Cell::from(Span::styled(ttfb, Style::default().fg(p.muted))),
                Cell::from(Span::styled(total_dur, Style::default().fg(p.muted))),
                Cell::from(input),
                Cell::from(output),
                Cell::from(cache_read),
                Cell::from(cache_new),
                Cell::from(Span::styled(total_tokens, Style::default().fg(p.accent))),
            ])
            .style(Style::default().bg(p.panel).fg(p.text))
        })
        .collect::<Vec<_>>();

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Length(4),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(6),
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
