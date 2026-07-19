use codex_helper_core::service_status::{
    ServiceStatusKind, ServiceStatusProbeSample, ServiceStatusProbeSnapshot,
    ServiceStatusServiceSnapshot, ServiceStatusSnapshot,
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
    let (table_area, details_area) = service_status_page_areas(area);

    let status = snapshot.service_status.as_ref();
    let operator_status = ui.operator_read_model.as_ref().map(|model| model.status);
    let operator_ready = operator_status == Some(crate::dashboard_core::OperatorReadStatus::Ready);
    render_status_table(f, p, ui.language, status, operator_ready, table_area);
    render_details_panel(f, p, ui.language, status, operator_status, details_area);
}

fn service_status_page_areas(area: Rect) -> (Rect, Rect) {
    let (direction, constraints) = if area.width < 140 {
        (
            Direction::Vertical,
            [Constraint::Percentage(68), Constraint::Percentage(32)],
        )
    } else {
        (
            Direction::Horizontal,
            [Constraint::Percentage(66), Constraint::Percentage(34)],
        )
    };
    let chunks = Layout::default()
        .direction(direction)
        .constraints(constraints)
        .split(area);
    (chunks[0], chunks[1])
}

fn service_status_table_constraints(width: u16) -> [Constraint; 6] {
    if width < 110 {
        [
            Constraint::Percentage(18),
            Constraint::Percentage(24),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Min(5),
        ]
    } else {
        [
            Constraint::Percentage(18),
            Constraint::Percentage(28),
            Constraint::Length(14),
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Min(12),
        ]
    }
}

