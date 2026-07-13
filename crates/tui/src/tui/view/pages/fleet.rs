use std::collections::BTreeMap;

use codex_helper_core::fleet::{
    FleetConfidence, FleetEvidenceSource, FleetGraphStatus, FleetNodeHealth, FleetNodeKind,
    FleetNodeSnapshot, FleetSnapshot, FleetWorkUnit, FleetWorkUnitKind, FleetWorkUnitState,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::tui::Language;
use crate::tui::i18n;
use crate::tui::model::{
    Palette, format_age, now_ms, short_sid, shorten, shorten_middle, tokens_short,
};
use crate::tui::state::{FleetViewMode, UiState};
use crate::tui::view::widgets::kv_line;

pub(super) fn render_fleet_page(f: &mut Frame<'_>, p: Palette, ui: &mut UiState, area: Rect) {
    ui.sync_fleet_selection();
    let lang = ui.language;
    let snapshot = ui.fleet_snapshot.clone();
    let l = |text| i18n::label(lang, text);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(39),
            Constraint::Percentage(28),
        ])
        .split(area);

    render_nodes_table(f, p, ui, snapshot.as_ref(), chunks[0]);
    render_units_table(f, p, ui, snapshot.as_ref(), chunks[1]);
    render_details_panel(f, p, ui, snapshot.as_ref(), chunks[2]);

    if snapshot.is_none() {
        let empty = Block::default()
            .title(Span::styled(
                l("fleet"),
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.border))
            .style(Style::default().bg(p.panel));
        let message = if ui.fleet_loading {
            i18n::label(lang, "fleet: refreshing")
        } else {
            i18n::label(lang, "no fleet snapshot")
        };
        f.render_widget(
            Paragraph::new(message)
                .block(empty)
                .style(Style::default().fg(p.muted)),
            area,
        );
    }
}

