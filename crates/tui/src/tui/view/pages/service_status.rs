use codex_helper_core::service_status::{
    ServiceStatusKind, ServiceStatusProbeSample, ServiceStatusSnapshot,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};

use crate::tui::Language;
use crate::tui::i18n;
use crate::tui::model::{Palette, Snapshot, duration_short, format_age, now_ms, shorten_middle};
use crate::tui::state::UiState;
use crate::tui::view::widgets::kv_line;

pub(super) fn render_service_status_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    area: Rect,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
        .split(area);

    let status = snapshot.service_status.as_ref();
    render_status_table(f, p, ui.language, status, chunks[0]);
    render_details_panel(f, p, ui.language, status, chunks[1]);
}

fn render_status_table(
    f: &mut Frame<'_>,
    p: Palette,
    lang: Language,
    snapshot: Option<&ServiceStatusSnapshot>,
    area: Rect,
) {
    let l = |text| i18n::label(lang, text);
    let title = match snapshot {
        Some(snapshot) if snapshot.enabled && snapshot.configured => {
            let counts = snapshot.status_counts();
            format!(
                "{}  ok={} slow={} failed={} unknown={}",
                l("service status"),
                counts.ok,
                counts.slow,
                counts.failed,
                counts.unknown
            )
        }
        Some(snapshot) if !snapshot.enabled => {
            format!("{}  ({})", l("service status"), l("disabled"))
        }
        Some(_) => format!("{}  ({})", l("service status"), l("not configured")),
        None => l("service status").to_string(),
    };

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new([
        l("probe"),
        l("model"),
        l("status"),
        l("latency"),
        l("uptime"),
        l("history"),
    ])
    .style(Style::default().fg(p.muted))
    .height(1);

    let rows = snapshot
        .map(|snapshot| status_rows(p, lang, snapshot))
        .unwrap_or_else(|| {
            vec![Row::new(vec![
                Cell::from(l("service status")),
                Cell::from("-"),
                Cell::from(Span::styled(l("unknown"), Style::default().fg(p.muted))),
                Cell::from("-"),
                Cell::from("-"),
                Cell::from(match lang {
                    Language::Zh => "等待下一次快照",
                    Language::En => "waiting for snapshot",
                }),
            ])]
        });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(18),
            Constraint::Percentage(28),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Min(12),
        ],
    )
    .header(header)
    .block(block);
    f.render_widget(table, area);
}

fn status_rows(p: Palette, lang: Language, snapshot: &ServiceStatusSnapshot) -> Vec<Row<'static>> {
    let l = |text| i18n::label(lang, text);
    if !snapshot.enabled {
        return vec![Row::new(vec![
            Cell::from(l("service status")),
            Cell::from("-"),
            Cell::from(Span::styled(l("disabled"), Style::default().fg(p.muted))),
            Cell::from("-"),
            Cell::from("-"),
            Cell::from(match lang {
                Language::Zh => "在 [ui.service_status] 启用",
                Language::En => "enable under [ui.service_status]",
            }),
        ])];
    }
    if !snapshot.configured {
        return vec![Row::new(vec![
            Cell::from(l("service status")),
            Cell::from("-"),
            Cell::from(Span::styled(
                l("not configured"),
                Style::default().fg(p.muted),
            )),
            Cell::from("-"),
            Cell::from("-"),
            Cell::from(match lang {
                Language::Zh => "添加 service_status.probes",
                Language::En => "add service_status.probes",
            }),
        ])];
    }

    let mut rows = Vec::new();
    for probe in &snapshot.probes {
        if probe.services.is_empty() {
            rows.push(Row::new(vec![
                Cell::from(shorten_middle(&probe.id, 24)),
                Cell::from("-"),
                Cell::from(Span::styled(
                    l("unknown"),
                    service_status_style(p, ServiceStatusKind::Unknown),
                )),
                Cell::from("-"),
                Cell::from("-"),
                Cell::from(
                    probe
                        .error
                        .as_deref()
                        .map(|err| shorten_middle(err, 56))
                        .unwrap_or_else(|| "-".to_string()),
                ),
            ]));
            continue;
        }

        for service in &probe.services {
            rows.push(Row::new(vec![
                Cell::from(shorten_middle(&probe.id, 24)),
                Cell::from(shorten_middle(&service.model, 42)),
                Cell::from(Span::styled(
                    service_status_label(lang, service.latest_kind),
                    service_status_style(p, service.latest_kind),
                )),
                Cell::from(latency_label(service.latest.as_ref())),
                Cell::from(
                    service
                        .uptime_pct
                        .as_deref()
                        .map(|pct| format!("{pct}%"))
                        .unwrap_or_else(|| "-".to_string()),
                ),
                Cell::from(history_spans(p, &service.history)),
            ]));
        }
    }
    if rows.is_empty() {
        rows.push(Row::new(vec![
            Cell::from(l("service status")),
            Cell::from("-"),
            Cell::from(Span::styled(l("unknown"), Style::default().fg(p.muted))),
            Cell::from("-"),
            Cell::from("-"),
            Cell::from(l("no data")),
        ]));
    }
    rows
}

