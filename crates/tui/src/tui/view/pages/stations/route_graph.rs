use std::collections::{BTreeMap, BTreeSet, HashMap};

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::tui::Language;
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_status_label_lang, balance_snapshot_status_style,
    provider_balance_compact_lang, routing_context_balance_rank, shorten, shorten_middle,
};
use crate::tui::state::UiState;

pub(super) const ROUTING_BALANCE_COLUMN_WIDTH: u16 = 24;
pub(super) const ROUTING_BALANCE_SUMMARY_WIDTH: usize = 32;

pub(super) fn routing_policy_label(policy: crate::config::RoutingPolicyV4) -> &'static str {
    match policy {
        crate::config::RoutingPolicyV4::ManualSticky => "manual-sticky",
        crate::config::RoutingPolicyV4::OrderedFailover => "ordered-failover",
        crate::config::RoutingPolicyV4::TagPreferred => "tag-preferred",
        crate::config::RoutingPolicyV4::Conditional => "conditional",
    }
}

pub(super) fn routing_exhausted_label(
    action: crate::config::RoutingExhaustedActionV4,
) -> &'static str {
    match action {
        crate::config::RoutingExhaustedActionV4::Continue => "continue",
        crate::config::RoutingExhaustedActionV4::Stop => "stop",
    }
}

pub(super) fn routing_tags_label(tags: &BTreeMap<String, String>, max_width: usize) -> String {
    if tags.is_empty() {
        return "-".to_string();
    }
    shorten_middle(
        &tags
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" "),
        max_width,
    )
}

pub(super) fn routing_prefer_tags_label(
    filters: &[BTreeMap<String, String>],
    max_width: usize,
) -> String {
    if filters.is_empty() {
        return "-".to_string();
    }
    shorten_middle(
        &filters
            .iter()
            .map(|tags| routing_tags_label(tags, max_width))
            .collect::<Vec<_>>()
            .join(" OR "),
        max_width,
    )
}