fn render_status_table(
    f: &mut Frame<'_>,
    p: Palette,
    lang: Language,
    snapshot: Option<&ServiceStatusSnapshot>,
    operator_ready: bool,
    area: Rect,
) {
    let l = |text| i18n::label(lang, text);
    let title = match snapshot {
        Some(snapshot) if snapshot.enabled && snapshot.configured => {
            let counts = snapshot.status_counts();
            let blocked_credentials = snapshot
                .probes
                .iter()
                .filter_map(|probe| probe.credential_readiness)
                .filter(|readiness| !readiness.is_routable())
                .count();
            let stale_credentials = snapshot
                .probes
                .iter()
                .filter(|probe| {
                    probe.credential_readiness
                        == Some(crate::credentials::CredentialReadinessCode::Stale)
                })
                .count();
            format!(
                "{}  ok={} slow={} failed={} unknown={} cred_blocked={} cred_stale={}{}",
                l("service status"),
                counts.ok,
                counts.slow,
                counts.failed,
                counts.unknown,
                blocked_credentials,
                stale_credentials,
                if operator_ready {
                    ""
                } else {
                    "  snapshot_stale"
                }
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
        .map(|snapshot| status_rows(p, lang, snapshot, operator_ready))
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

    let table = Table::new(rows, service_status_table_constraints(area.width))
        .header(header)
        .block(block);
    f.render_widget(table, area);
}

fn status_rows(
    p: Palette,
    lang: Language,
    snapshot: &ServiceStatusSnapshot,
    operator_ready: bool,
) -> Vec<Row<'static>> {
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
            let (status_label, status_style) =
                effective_status_label(p, lang, snapshot, probe, service, operator_ready);
            rows.push(Row::new(vec![
                Cell::from(shorten_middle(&probe.id, 24)),
                Cell::from(shorten_middle(&service.model, 42)),
                Cell::from(Span::styled(status_label, status_style)),
                Cell::from(if operator_ready && probe.fetched_at_ms != 0 {
                    latency_label(service.latest.as_ref())
                } else {
                    "-".to_string()
                }),
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

fn effective_status_label(
    p: Palette,
    lang: Language,
    snapshot: &ServiceStatusSnapshot,
    probe: &ServiceStatusProbeSnapshot,
    service: &ServiceStatusServiceSnapshot,
    operator_ready: bool,
) -> (String, Style) {
    if !operator_ready {
        return (
            match lang {
                Language::Zh => "快照过期",
                Language::En => "snapshot stale",
            }
            .to_string(),
            Style::default().fg(p.warn),
        );
    }
    if let Some(readiness) = probe
        .credential_readiness
        .filter(|readiness| !readiness.is_routable())
    {
        return (
            super::routing::credential_readiness_short_label(readiness, lang).to_string(),
            Style::default().fg(p.warn),
        );
    }
    if probe.fetched_at_ms == 0 {
        return (
            service_status_label(lang, ServiceStatusKind::Unknown),
            service_status_style(p, ServiceStatusKind::Unknown),
        );
    }
    let fact_age_ms = now_ms().saturating_sub(probe.fetched_at_ms);
    if fact_age_ms > snapshot.refresh_interval_secs.max(1).saturating_mul(1_000) {
        return (
            match lang {
                Language::Zh => "探针过期",
                Language::En => "probe stale",
            }
            .to_string(),
            Style::default().fg(p.warn),
        );
    }
    if probe.credential_readiness == Some(crate::credentials::CredentialReadinessCode::Stale) {
        return (
            format!(
                "{}/stale",
                service_status_label(Language::En, service.latest_kind)
            ),
            Style::default().fg(p.warn),
        );
    }
    (
        service_status_label(lang, service.latest_kind),
        service_status_style(p, service.latest_kind),
    )
}

fn render_details_panel(
    f: &mut Frame<'_>,
    p: Palette,
    lang: Language,
    snapshot: Option<&ServiceStatusSnapshot>,
    operator_status: Option<crate::dashboard_core::OperatorReadStatus>,
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
        Some(snapshot) => push_snapshot_details(&mut lines, p, lang, snapshot, operator_status),
        None => {
            lines.push(Line::from(Span::styled(
                match lang {
                    Language::Zh => "尚未从 operator read model 读取服务状态。",
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
    operator_status: Option<crate::dashboard_core::OperatorReadStatus>,
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
        l("operator snapshot"),
        operator_status
            .map(|status| format!("{status:?}").to_ascii_lowercase())
            .unwrap_or_else(|| "unavailable".to_string()),
        Style::default().fg(
            if operator_status == Some(crate::dashboard_core::OperatorReadStatus::Ready) {
                p.good
            } else {
                p.warn
            },
        ),
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
        let credential = probe
            .credential_readiness
            .map(|readiness| super::routing::credential_readiness_short_label(readiness, lang))
            .unwrap_or("-");
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
                    "services={} all_ok={} credential={} age={}",
                    probe.services.len(),
                    probe
                        .all_ok
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    credential,
                    format_age(
                        now,
                        (probe.fetched_at_ms != 0).then_some(probe.fetched_at_ms)
                    )
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
        for detail in &probe.credential_details {
            let kind = detail.kind.map(|kind| kind.as_str()).unwrap_or("upstream");
            let source = detail.source_kind.as_deref().unwrap_or("unreported");
            let reference = detail.reference.as_deref().unwrap_or("-");
            let cause = detail
                .stale_cause
                .map(|cause| format!(" cause={}", cause.as_str()))
                .unwrap_or_default();
            lines.push(Line::from(Span::styled(
                format!(
                    "  {kind}: {} source={source} ref={reference}{cause}",
                    detail.code.as_str()
                ),
                Style::default().fg(if detail.code.is_routable() {
                    p.muted
                } else {
                    p.warn
                }),
            )));
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

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn status_fixture(
        readiness: crate::credentials::CredentialReadinessCode,
    ) -> (
        ServiceStatusSnapshot,
        ServiceStatusProbeSnapshot,
        ServiceStatusServiceSnapshot,
    ) {
        let service = ServiceStatusServiceSnapshot {
            model: "gpt-test".to_string(),
            uptime_pct: None,
            latest_kind: ServiceStatusKind::Ok,
            latest: Some(ServiceStatusProbeSample {
                ts_ms: Some(now_ms()),
                ok: Some(true),
                latency_ms: Some(10),
                error: None,
            }),
            history: Vec::new(),
        };
        let probe = ServiceStatusProbeSnapshot {
            id: "provider".to_string(),
            url: "https://provider.example".to_string(),
            fetched_at_ms: now_ms(),
            generated_at_ms: None,
            all_ok: Some(true),
            services: vec![service.clone()],
            credential_readiness: Some(readiness),
            credential_details: Vec::new(),
            error: None,
        };
        let snapshot = ServiceStatusSnapshot {
            generated_at_ms: now_ms(),
            configured: true,
            enabled: true,
            refresh_interval_secs: 60,
            history_cells: 60,
            probes: vec![probe.clone()],
            error: None,
        };
        (snapshot, probe, service)
    }

    fn buffer_text(buffer: &Buffer) -> String {
        let mut output = String::new();
        for y in buffer.area.y..buffer.area.y.saturating_add(buffer.area.height) {
            for x in buffer.area.x..buffer.area.x.saturating_add(buffer.area.width) {
                output.push_str(buffer[(x, y)].symbol());
            }
            output.push('\n');
        }
        output
    }

    fn render_service_status_text(width: u16, height: u16) -> String {
        let (service_status, _, _) =
            status_fixture(crate::credentials::CredentialReadinessCode::Ready);
        let snapshot = Snapshot {
            service_status: Some(service_status),
            ..Snapshot::default()
        };
        let ui = UiState {
            language: Language::En,
            ..UiState::default()
        };
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let frame = terminal
            .draw(|frame| {
                render_service_status_page(frame, Palette::default(), &ui, &snapshot, frame.area());
            })
            .expect("render service status page");
        buffer_text(frame.buffer)
    }

    #[test]
    fn service_status_layout_stacks_on_narrow_terminals() {
        for width in [80, 100] {
            let area = Rect::new(0, 0, width, 32);
            let (table, details) = service_status_page_areas(area);
            assert_eq!(table.width, width);
            assert_eq!(details.width, width);
            assert!(details.y > table.y);
        }

        let area = Rect::new(0, 0, 160, 32);
        let (table, details) = service_status_page_areas(area);
        assert_eq!(table.height, area.height);
        assert_eq!(details.height, area.height);
        assert!(details.x > table.x);
    }

    #[test]
    fn service_status_page_renders_at_supported_widths() {
        for width in [80, 100, 160] {
            let text = render_service_status_text(width, 32).to_ascii_lowercase();
            assert!(text.contains("service status"), "width={width}\n{text}");
            assert!(text.contains("details"), "width={width}\n{text}");
            assert!(text.contains("gpt-test"), "width={width}\n{text}");
        }
    }

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

    #[test]
    fn credential_blockage_takes_precedence_over_historical_ok() {
        let (snapshot, probe, service) =
            status_fixture(crate::credentials::CredentialReadinessCode::Missing);

        let (label, _) = effective_status_label(
            Palette::default(),
            Language::En,
            &snapshot,
            &probe,
            &service,
            true,
        );

        assert_eq!(label, "auth miss");
    }

    #[test]
    fn stale_operator_snapshot_takes_precedence_over_historical_ok() {
        let (snapshot, probe, service) =
            status_fixture(crate::credentials::CredentialReadinessCode::Ready);

        let (label, _) = effective_status_label(
            Palette::default(),
            Language::En,
            &snapshot,
            &probe,
            &service,
            false,
        );

        assert_eq!(label, "snapshot stale");
    }
}