fn render_nodes_table(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: Option<&FleetSnapshot>,
    area: Rect,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let title = match snapshot {
        Some(snapshot) => format!(
            "{}  ({} {}, {} {})",
            l("nodes"),
            snapshot.nodes.len(),
            l("nodes"),
            snapshot.active_work_units(),
            l("current work")
        ),
        None => l("nodes").to_string(),
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new([l("node"), l("health"), l("work units"), l("age")])
        .style(Style::default().fg(p.muted))
        .height(1);

    let now = now_ms();
    let rows = snapshot
        .map(|snapshot| {
            snapshot
                .nodes
                .iter()
                .map(|node| {
                    let work = node.current_work_units().count();
                    Row::new(vec![
                        Cell::from(node_label(node)),
                        Cell::from(Span::styled(
                            fleet_node_health_label(lang, node.health),
                            fleet_node_health_style(p, node.health),
                        )),
                        Cell::from(Span::styled(
                            format!("{work}/{}", node.work_units.len()),
                            Style::default().fg(if work > 0 { p.good } else { p.muted }),
                        )),
                        Cell::from(Span::styled(
                            format_age(now, Some(node.refreshed_at_ms)),
                            Style::default().fg(p.muted),
                        )),
                    ])
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(38),
            Constraint::Length(13),
            Constraint::Length(10),
            Constraint::Min(6),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, area, &mut ui.fleet_nodes_table);
}

fn render_units_table(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: Option<&FleetSnapshot>,
    area: Rect,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let selected_node =
        snapshot.and_then(|snapshot| snapshot.nodes.get(ui.selected_fleet_node_idx));
    let units = selected_node
        .map(|node| node.work_units.as_slice())
        .unwrap_or(&[]);
    let title = format!(
        "{}  ({}: {})",
        l("work units"),
        l("fleet view"),
        match ui.fleet_view_mode {
            FleetViewMode::Tree => l("tree"),
            FleetViewMode::Flat => l("flat"),
        }
    );
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new([
        l("work unit"),
        l("state"),
        l("source/confidence"),
        l("last activity"),
    ])
    .style(Style::default().fg(p.muted))
    .height(1);

    let rows = match ui.fleet_view_mode {
        FleetViewMode::Tree => tree_unit_rows(p, lang, units),
        FleetViewMode::Flat => flat_unit_rows(p, lang, units),
    };

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(42),
            Constraint::Length(16),
            Constraint::Length(18),
            Constraint::Min(8),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, area, &mut ui.fleet_units_table);
}

fn render_details_panel(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: Option<&FleetSnapshot>,
    area: Rect,
) {
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);
    let block = Block::default()
        .title(Span::styled(
            i18n::label(lang, "details"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let mut lines = Vec::new();
    if ui.fleet_loading {
        lines.push(Line::from(Span::styled(
            i18n::label(lang, "fleet: refreshing"),
            Style::default().fg(p.accent),
        )));
        lines.push(Line::from(""));
    }
    if let Some(err) = ui.fleet_last_error.as_deref() {
        lines.push(kv_line(
            p,
            l("last error"),
            err.to_string(),
            Style::default().fg(p.bad),
        ));
        lines.push(Line::from(""));
    }

    match snapshot {
        Some(snapshot) if snapshot.nodes.is_empty() => {
            lines.push(Line::from(Span::styled(
                l("fleet empty"),
                Style::default().fg(p.muted),
            )));
        }
        Some(snapshot) => {
            push_snapshot_details(&mut lines, p, lang, snapshot, ui);
        }
        None => {
            lines.push(Line::from(Span::styled(
                l("no fleet snapshot"),
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
    snapshot: &FleetSnapshot,
    ui: &UiState,
) {
    let l = |text| i18n::label(lang, text);
    let now = now_ms();
    lines.push(kv_line(
        p,
        l("fleet"),
        format!(
            "service={} nodes={} active={} age={}",
            snapshot.service_name,
            snapshot.nodes.len(),
            snapshot.active_work_units(),
            format_age(now, Some(snapshot.refreshed_at_ms))
        ),
        Style::default().fg(p.text),
    ));

    if let Some(loaded_at) = ui.fleet_last_loaded_at_ms {
        lines.push(kv_line(
            p,
            l("loaded"),
            format_age(now, Some(loaded_at)),
            Style::default().fg(p.muted),
        ));
    }
    lines.push(Line::from(""));

    let Some(node) = snapshot.nodes.get(ui.selected_fleet_node_idx) else {
        return;
    };
    lines.push(Line::from(vec![Span::styled(
        l("node"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.push(kv_line(
        p,
        "id",
        node.node_id.clone(),
        Style::default().fg(p.text),
    ));
    lines.push(kv_line(
        p,
        l("name"),
        node.label.clone(),
        Style::default().fg(p.text),
    ));
    lines.push(kv_line(
        p,
        l("health"),
        fleet_node_health_label(lang, node.health),
        fleet_node_health_style(p, node.health),
    ));
    lines.push(kv_line(
        p,
        l("endpoint"),
        node.active_endpoint
            .as_deref()
            .map(|value| shorten_middle(value, 80))
            .unwrap_or_else(|| "-".to_string()),
        Style::default().fg(p.muted),
    ));
    lines.push(kv_line(
        p,
        l("processes"),
        format!(
            "scan={} codex_like={}{}",
            node.processes.scan_available,
            node.processes.codex_like_processes,
            node.processes
                .error
                .as_deref()
                .map(|err| format!(" error={}", shorten_middle(err, 48)))
                .unwrap_or_default()
        ),
        Style::default().fg(if node.processes.error.is_some() {
            p.warn
        } else {
            p.muted
        }),
    ));
    lines.push(kv_line(
        p,
        l("topology"),
        topology_summary(node),
        Style::default().fg(p.muted),
    ));
    if let Some(err) = node.last_error.as_deref() {
        lines.push(kv_line(
            p,
            l("last error"),
            shorten_middle(err, 96),
            Style::default().fg(p.warn),
        ));
    }
    lines.push(Line::from(""));

    let Some(unit) = node.work_units.get(ui.selected_fleet_unit_idx) else {
        lines.push(Line::from(Span::styled(
            l("no selection"),
            Style::default().fg(p.muted),
        )));
        return;
    };
    lines.push(Line::from(vec![Span::styled(
        l("work unit"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    push_unit_details(lines, p, lang, unit);
}

fn push_unit_details(
    lines: &mut Vec<Line<'static>>,
    p: Palette,
    lang: Language,
    unit: &FleetWorkUnit,
) {
    let l = |text| i18n::label(lang, text);
    let now = now_ms();
    lines.push(kv_line(
        p,
        "id",
        shorten_middle(&unit.id, 72),
        Style::default().fg(p.text),
    ));
    lines.push(kv_line(
        p,
        l("state"),
        fleet_work_state_label(lang, unit.state),
        fleet_work_state_style(p, unit.state),
    ));
    lines.push(kv_line(
        p,
        l("source/confidence"),
        format!(
            "{}/{}",
            evidence_source_label(unit.evidence.source),
            confidence_label(unit.evidence.confidence)
        ),
        Style::default().fg(p.muted),
    ));
    lines.push(kv_line(
        p,
        l("session"),
        unit.session_id
            .as_deref()
            .map(|sid| short_sid(sid, 28))
            .unwrap_or_else(|| "-".to_string()),
        Style::default().fg(p.text),
    ));
    if let Some(task) = unit.task_name.as_deref() {
        lines.push(kv_line(
            p,
            "task",
            shorten_middle(task, 72),
            Style::default().fg(p.text),
        ));
    }
    lines.push(kv_line(
        p,
        l("cwd"),
        unit.cwd
            .as_deref()
            .map(|cwd| shorten_middle(cwd, 80))
            .unwrap_or_else(|| "-".to_string()),
        Style::default().fg(p.text),
    ));
    lines.push(kv_line(
        p,
        l("model"),
        unit.model
            .as_deref()
            .map(|model| shorten_middle(model, 48))
            .unwrap_or_else(|| "-".to_string()),
        Style::default().fg(p.text),
    ));
    lines.push(kv_line(
        p,
        l("provider"),
        unit.provider_id
            .as_deref()
            .map(|value| shorten_middle(value, 48))
            .unwrap_or_else(|| "-".to_string()),
        Style::default().fg(p.text),
    ));
    lines.push(kv_line(
        p,
        l("last activity"),
        format_age(now, unit.last_activity_ms),
        Style::default().fg(p.muted),
    ));
    if let Some(status) = unit.last_status {
        lines.push(kv_line(
            p,
            l("status"),
            status.to_string(),
            Style::default().fg(if status >= 500 {
                p.bad
            } else if status >= 400 {
                p.warn
            } else {
                p.good
            }),
        ));
    }
    if let Some(error) = unit.last_error.as_deref() {
        lines.push(kv_line(
            p,
            l("last error"),
            shorten_middle(error, 80),
            Style::default().fg(p.warn),
        ));
    }
    let total_tokens = unit
        .usage
        .total_usage
        .as_ref()
        .map(|usage| usage.total_tokens)
        .filter(|tokens| *tokens > 0);
    lines.push(kv_line(
        p,
        l("usage"),
        format!(
            "turns={}/{} tok={} out_tok/s={}",
            unit.usage.turns_total.unwrap_or(0),
            unit.usage.turns_with_usage.unwrap_or(0),
            total_tokens
                .map(tokens_short)
                .unwrap_or_else(|| "-".to_string()),
            unit.usage
                .avg_output_tokens_per_second
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| "-".to_string())
        ),
        Style::default().fg(p.muted),
    ));
    if let Some(detail) = unit.evidence.detail.as_deref() {
        lines.push(kv_line(
            p,
            l("detail"),
            shorten_middle(detail, 80),
            Style::default().fg(p.muted),
        ));
    }
}

fn flat_unit_rows(p: Palette, lang: Language, units: &[FleetWorkUnit]) -> Vec<Row<'static>> {
    let now = now_ms();
    units
        .iter()
        .map(|unit| unit_row(p, lang, unit, 0, now))
        .collect()
}

fn tree_unit_rows(p: Palette, lang: Language, units: &[FleetWorkUnit]) -> Vec<Row<'static>> {
    let mut children = BTreeMap::<Option<&str>, Vec<&FleetWorkUnit>>::new();
    for unit in units {
        children
            .entry(unit.parent_id.as_deref())
            .or_default()
            .push(unit);
    }
    for group in children.values_mut() {
        group.sort_by_key(|unit| std::cmp::Reverse(unit.last_activity_ms.unwrap_or(0)));
    }

    let mut rows = Vec::new();
    let now = now_ms();
    if let Some(roots) = children.get(&None) {
        for unit in roots {
            push_tree_unit_rows(&mut rows, p, lang, unit, &children, 0, now);
        }
    }
    for unit in units {
        if unit.parent_id.is_some()
            && !units
                .iter()
                .any(|candidate| unit.parent_id.as_deref() == Some(candidate.id.as_str()))
        {
            push_tree_unit_rows(&mut rows, p, lang, unit, &children, 0, now);
        }
    }
    rows
}

fn push_tree_unit_rows(
    rows: &mut Vec<Row<'static>>,
    p: Palette,
    lang: Language,
    unit: &FleetWorkUnit,
    children: &BTreeMap<Option<&str>, Vec<&FleetWorkUnit>>,
    depth: usize,
    now: u64,
) {
    rows.push(unit_row(p, lang, unit, depth, now));
    if let Some(kids) = children.get(&Some(unit.id.as_str())) {
        for child in kids {
            push_tree_unit_rows(rows, p, lang, child, children, depth.saturating_add(1), now);
        }
    }
}

fn unit_row(
    p: Palette,
    lang: Language,
    unit: &FleetWorkUnit,
    depth: usize,
    now: u64,
) -> Row<'static> {
    let title = unit_title(unit);
    let indent = if depth == 0 {
        String::new()
    } else {
        format!("{}- ", "  ".repeat(depth))
    };
    Row::new(vec![
        Cell::from(format!("{indent}{title}")),
        Cell::from(Span::styled(
            fleet_work_state_label(lang, unit.state),
            fleet_work_state_style(p, unit.state),
        )),
        Cell::from(Span::styled(
            format!(
                "{}/{}",
                evidence_source_label(unit.evidence.source),
                confidence_label(unit.evidence.confidence)
            ),
            Style::default().fg(p.muted),
        )),
        Cell::from(Span::styled(
            format_age(now, unit.last_activity_ms),
            Style::default().fg(p.muted),
        )),
    ])
}

fn node_label(node: &FleetNodeSnapshot) -> String {
    let kind = match node.kind {
        FleetNodeKind::Local => "local",
        FleetNodeKind::Remote => "remote",
    };
    format!("{} ({kind})", shorten(&node.label, 28))
}

fn unit_title(unit: &FleetWorkUnit) -> String {
    let label = unit
        .task_name
        .as_deref()
        .or(unit.session_id.as_deref())
        .or(unit.local_thread_id.as_deref())
        .unwrap_or(unit.id.as_str());
    let kind = match unit.kind {
        FleetWorkUnitKind::Root => "root",
        FleetWorkUnitKind::Subagent => "subagent",
        FleetWorkUnitKind::Process => "process",
    };
    format!("{} [{kind}]", shorten_middle(label, 44))
}

fn topology_summary(node: &FleetNodeSnapshot) -> String {
    let status = match node.topology.status {
        FleetGraphStatus::Available => "available",
        FleetGraphStatus::Unavailable => "unavailable",
        FleetGraphStatus::Partial => "partial",
    };
    let mut summary = format!("{status} edges={}", node.topology.edges.len());
    if let Some(note) = node.topology.note.as_deref() {
        summary.push(' ');
        summary.push_str(&shorten_middle(note, 48));
    }
    summary
}

fn fleet_node_health_label(lang: Language, health: FleetNodeHealth) -> String {
    let key = match health {
        FleetNodeHealth::Fresh => "fresh",
        FleetNodeHealth::Stale => "stale",
        FleetNodeHealth::AuthFailed => "auth_failed",
        FleetNodeHealth::RateLimited => "rate_limited",
        FleetNodeHealth::Unsupported => "unsupported",
        FleetNodeHealth::Unreachable => "unreachable",
        FleetNodeHealth::ParseFailed => "parse_failed",
    };
    i18n::label(lang, key).to_string()
}

fn fleet_node_health_style(p: Palette, health: FleetNodeHealth) -> Style {
    match health {
        FleetNodeHealth::Fresh => Style::default().fg(p.good),
        FleetNodeHealth::Stale | FleetNodeHealth::Unsupported => Style::default().fg(p.warn),
        FleetNodeHealth::AuthFailed
        | FleetNodeHealth::RateLimited
        | FleetNodeHealth::Unreachable
        | FleetNodeHealth::ParseFailed => Style::default().fg(p.bad),
    }
}

fn fleet_work_state_label(lang: Language, state: FleetWorkUnitState) -> String {
    let key = match state {
        FleetWorkUnitState::Unknown => "unknown",
        FleetWorkUnitState::Running => "running",
        FleetWorkUnitState::WaitingInput => "waiting_input",
        FleetWorkUnitState::WaitingApproval => "waiting_approval",
        FleetWorkUnitState::Idle => "idle",
        FleetWorkUnitState::Interrupted => "interrupted",
        FleetWorkUnitState::Completed => "completed",
        FleetWorkUnitState::Errored => "errored",
        FleetWorkUnitState::Exited => "exited",
    };
    i18n::label(lang, key).to_string()
}

fn fleet_work_state_style(p: Palette, state: FleetWorkUnitState) -> Style {
    match state {
        FleetWorkUnitState::Running => Style::default().fg(p.good),
        FleetWorkUnitState::WaitingInput | FleetWorkUnitState::WaitingApproval => {
            Style::default().fg(p.accent)
        }
        FleetWorkUnitState::Errored | FleetWorkUnitState::Interrupted => Style::default().fg(p.bad),
        FleetWorkUnitState::Idle
        | FleetWorkUnitState::Completed
        | FleetWorkUnitState::Exited
        | FleetWorkUnitState::Unknown => Style::default().fg(p.muted),
    }
}

fn evidence_source_label(source: FleetEvidenceSource) -> &'static str {
    match source {
        FleetEvidenceSource::RuntimeStatus => "runtime",
        FleetEvidenceSource::SessionLog => "session_log",
        FleetEvidenceSource::ProcessScan => "process",
        FleetEvidenceSource::CachedSnapshot => "cache",
        FleetEvidenceSource::Unavailable => "unknown",
    }
}

fn confidence_label(confidence: FleetConfidence) -> &'static str {
    match confidence {
        FleetConfidence::High => "high",
        FleetConfidence::Medium => "medium",
        FleetConfidence::Low => "low",
        FleetConfidence::Unknown => "unknown",
    }
}