pub(super) fn route_graph_tree_text_lines(
    spec: &crate::tui::model::RoutingSpecView,
    lang: Language,
) -> Vec<String> {
    let providers = spec
        .providers
        .iter()
        .map(|provider| (provider.name.as_str(), provider))
        .collect::<BTreeMap<_, _>>();
    let mut visited_routes = BTreeSet::new();
    let mut stack = BTreeSet::new();
    let mut lines = Vec::new();

    push_route_graph_ref_text_lines(
        spec,
        &providers,
        spec.entry.as_str(),
        0,
        &mut visited_routes,
        &mut stack,
        &mut lines,
        lang,
    );

    for route_name in spec.routes.keys() {
        if visited_routes.contains(route_name) {
            continue;
        }
        lines.push(format!("unreachable route {route_name}:"));
        push_route_graph_ref_text_lines(
            spec,
            &providers,
            route_name,
            1,
            &mut visited_routes,
            &mut stack,
            &mut lines,
            lang,
        );
    }

    lines
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_route_graph_ref_text_lines(
    spec: &crate::tui::model::RoutingSpecView,
    providers: &BTreeMap<&str, &crate::tui::model::RoutingProviderRef>,
    name: &str,
    depth: usize,
    visited_routes: &mut BTreeSet<String>,
    stack: &mut BTreeSet<String>,
    lines: &mut Vec<String>,
    lang: Language,
) {
    let indent = "  ".repeat(depth);
    if let Some(provider) = providers.get(name) {
        let state = if provider.enabled {
            i18n::label(lang, "on")
        } else {
            i18n::label(lang, "off")
        };
        let tags = routing_tags_label(&provider.tags, 72);
        lines.push(format!("{indent}- provider {name} [{state}, tags={tags}]"));
        return;
    }

    let Some(node) = spec.routes.get(name) else {
        lines.push(format!("{indent}- missing ref {name}"));
        return;
    };

    if !stack.insert(name.to_string()) {
        lines.push(format!("{indent}- route {name} [cycle]"));
        return;
    }
    visited_routes.insert(name.to_string());

    let route_kind = if name == spec.entry {
        "entry route"
    } else {
        "route"
    };
    lines.push(format!(
        "{indent}- {route_kind} {name} [{}]",
        route_graph_node_brief(node, lang)
    ));

    match node.strategy {
        crate::config::RoutingPolicyV4::Conditional => {
            lines.push(format!(
                "{indent}  when: {}",
                route_graph_condition_label(node.when.as_ref())
            ));
            if let Some(target) = node.then.as_deref() {
                lines.push(format!("{indent}  then:"));
                push_route_graph_ref_text_lines(
                    spec,
                    providers,
                    target,
                    depth + 2,
                    visited_routes,
                    stack,
                    lines,
                    lang,
                );
            } else {
                lines.push(format!("{indent}  then: <missing>"));
            }
            if let Some(target) = node.default_route.as_deref() {
                lines.push(format!("{indent}  default:"));
                push_route_graph_ref_text_lines(
                    spec,
                    providers,
                    target,
                    depth + 2,
                    visited_routes,
                    stack,
                    lines,
                    lang,
                );
            } else {
                lines.push(format!("{indent}  default: <missing>"));
            }
        }
        crate::config::RoutingPolicyV4::ManualSticky => {
            if let Some(target) = node.target.as_deref() {
                lines.push(format!("{indent}  target:"));
                push_route_graph_ref_text_lines(
                    spec,
                    providers,
                    target,
                    depth + 2,
                    visited_routes,
                    stack,
                    lines,
                    lang,
                );
            }
            push_route_graph_children_text_lines(
                spec,
                providers,
                &node.children,
                depth,
                visited_routes,
                stack,
                lines,
                lang,
            );
        }
        crate::config::RoutingPolicyV4::OrderedFailover
        | crate::config::RoutingPolicyV4::TagPreferred => {
            push_route_graph_children_text_lines(
                spec,
                providers,
                &node.children,
                depth,
                visited_routes,
                stack,
                lines,
                lang,
            );
        }
    }

    stack.remove(name);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_route_graph_children_text_lines(
    spec: &crate::tui::model::RoutingSpecView,
    providers: &BTreeMap<&str, &crate::tui::model::RoutingProviderRef>,
    children: &[String],
    depth: usize,
    visited_routes: &mut BTreeSet<String>,
    stack: &mut BTreeSet<String>,
    lines: &mut Vec<String>,
    lang: Language,
) {
    if children.is_empty() {
        lines.push(format!("{}  (no children)", "  ".repeat(depth)));
        return;
    }
    for child in children {
        push_route_graph_ref_text_lines(
            spec,
            providers,
            child,
            depth + 1,
            visited_routes,
            stack,
            lines,
            lang,
        );
    }
}

pub(super) fn route_graph_node_brief(
    node: &crate::config::RoutingNodeV4,
    lang: Language,
) -> String {
    let mut parts = vec![routing_policy_label(node.strategy).to_string()];
    if let Some(target) = node.target.as_deref() {
        parts.push(format!("target={target}"));
    }
    if !node.prefer_tags.is_empty() {
        parts.push(format!(
            "prefer_tags={}",
            routing_prefer_tags_label(&node.prefer_tags, 64)
        ));
    }
    if node.on_exhausted != crate::config::RoutingExhaustedActionV4::Continue {
        parts.push(format!(
            "{}={}",
            i18n::label(lang, "on_exhausted"),
            routing_exhausted_label(node.on_exhausted)
        ));
    }
    parts.join(", ")
}

pub(super) fn route_graph_condition_label(
    condition: Option<&crate::config::RoutingConditionV4>,
) -> String {
    let Some(condition) = condition else {
        return "<always>".to_string();
    };
    if condition.is_empty() {
        return "<always>".to_string();
    }

    let mut parts = Vec::new();
    if let Some(value) = condition.model.as_deref() {
        parts.push(format!("model={value}"));
    }
    if let Some(value) = condition.service_tier.as_deref() {
        parts.push(format!("service_tier={value}"));
    }
    if let Some(value) = condition.reasoning_effort.as_deref() {
        parts.push(format!("reasoning_effort={value}"));
    }
    if let Some(value) = condition.method.as_deref() {
        parts.push(format!("method={value}"));
    }
    if let Some(value) = condition.path.as_deref() {
        parts.push(format!("path={value}"));
    }
    for (key, value) in &condition.headers {
        parts.push(format!("header:{key}={value}"));
    }
    shorten_middle(&parts.join(" "), 96)
}

pub(super) fn routing_provider_matches_preference(
    spec: &crate::tui::model::RoutingSpecView,
    provider: &crate::tui::state::RoutingProviderRow,
) -> bool {
    matches!(spec.policy, crate::config::RoutingPolicyV4::TagPreferred)
        && spec.prefer_tags.iter().any(|filter| {
            !filter.is_empty()
                && filter
                    .iter()
                    .all(|(key, value)| provider.tags.get(key) == Some(value))
        })
}

pub(super) fn routing_provider_balance_snapshots<'a>(
    snapshot: &'a Snapshot,
    provider_name: &str,
) -> Vec<&'a crate::state::ProviderBalanceSnapshot> {
    let mut matches = snapshot
        .provider_balances
        .iter()
        .flat_map(|(key, balances)| {
            balances.iter().filter_map(move |balance| {
                if balance.provider_id == provider_name
                    || (balance.provider_id.trim().is_empty() && key == provider_name)
                {
                    Some((
                        routing_context_balance_rank(key, balance, provider_name),
                        balance,
                    ))
                } else {
                    None
                }
            })
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.upstream_index.cmp(&right.1.upstream_index))
            .then_with(|| right.1.fetched_at_ms.cmp(&left.1.fetched_at_ms))
    });
    matches.into_iter().map(|(_, balance)| balance).collect()
}