fn render_details_panel(
    f: &mut Frame<'_>,
    p: Palette,
    lang: Language,
    snapshot: Option<&ServiceStatusSnapshot>,
    area: Rect,
) {
    let l = |text| i18n::label(lang, text);
    let block = Block::default()
        .title(Span::styled(
            l("details"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    match snapshot {
        Some(snapshot) => push_snapshot_details(&mut lines, p, lang, snapshot),
        None => {
            lines.push(Line::from(Span::styled(
                match lang {
                    Language::Zh => "尚未从 dashboard snapshot 读取服务状态。",
                    Language::En => "No service status snapshot has been loaded yet.",
                },
                Style::default().fg(p.muted),
            )));
        }
    }

    let content = Paragraph::new(Text::from(lines))
        .block(block)
        .style(Style::default().fg(p.text))
        .wrap(Wrap { trim: false });
    f.render_widget(content, area);
}

fn push_snapshot_details(
    lines: &mut Vec<Line<'static>>,
    p: Palette,
    lang: Language,
    snapshot: &ServiceStatusSnapshot,
) {
    let l = |text| i18n::label(lang, text);
    let now = now_ms();
    let counts = snapshot.status_counts();
    lines.push(kv_line(
        p,
        l("status"),
        format!(
            "{}={}  {}={}  {}={}",
            l("enabled"),
            snapshot.enabled,
            l("configured"),
            snapshot.configured,
            l("probes"),
            snapshot.probes.len()
        ),
        Style::default().fg(p.text),
    ));
    lines.push(kv_line(
        p,
        l("summary"),
        format!(
            "ok={} slow={} failed={} unknown={}",
            counts.ok, counts.slow, counts.failed, counts.unknown
        ),
        Style::default().fg(if counts.failed > 0 {
            p.bad
        } else if counts.slow > 0 || counts.unknown > 0 {
            p.warn
        } else {
            p.good
        }),
    ));
    lines.push(kv_line(
        p,
        l("updated"),
        format_age(now, Some(snapshot.generated_at_ms)),
        Style::default().fg(p.muted),
    ));
    lines.push(kv_line(
        p,
        l("refresh"),
        format!(
            "{}s  cells={}",
            snapshot.refresh_interval_secs, snapshot.history_cells
        ),
        Style::default().fg(p.muted),
    ));
    if let Some(error) = snapshot.error.as_deref() {
        lines.push(kv_line(
            p,
            l("error"),
            shorten_middle(error, 96),
            Style::default().fg(p.warn),
        ));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        l("legend"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(Line::from(vec![
        Span::styled("O", service_status_style(p, ServiceStatusKind::Ok)),
        Span::styled(" ok  ", Style::default().fg(p.muted)),
        Span::styled("S", service_status_style(p, ServiceStatusKind::Slow)),
        Span::styled(" slow  ", Style::default().fg(p.muted)),
        Span::styled("X", service_status_style(p, ServiceStatusKind::Failed)),
        Span::styled(" failed  ", Style::default().fg(p.muted)),
        Span::styled("-", service_status_style(p, ServiceStatusKind::Unknown)),
        Span::styled(" unknown", Style::default().fg(p.muted)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        l("probes"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    if snapshot.probes.is_empty() {
        lines.push(Line::from(Span::styled(
            if snapshot.enabled {
                match lang {
                    Language::Zh => "没有可展示的探针。请配置 [ui.service_status].probes。",
                    Language::En => "No probes to show. Configure [ui.service_status].probes.",
                }
            } else {
                match lang {
                    Language::Zh => "服务状态探针已关闭。",
                    Language::En => "Service status probes are disabled.",
                }
            },
            Style::default().fg(p.muted),
        )));
        return;
    }

    for probe in &snapshot.probes {
        let status_style = if probe.error.is_some() {
            Style::default().fg(p.warn)
        } else if probe.all_ok == Some(false) {
            Style::default().fg(p.bad)
        } else {
            Style::default().fg(p.good)
        };
        let probe_label = shorten_middle(&probe.id, 30);
        lines.push(Line::from(vec![
            Span::styled(format!("{probe_label}: "), Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "services={} all_ok={} age={}",
                    probe.services.len(),
                    probe
                        .all_ok
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    format_age(now, Some(probe.fetched_at_ms))
                ),
                status_style,
            ),
        ]));
        if let Some(error) = probe.error.as_deref() {
            lines.push(Line::from(vec![Span::styled(
                format!("  {}: {}", l("error"), shorten_middle(error, 82)),
                Style::default().fg(p.warn),
            )]));
        }
        lines.push(Line::from(vec![Span::styled(
            format!("  {}", shorten_middle(&probe.url, 92)),
            Style::default().fg(p.muted),
        )]));
    }
}

fn history_spans(
    p: Palette,
    history: &[codex_helper_core::service_status::ServiceStatusCellSnapshot],
) -> Line<'static> {
    let max_cells = 48usize;
    let start = history.len().saturating_sub(max_cells);
    let spans = history[start..]
        .iter()
        .map(|cell| {
            Span::styled(
                service_status_symbol(cell.kind),
                service_status_style(p, cell.kind),
            )
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn service_status_symbol(kind: ServiceStatusKind) -> &'static str {
    match kind {
        ServiceStatusKind::Ok => "O",
        ServiceStatusKind::Slow => "S",
        ServiceStatusKind::Failed => "X",
        ServiceStatusKind::Unknown => "-",
    }
}

fn service_status_label(lang: Language, kind: ServiceStatusKind) -> String {
    let key = match kind {
        ServiceStatusKind::Ok => "ok",
        ServiceStatusKind::Slow => "slow",
        ServiceStatusKind::Failed => "failed",
        ServiceStatusKind::Unknown => "unknown",
    };
    i18n::label(lang, key).to_string()
}

fn service_status_style(p: Palette, kind: ServiceStatusKind) -> Style {
    match kind {
        ServiceStatusKind::Ok => Style::default().fg(p.good),
        ServiceStatusKind::Slow => Style::default().fg(p.warn),
        ServiceStatusKind::Failed => Style::default().fg(p.bad),
        ServiceStatusKind::Unknown => Style::default().fg(p.muted),
    }
}

fn latency_label(sample: Option<&ServiceStatusProbeSample>) -> String {
    sample
        .and_then(|sample| sample.latency_ms)
        .map(duration_short)
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_symbols_keep_recent_cells() {
        let p = Palette::default();
        let history = [
            ServiceStatusKind::Ok,
            ServiceStatusKind::Slow,
            ServiceStatusKind::Failed,
            ServiceStatusKind::Unknown,
        ]
        .into_iter()
        .map(
            |kind| codex_helper_core::service_status::ServiceStatusCellSnapshot {
                kind,
                probe: None,
            },
        )
        .collect::<Vec<_>>();

        let text = history_spans(p, &history)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(text, "OSX-");
    }
}