pub(super) fn routing_provider_balance_brief_lang(
    snapshot: &Snapshot,
    provider_name: &str,
    max_width: usize,
    lang: Language,
) -> String {
    routing_provider_balance_snapshots(snapshot, provider_name)
        .first()
        .map(|balance| provider_balance_compact_lang(balance, max_width, lang))
        .unwrap_or_else(|| "-".to_string())
}

pub(super) fn routing_provider_balance_cell_lang(
    p: Palette,
    snapshot: &Snapshot,
    provider_name: &str,
    max_width: usize,
    lang: Language,
) -> (String, Style) {
    let balances = routing_provider_balance_snapshots(snapshot, provider_name);
    match balances.first() {
        Some(balance) => (
            provider_balance_compact_lang(balance, max_width, lang),
            balance_snapshot_status_style(p, balance),
        ),
        None => ("-".to_string(), Style::default().fg(p.muted)),
    }
}

pub(super) fn routing_provider_display_label(
    provider: Option<&crate::tui::state::RoutingProviderRow>,
    provider_name: &str,
) -> String {
    provider
        .map(crate::tui::state::RoutingProviderRow::display_label)
        .unwrap_or_else(|| provider_name.to_string())
}

pub(super) fn route_target_summary_line(
    snapshot: &Snapshot,
    target: Option<&str>,
    provider_by_name: &HashMap<&str, crate::tui::state::RoutingProviderRow>,
    max_width: usize,
    lang: Language,
) -> String {
    let Some(target) = target.filter(|target| !target.trim().is_empty()) else {
        return "-".to_string();
    };
    let provider = provider_by_name.get(target);
    let label = routing_provider_display_label(provider, target);
    let balance_width = max_width
        .saturating_div(2)
        .clamp(10, ROUTING_BALANCE_SUMMARY_WIDTH);
    let balance = routing_provider_balance_brief_lang(snapshot, target, balance_width, lang);
    let mut parts = vec![label.clone()];
    let has_balance = balance != "-";
    if has_balance {
        parts.push(balance.clone());
    }
    if provider.is_none() {
        parts.push(i18n::label(lang, "not in catalog").to_string());
    }
    let full = parts.join(" | ");
    if UnicodeWidthStr::width(full.as_str()) <= max_width {
        return full;
    }

    if has_balance {
        let suffix = format!(" | {balance}");
        let suffix_width = UnicodeWidthStr::width(suffix.as_str());
        if suffix_width < max_width {
            let label_width = max_width.saturating_sub(suffix_width);
            let compact = format!("{}{}", shorten_middle(&label, label_width), suffix);
            if UnicodeWidthStr::width(compact.as_str()) <= max_width {
                return compact;
            }
        }
    }

    shorten_middle(&label, max_width)
}

pub(super) fn push_wrapped_segments<'a>(
    lines: &mut Vec<Line<'a>>,
    p: Palette,
    label: &str,
    segments: &[String],
    separator: &str,
    max_width: usize,
) {
    lines.push(Line::from(vec![Span::styled(
        format!("{label}: "),
        Style::default().fg(p.muted),
    )]));
    let mut line = String::new();
    for segment in segments {
        if segment.is_empty() {
            continue;
        }
        let candidate = if line.is_empty() {
            segment.clone()
        } else {
            format!("{line}{separator}{segment}")
        };
        if line.is_empty() || unicode_width::UnicodeWidthStr::width(candidate.as_str()) <= max_width
        {
            line = candidate;
        } else {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(line, Style::default().fg(p.text)),
            ]));
            line = segment.clone();
        }
    }
    if line.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("-", Style::default().fg(p.muted)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(line, Style::default().fg(p.text)),
        ]));
    }
}

pub(super) fn push_wrapped_segment_body<'a>(
    lines: &mut Vec<Line<'a>>,
    p: Palette,
    segments: &[String],
    separator: &str,
    max_width: usize,
) {
    let mut line = String::new();
    for segment in segments {
        if segment.is_empty() {
            continue;
        }
        let candidate = if line.is_empty() {
            segment.clone()
        } else {
            format!("{line}{separator}{segment}")
        };
        if line.is_empty() || UnicodeWidthStr::width(candidate.as_str()) <= max_width {
            line = candidate;
        } else {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(line, Style::default().fg(p.text)),
            ]));
            line = segment.clone();
        }
    }
    if line.is_empty() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("-", Style::default().fg(p.muted)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(line, Style::default().fg(p.text)),
        ]));
    }
}

pub(super) fn folded_route_chain_segments(
    segments: &[String],
    selected: Option<&str>,
    max_items: usize,
) -> Vec<String> {
    if segments.len() <= max_items.max(1) {
        return segments.to_vec();
    }

    let mut keep = BTreeSet::new();
    keep.insert(0usize);
    if max_items >= 5 && segments.len() > 1 {
        keep.insert(1);
    }
    if segments.len() > 1 {
        keep.insert(segments.len() - 1);
    }
    if max_items >= 6 && segments.len() > 2 {
        keep.insert(segments.len() - 2);
    }
    if let Some(selected_idx) =
        selected.and_then(|selected| segments.iter().position(|segment| segment == selected))
    {
        keep.insert(selected_idx);
    }

    let mut out = Vec::new();
    let mut last_idx = None;
    for idx in keep {
        if let Some(prev) = last_idx {
            let gap = idx.saturating_sub(prev + 1);
            if gap > 0 {
                out.push(format!("... +{gap}"));
            }
        }
        let mut item = segments[idx].clone();
        if selected.is_some_and(|selected| selected == segments[idx]) {
            item = format!("*{item}");
        }
        out.push(item);
        last_idx = Some(idx);
    }
    out
}

pub(super) fn route_chain_summary(
    segments: &[String],
    selected: Option<&str>,
    lang: Language,
) -> String {
    let total = segments.len();
    let selected = selected
        .and_then(|selected| {
            segments
                .iter()
                .position(|segment| segment == selected)
                .map(|idx| (idx, selected))
        })
        .map(|(idx, selected)| match lang {
            Language::Zh => format!("选中 {selected} #{}/{}", idx + 1, total),
            Language::En => format!("selected {selected} #{}/{}", idx + 1, total),
        });
    match (lang, selected) {
        (Language::Zh, Some(selected)) => format!("{total} 个 provider · {selected}"),
        (Language::Zh, None) => format!("{total} 个 provider"),
        (Language::En, Some(selected)) => format!("{total} providers · {selected}"),
        (Language::En, None) => format!("{total} providers"),
    }
}

pub(super) fn push_route_chain<'a>(
    lines: &mut Vec<Line<'a>>,
    p: Palette,
    label: &str,
    segments: &[String],
    selected: Option<&str>,
    max_width: usize,
    lang: Language,
) {
    const SEPARATOR: &str = " > ";
    if segments.len() <= 6 {
        push_wrapped_segments(lines, p, label, segments, SEPARATOR, max_width);
        return;
    }

    lines.push(Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().fg(p.muted)),
        Span::styled(
            route_chain_summary(segments, selected, lang),
            Style::default().fg(p.muted),
        ),
    ]));
    let folded = folded_route_chain_segments(segments, selected, 6);
    push_wrapped_segment_body(lines, p, &folded, SEPARATOR, max_width);
}

pub(super) fn routing_provider_marker(
    spec: &crate::tui::model::RoutingSpecView,
    provider_name: &str,
    provider: Option<&crate::tui::state::RoutingProviderRow>,
) -> &'static str {
    if spec.target.as_deref() == Some(provider_name) {
        "PIN"
    } else if provider.is_some_and(|provider| routing_provider_matches_preference(spec, provider)) {
        "PREF"
    } else {
        ""
    }
}

pub(super) fn render_route_graph_routing_page(
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
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    let Some(spec) = ui.routing_spec.clone() else {
        let block = Block::default()
            .title(Span::styled(
                l("Routing"),
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.border))
            .style(Style::default().bg(p.panel));
        let text = Text::from(vec![
            Line::from(match lang {
                crate::tui::Language::Zh => "正在加载 routing providers...",
                crate::tui::Language::En => "loading routing providers...",
            }),
            Line::from(Span::styled(
                match lang {
                    crate::tui::Language::Zh => "按 r 可立即打开编辑器",
                    crate::tui::Language::En => "press r to open the editor immediately",
                },
                Style::default().fg(p.muted),
            )),
        ]);
        f.render_widget(
            Paragraph::new(text)
                .block(block)
                .style(Style::default().fg(p.text))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    };

    let rows = ui.routing_provider_rows().unwrap_or_default();
    let order = rows.iter().map(|row| row.name.clone()).collect::<Vec<_>>();
    let selected_session = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|row| row.session_id.as_deref())
        .unwrap_or("-");
    let session_route_target = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|row| row.override_route_target.as_deref());
    let global_route_target = snapshot.global_route_target_override.as_deref();
    let provider_by_name = rows
        .iter()
        .map(|row| (row.name.as_str(), row.clone()))
        .collect::<HashMap<_, _>>();

    let left_block = Block::default()
        .title(Span::styled(
            format!(
                "{} {}  {}={}  {}={}",
                l("Routing"),
                l("providers"),
                l("policy"),
                routing_policy_label(spec.policy),
                l("exhausted"),
                routing_exhausted_label(spec.on_exhausted)
            ),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let left_inner_width = left_block.inner(columns[0]).width;
    let compact_provider_table = left_inner_width < 52;
    let balance_column_width = if compact_provider_table {
        left_inner_width
            .saturating_sub(19)
            .clamp(10, ROUTING_BALANCE_COLUMN_WIDTH)
    } else {
        ROUTING_BALANCE_COLUMN_WIDTH
    };
    let header = if compact_provider_table {
        Row::new(vec![
            "#".to_string(),
            l("Provider").to_string(),
            l("On").to_string(),
            l("Balance/Quota").to_string(),
        ])
    } else {
        Row::new(vec![
            "#".to_string(),
            l("Provider").to_string(),
            l("On").to_string(),
            l("Route").to_string(),
            l("Balance/Quota").to_string(),
        ])
    }
    .style(Style::default().fg(p.muted))
    .height(1);
    let table_rows = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| {
            let name = row.name.as_str();
            let marker = routing_provider_marker(&spec, name, Some(row));
            let label = row.display_label();
            let (balance, balance_style) = routing_provider_balance_cell_lang(
                p,
                snapshot,
                name,
                usize::from(balance_column_width),
                lang,
            );
            let provider_style = if !row.enabled {
                Style::default().fg(p.muted)
            } else if session_route_target == Some(name) {
                Style::default().fg(p.focus).add_modifier(Modifier::BOLD)
            } else if global_route_target == Some(name) || spec.target.as_deref() == Some(name) {
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
            } else if routing_provider_matches_preference(&spec, row) {
                Style::default().fg(p.good)
            } else {
                Style::default().fg(p.text)
            };
            let on_style = Style::default().fg(if row.enabled { p.good } else { p.muted });
            let route_style = if row.enabled {
                provider_style
            } else {
                Style::default().fg(p.muted)
            };
            let cells = if compact_provider_table {
                vec![
                    Cell::from(Span::styled(
                        (idx + 1).to_string(),
                        Style::default().fg(p.muted),
                    )),
                    Cell::from(Span::styled(shorten_middle(&label, 28), provider_style)),
                    Cell::from(Span::styled(
                        if row.enabled { l("on") } else { l("off") }.to_string(),
                        on_style,
                    )),
                    Cell::from(Span::styled(balance, balance_style)),
                ]
            } else {
                vec![
                    Cell::from(Span::styled(
                        (idx + 1).to_string(),
                        Style::default().fg(p.muted),
                    )),
                    Cell::from(Span::styled(shorten_middle(&label, 28), provider_style)),
                    Cell::from(Span::styled(
                        if row.enabled { l("on") } else { l("off") }.to_string(),
                        on_style,
                    )),
                    Cell::from(Span::styled(marker.to_string(), route_style)),
                    Cell::from(Span::styled(balance, balance_style)),
                ]
            };
            Row::new(cells).height(1)
        })
        .collect::<Vec<_>>();

    let table_visible_rows = usize::from(left_block.inner(columns[0]).height.saturating_sub(1));
    ui.sync_route_graph_table_viewport(table_visible_rows);
    let table_constraints = if compact_provider_table {
        vec![
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(balance_column_width),
        ]
    } else {
        vec![
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(balance_column_width),
        ]
    };
    let table = Table::new(table_rows, table_constraints)
        .header(header)
        .block(left_block)
        .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("  ")
        .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, columns[0], &mut ui.stations_table);

    let selected_row = ui.selected_route_graph_provider_row();
    let selected_name = selected_row.as_ref().map(|row| row.name.as_str());
    let right_title = selected_name
        .map(|name| format!("{}: {name}", l("Provider routing")))
        .unwrap_or_else(|| l("Provider routing").to_string());
    let right_detail_width = usize::from(columns[1].width.saturating_sub(2)).clamp(24, 96);

    let mut lines = Vec::new();
    let active_route_target = session_route_target.or(global_route_target);
    let route_target_source = if session_route_target.is_some() {
        i18n::label(lang, "session")
    } else if global_route_target.is_some() {
        i18n::label(lang, "global")
    } else {
        "-"
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{}: ", l("session")), Style::default().fg(p.muted)),
        Span::styled(
            shorten_middle(selected_session, 24),
            Style::default().fg(p.text),
        ),
        Span::raw("   "),
        Span::styled(format!("{}: ", l("source")), Style::default().fg(p.muted)),
        Span::styled(
            route_target_source.to_string(),
            Style::default().fg(if session_route_target.is_some() {
                p.focus
            } else if global_route_target.is_some() {
                p.accent
            } else {
                p.muted
            }),
        ),
    ]));
    let active_route_target_summary = route_target_summary_line(
        snapshot,
        active_route_target,
        &provider_by_name,
        right_detail_width,
        lang,
    );
    let active_route_target_label = active_route_target
        .filter(|target| !target.trim().is_empty())
        .map(|target| routing_provider_display_label(provider_by_name.get(target), target))
        .unwrap_or_else(|| "-".to_string());
    let target_style = if session_route_target.is_some() {
        p.focus
    } else if global_route_target.is_some() {
        p.accent
    } else {
        p.muted
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{}: ", l("target")), Style::default().fg(p.muted)),
        Span::styled(
            shorten_middle(&active_route_target_label, right_detail_width),
            Style::default().fg(target_style),
        ),
    ]));
    if active_route_target_summary != active_route_target_label
        && active_route_target_summary != "-"
    {
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("balance")), Style::default().fg(p.muted)),
            Span::styled(
                shorten_middle(&active_route_target_summary, right_detail_width),
                Style::default().fg(target_style),
            ),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled(format!("{}: ", l("policy")), Style::default().fg(p.muted)),
        Span::styled(
            routing_policy_label(spec.policy),
            Style::default().fg(p.text),
        ),
        Span::raw("   "),
        Span::styled(format!("{}: ", l("pinned")), Style::default().fg(p.muted)),
        Span::styled(
            shorten_middle(
                spec.target.as_deref().unwrap_or("-"),
                right_detail_width / 3,
            ),
            Style::default().fg(if spec.target.is_some() {
                p.accent
            } else {
                p.muted
            }),
        ),
        Span::raw("   "),
        Span::styled(
            format!("{}: ", l("on_exhausted")),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            routing_exhausted_label(spec.on_exhausted),
            Style::default().fg(p.text),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}: ", l("prefer_tags")),
            Style::default().fg(p.muted),
        ),
        Span::styled(
            routing_prefer_tags_label(&spec.prefer_tags, right_detail_width),
            Style::default().fg(if spec.prefer_tags.is_empty() {
                p.muted
            } else {
                p.accent
            }),
        ),
    ]));
    push_route_chain(
        &mut lines,
        p,
        l("order"),
        &order,
        selected_name,
        right_detail_width,
        lang,
    );

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        l("route graph"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    let graph_lines = route_graph_tree_text_lines(&spec, lang);
    let graph_limit = 24usize;
    for line in graph_lines.iter().take(graph_limit) {
        let style = if line.contains("missing ref") || line.contains("[cycle]") {
            Style::default().fg(p.warn)
        } else if line.contains("- provider ") {
            Style::default().fg(p.text)
        } else {
            Style::default().fg(p.muted)
        };
        lines.push(Line::from(Span::styled(
            shorten_middle(line, right_detail_width),
            style,
        )));
    }
    if graph_lines.len() > graph_limit {
        lines.push(Line::from(Span::styled(
            format!("... +{} more", graph_lines.len() - graph_limit),
            Style::default().fg(p.muted),
        )));
    }

    if let Some(name) = selected_name {
        lines.push(Line::from(""));
        if let Some(provider) = selected_row.as_ref().filter(|row| row.in_catalog) {
            if let Some(alias) = provider
                .alias
                .as_deref()
                .filter(|alias| !alias.trim().is_empty())
            {
                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", l("alias")), Style::default().fg(p.muted)),
                    Span::styled(alias.to_string(), Style::default().fg(p.text)),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("enabled")), Style::default().fg(p.muted)),
                Span::styled(
                    if provider.enabled { l("yes") } else { l("no") },
                    Style::default().fg(if provider.enabled { p.good } else { p.warn }),
                ),
                Span::raw("   "),
                Span::styled(format!("{}: ", l("Route")), Style::default().fg(p.muted)),
                Span::styled(
                    routing_provider_marker(&spec, name, Some(provider)).to_string(),
                    Style::default().fg(p.accent),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("tags")), Style::default().fg(p.muted)),
                Span::styled(
                    routing_tags_label(&provider.tags, right_detail_width),
                    Style::default().fg(p.text),
                ),
            ]));
        } else {
            lines.push(Line::from(Span::styled(
                l("provider is referenced by the route graph but missing from catalog"),
                Style::default().fg(p.warn),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Balance / quota"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        let balances = routing_provider_balance_snapshots(snapshot, name);
        if balances.is_empty() {
            lines.push(Line::from(Span::styled(
                i18n::text(lang, msg::NONE_PARENS),
                Style::default().fg(p.muted),
            )));
        } else {
            for balance in balances.into_iter().take(10) {
                let idx = balance
                    .upstream_index
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_string());
                lines.push(Line::from(vec![
                    Span::styled(format!("{idx:>2}. "), Style::default().fg(p.muted)),
                    Span::styled(
                        balance_snapshot_status_label_lang(balance, lang),
                        balance_snapshot_status_style(p, balance),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        provider_balance_compact_lang(balance, right_detail_width, lang),
                        Style::default().fg(p.text),
                    ),
                ]));
                if let Some(err) = balance
                    .error
                    .as_deref()
                    .filter(|err| !err.trim().is_empty())
                {
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(
                            format!(
                                "{}: {}",
                                l("balance lookup failed"),
                                shorten(err, right_detail_width.saturating_sub(24))
                            ),
                            Style::default().fg(p.muted),
                        ),
                    ]));
                }
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        l("Actions"),
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]));
    lines.extend(match lang {
        crate::tui::Language::Zh => vec![
            Line::from("  Enter        将全局 route target 设置为选中 provider"),
            Line::from("  Backspace    清除全局 route target"),
            Line::from("  o/O          设置/清除当前会话 route target"),
            Line::from("  g            刷新余额并同步路由解释"),
            Line::from("  r            打开 routing 编辑器"),
            Line::from("  e            启用/禁用选中 provider"),
            Line::from("  f            优先 billing=monthly"),
            Line::from("  1/2/0        设置 monthly/paygo/清除 billing 标签"),
            Line::from("  s            切换 on_exhausted continue/stop"),
            Line::from("  [/]/u/d      调整 fallback 顺序"),
        ],
        crate::tui::Language::En => vec![
            Line::from("  Enter        set global route target to selected provider"),
            Line::from("  Backspace    clear global route target"),
            Line::from("  o/O          set/clear session route target"),
            Line::from("  g            refresh balances and routing explain"),
            Line::from("  r            open routing editor"),
            Line::from("  e            enable/disable selected provider"),
            Line::from("  f            prefer billing=monthly"),
            Line::from("  1/2/0        set monthly/paygo/clear billing tag"),
            Line::from("  s            toggle on_exhausted continue/stop"),
            Line::from("  [/]/u/d      reorder fallback order"),
        ],
    });

    let right_block = Block::default()
        .title(Span::styled(
            right_title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));
    let content = Paragraph::new(Text::from(lines))
        .block(right_block)
        .style(Style::default().fg(p.muted))
        .wrap(Wrap { trim: false });
    f.render_widget(content, columns[1]);
}
