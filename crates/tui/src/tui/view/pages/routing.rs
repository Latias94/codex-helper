use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, HighlightSpacing, Paragraph, Row, Table, Wrap};

use crate::dashboard_core::{
    OperatorProviderCapacity, OperatorProviderEndpointSummary, OperatorRouteCandidateSummary,
    OperatorRouteTargetSummary, OperatorRoutingSummary,
};
use crate::state::{BalanceSnapshotStatus, ProviderBalanceSnapshot, RuntimeConfigState};
use crate::tui::model::{
    Palette, Snapshot, balance_snapshot_rank, provider_balance_brief_lang,
    provider_balance_compact_lang, shorten_middle,
};
use crate::tui::operator_actions::PendingOperatorAction;
use crate::tui::state::UiState;
use crate::tui::{Language, ProviderOption};

const MASTER_DETAIL_MIN_WIDTH: u16 = 118;
const MASTER_DETAIL_MIN_HEIGHT: u16 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoutingTableLayout {
    Wide,
    Regular,
    Compact,
    Tiny,
}

impl RoutingTableLayout {
    fn for_width(width: u16) -> Self {
        match width {
            132.. => Self::Wide,
            100..=131 => Self::Regular,
            72..=99 => Self::Compact,
            _ => Self::Tiny,
        }
    }

    fn for_master_width(width: u16) -> Self {
        if width >= 92 {
            Self::Regular
        } else {
            Self::Compact
        }
    }
}

fn use_master_detail_layout(area: Rect) -> bool {
    area.width >= MASTER_DETAIL_MIN_WIDTH && area.height >= MASTER_DETAIL_MIN_HEIGHT
}

fn route_strategy_label(strategy: crate::config::RouteStrategy) -> &'static str {
    match strategy {
        crate::config::RouteStrategy::ManualSticky => "manual-sticky",
        crate::config::RouteStrategy::OrderedFailover => "ordered-failover",
        crate::config::RouteStrategy::RoundRobin => "round-robin",
        crate::config::RouteStrategy::TagPreferred => "tag-preferred",
        crate::config::RouteStrategy::Conditional => "conditional",
    }
}

fn affinity_label(policy: crate::config::RouteAffinityPolicy) -> &'static str {
    match policy {
        crate::config::RouteAffinityPolicy::Off => "off",
        crate::config::RouteAffinityPolicy::PreferredGroup => "preferred-group",
        crate::config::RouteAffinityPolicy::FallbackSticky => "fallback-sticky",
        crate::config::RouteAffinityPolicy::Hard => "hard",
    }
}

fn route_target_label(target: Option<&OperatorRouteTargetSummary>, lang: Language) -> String {
    target
        .map(|target| format!("{}.{}", target.provider_id, target.endpoint_id))
        .unwrap_or_else(|| match lang {
            Language::Zh => "自动分配".to_string(),
            Language::En => "automatic allocation".to_string(),
        })
}

fn configured_entry_target_label(routing: &OperatorRoutingSummary, lang: Language) -> String {
    routing.entry_target.clone().unwrap_or_else(|| match lang {
        Language::Zh => "自动路由".to_string(),
        Language::En => "automatic routing".to_string(),
    })
}

fn routing_summary_line(routing: &OperatorRoutingSummary, lang: Language) -> String {
    let configured_target = configured_entry_target_label(routing, lang);
    let new_session_preference = route_target_label(routing.new_session_preference.as_ref(), lang);
    match lang {
        Language::Zh => format!(
            "配置目标={configured_target}  新会话首选={new_session_preference}  策略={}  粘性={}  调度={}",
            route_strategy_label(routing.entry_strategy),
            affinity_label(routing.affinity_policy),
            routing.scheduling_preset.as_str(),
        ),
        Language::En => format!(
            "config target={configured_target}  new-session preference={new_session_preference}  policy={}  affinity={}  scheduling={}",
            route_strategy_label(routing.entry_strategy),
            affinity_label(routing.affinity_policy),
            routing.scheduling_preset.as_str(),
        ),
    }
}

fn balance_refresh_status(ui: &UiState) -> String {
    if ui.runtime_connection.is_remote_observer() {
        return match ui.language {
            Language::Zh => "远程只读；余额由 daemon 周期刷新".to_string(),
            Language::En => "remote read-only; balances refresh on the daemon".to_string(),
        };
    }
    if !ui.can_refresh_provider_balances() {
        return match ui.language {
            Language::Zh => "daemon 不支持本机刷新；当前只读".to_string(),
            Language::En => "daemon does not support local refresh; read-only".to_string(),
        };
    }
    if ui.balance_refresh_in_flight
        || matches!(
            ui.pending_operator_action,
            Some(PendingOperatorAction::RefreshBalances { .. })
        )
    {
        return match ui.language {
            Language::Zh => "余额/额度刷新中".to_string(),
            Language::En => "balance/quota refresh in progress".to_string(),
        };
    }
    if let Some(error) = ui.last_balance_refresh_error.as_deref() {
        return match ui.language {
            Language::Zh => format!("余额/额度刷新失败：{error}"),
            Language::En => format!("balance/quota refresh failed: {error}"),
        };
    }
    ui.last_balance_refresh_message
        .clone()
        .unwrap_or_else(|| match ui.language {
            Language::Zh => "进入本页自动刷新；g 强制全量刷新".to_string(),
            Language::En => "auto-refreshes on entry; g forces a full refresh".to_string(),
        })
}

fn routing_summary_lines(
    routing: &OperatorRoutingSummary,
    ui: &UiState,
    width: u16,
) -> Vec<Line<'static>> {
    let max_width = usize::from(width.saturating_sub(2)).max(1);
    vec![
        Line::from(shorten_middle(
            &routing_summary_line(routing, ui.language),
            max_width,
        )),
        Line::from(shorten_middle(&balance_refresh_status(ui), max_width)),
    ]
}

fn endpoint_for_candidate<'a>(
    providers: &'a [ProviderOption],
    candidate: &OperatorRouteCandidateSummary,
) -> Option<&'a OperatorProviderEndpointSummary> {
    providers
        .iter()
        .find(|provider| provider.name == candidate.provider_id)?
        .endpoints
        .iter()
        .find(|endpoint| endpoint.name == candidate.endpoint_id)
}

fn balance_for_candidate<'a>(
    snapshot: &'a Snapshot,
    candidate: &OperatorRouteCandidateSummary,
) -> Option<&'a ProviderBalanceSnapshot> {
    snapshot
        .provider_balances
        .get(candidate.provider_id.as_str())?
        .iter()
        .filter(|balance| balance.provider_endpoint.endpoint_id == candidate.endpoint_id)
        .min_by(|left, right| {
            balance_snapshot_rank(left)
                .cmp(&balance_snapshot_rank(right))
                .then_with(|| right.fetched_at_ms.cmp(&left.fetched_at_ms))
        })
}

fn capacity_label(capacity: Option<&OperatorProviderCapacity>) -> String {
    let Some(capacity) = capacity else {
        return "-".to_string();
    };
    match (capacity.active, capacity.limit) {
        (Some(active), Some(limit)) => format!("{active}/{limit}"),
        (None, Some(limit)) => format!("-/{limit}"),
        _ => capacity
            .effective_max_concurrent_requests
            .map(|limit| format!("-/{limit}"))
            .unwrap_or_else(|| "-".to_string()),
    }
}

fn runtime_state_status_label(state: RuntimeConfigState, lang: Language) -> Option<&'static str> {
    match (state, lang) {
        (RuntimeConfigState::Draining, Language::Zh) => Some("排空"),
        (RuntimeConfigState::Draining, Language::En) => Some("draining"),
        (RuntimeConfigState::BreakerOpen, Language::Zh) => Some("熔断"),
        (RuntimeConfigState::BreakerOpen, Language::En) => Some("breaker"),
        (RuntimeConfigState::HalfOpen, Language::Zh) => Some("探测"),
        (RuntimeConfigState::HalfOpen, Language::En) => Some("probing"),
        (RuntimeConfigState::Normal, _) => None,
    }
}

fn endpoint_status_label(
    endpoint: Option<&OperatorProviderEndpointSummary>,
    lang: Language,
) -> &'static str {
    let Some(endpoint) = endpoint else {
        return match lang {
            Language::Zh => "未知",
            Language::En => "unknown",
        };
    };
    if !endpoint.effective_enabled {
        return match lang {
            Language::Zh => "停用",
            Language::En => "disabled",
        };
    }
    if let Some(label) = runtime_state_status_label(endpoint.runtime_state, lang) {
        return label;
    }
    if endpoint.capacity.saturated {
        return match lang {
            Language::Zh => "已满",
            Language::En => "full",
        };
    }
    if !endpoint.routable {
        return match lang {
            Language::Zh => "不可用",
            Language::En => "blocked",
        };
    }
    match lang {
        Language::Zh => "可用",
        Language::En => "ready",
    }
}

fn candidate_style(
    p: Palette,
    endpoint: Option<&OperatorProviderEndpointSummary>,
    balance: Option<&ProviderBalanceSnapshot>,
    new_session_preferred: bool,
) -> Style {
    let style = if endpoint.is_none_or(|endpoint| !endpoint.effective_enabled) {
        Style::default().fg(p.muted)
    } else if endpoint.is_some_and(|endpoint| endpoint.capacity.saturated || !endpoint.routable) {
        Style::default().fg(p.warn)
    } else if balance.is_some_and(|balance| {
        balance.status == BalanceSnapshotStatus::Exhausted && !balance.routing_ignored_exhaustion()
    }) {
        Style::default().fg(p.bad)
    } else {
        Style::default().fg(p.text)
    };
    if new_session_preferred {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn candidate_is_new_session_preference(
    routing: &OperatorRoutingSummary,
    candidate: &OperatorRouteCandidateSummary,
) -> bool {
    routing
        .new_session_preference
        .as_ref()
        .is_some_and(|target| {
            target.provider_id == candidate.provider_id
                && target.endpoint_id == candidate.endpoint_id
        })
}

fn route_order_label(
    routing: &OperatorRoutingSummary,
    candidate: &OperatorRouteCandidateSummary,
) -> String {
    let marker = if candidate_is_new_session_preference(routing, candidate) {
        "P"
    } else {
        " "
    };
    format!("{}{marker}", candidate.route_order.saturating_add(1))
}

fn candidate_target_label(candidate: &OperatorRouteCandidateSummary, width: usize) -> String {
    shorten_middle(
        &format!("{}.{}", candidate.provider_id, candidate.endpoint_id),
        width,
    )
}

fn candidate_balance_label(
    snapshot: &Snapshot,
    candidate: &OperatorRouteCandidateSummary,
    max_width: usize,
    lang: Language,
) -> String {
    balance_for_candidate(snapshot, candidate)
        .map(|balance| provider_balance_compact_lang(balance, max_width, lang))
        .unwrap_or_else(|| "-".to_string())
}

fn table_header_labels(layout: RoutingTableLayout, lang: Language) -> Vec<&'static str> {
    match (layout, lang) {
        (RoutingTableLayout::Wide, Language::Zh) => {
            vec![
                "顺序",
                "组",
                "提供商",
                "端点",
                "优先",
                "状态",
                "并发",
                "余额/额度",
            ]
        }
        (RoutingTableLayout::Wide, Language::En) => {
            vec![
                "Order",
                "Group",
                "Provider",
                "Endpoint",
                "Pri",
                "State",
                "Capacity",
                "Balance/Quota",
            ]
        }
        (RoutingTableLayout::Regular, Language::Zh) => {
            vec!["顺序", "组", "目标", "优先", "状态", "并发", "余额/额度"]
        }
        (RoutingTableLayout::Regular, Language::En) => {
            vec![
                "Order",
                "Group",
                "Target",
                "Pri",
                "State",
                "Capacity",
                "Balance/Quota",
            ]
        }
        (RoutingTableLayout::Compact, Language::Zh) => {
            vec!["顺序", "组", "目标", "优先", "状态", "并发", "余额"]
        }
        (RoutingTableLayout::Compact, Language::En) => {
            vec!["Order", "G", "Target", "Pri", "State", "Cap", "Balance"]
        }
        (RoutingTableLayout::Tiny, Language::Zh) => vec!["顺序", "目标", "状态", "余额"],
        (RoutingTableLayout::Tiny, Language::En) => vec!["#", "Target", "State", "Balance"],
    }
}

fn table_header(layout: RoutingTableLayout, lang: Language) -> Row<'static> {
    Row::new(table_header_labels(layout, lang)).style(Style::default().add_modifier(Modifier::BOLD))
}

fn table_constraints(layout: RoutingTableLayout) -> Vec<Constraint> {
    match layout {
        RoutingTableLayout::Wide => vec![
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Min(14),
            Constraint::Length(15),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Length(28),
        ],
        RoutingTableLayout::Regular => vec![
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Min(22),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(24),
        ],
        RoutingTableLayout::Compact => vec![
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Min(16),
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Min(10),
        ],
        RoutingTableLayout::Tiny => vec![
            Constraint::Length(4),
            Constraint::Min(18),
            Constraint::Length(9),
            Constraint::Min(12),
        ],
    }
}

fn candidate_cells(
    routing: &OperatorRoutingSummary,
    candidate: &OperatorRouteCandidateSummary,
    snapshot: &Snapshot,
    endpoint: Option<&OperatorProviderEndpointSummary>,
    layout: RoutingTableLayout,
    lang: Language,
) -> Vec<String> {
    let order = route_order_label(routing, candidate);
    let group = candidate.preference_group.saturating_add(1).to_string();
    let priority = endpoint
        .map(|endpoint| endpoint.priority.to_string())
        .unwrap_or_else(|| "-".to_string());
    let status = endpoint_status_label(endpoint, lang).to_string();
    let capacity = capacity_label(endpoint.map(|endpoint| &endpoint.capacity));
    match layout {
        RoutingTableLayout::Wide => vec![
            order,
            group,
            shorten_middle(&candidate.provider_id, 24),
            shorten_middle(&candidate.endpoint_id, 13),
            priority,
            status,
            capacity,
            candidate_balance_label(snapshot, candidate, 27, lang),
        ],
        RoutingTableLayout::Regular => vec![
            order,
            group,
            candidate_target_label(candidate, 21),
            priority,
            status,
            capacity,
            candidate_balance_label(snapshot, candidate, 23, lang),
        ],
        RoutingTableLayout::Compact => vec![
            order,
            group,
            candidate_target_label(candidate, 15),
            priority,
            status,
            capacity,
            candidate_balance_label(snapshot, candidate, 9, lang),
        ],
        RoutingTableLayout::Tiny => vec![
            format!("{order}\nG{group}"),
            format!("{}\nPri {priority}", candidate_target_label(candidate, 17)),
            format!("{status}\n{capacity}"),
            candidate_balance_label(snapshot, candidate, 11, lang),
        ],
    }
}

fn candidate_row(
    routing: &OperatorRoutingSummary,
    candidate: &OperatorRouteCandidateSummary,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    layout: RoutingTableLayout,
    p: Palette,
    lang: Language,
) -> Row<'static> {
    let endpoint = endpoint_for_candidate(providers, candidate);
    let balance = balance_for_candidate(snapshot, candidate);
    let new_session_preferred = candidate_is_new_session_preference(routing, candidate);
    Row::new(candidate_cells(
        routing, candidate, snapshot, endpoint, layout, lang,
    ))
    .height(if layout == RoutingTableLayout::Tiny {
        2
    } else {
        1
    })
    .style(candidate_style(p, endpoint, balance, new_session_preferred))
}

fn selected_candidate_number(ui: &UiState, routing: &OperatorRoutingSummary) -> usize {
    if routing.candidates.is_empty() {
        0
    } else {
        ui.selected_routing_candidate_idx
            .min(routing.candidates.len().saturating_sub(1))
            .saturating_add(1)
    }
}

fn candidate_table_title(ui: &UiState, routing: &OperatorRoutingSummary) -> String {
    let selected = selected_candidate_number(ui, routing);
    match ui.language {
        Language::Zh => format!(
            " 候选端点 {selected}/{}  (P 新会话首选) ",
            routing.candidates.len()
        ),
        Language::En => format!(
            " Endpoint candidates {selected}/{}  (P new-session preference) ",
            routing.candidates.len()
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_candidate_table(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    routing: &OperatorRoutingSummary,
    area: Rect,
    layout: RoutingTableLayout,
) {
    let block = Block::default()
        .title(Span::styled(
            candidate_table_title(ui, routing),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let row_height = if layout == RoutingTableLayout::Tiny {
        2
    } else {
        1
    };
    let visible_rows = usize::from(block.inner(area).height.saturating_sub(1)) / row_height;
    ui.sync_routing_candidates_table_viewport(snapshot, visible_rows);
    let rows = routing.candidates.iter().map(|candidate| {
        candidate_row(
            routing,
            candidate,
            snapshot,
            providers,
            layout,
            p,
            ui.language,
        )
    });
    let table = Table::new(rows, table_constraints(layout))
        .header(table_header(layout, ui.language).style(Style::default().fg(p.muted)))
        .block(block)
        .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
        .highlight_symbol("> ")
        .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, area, &mut ui.routing_candidates_table);
}

fn detail_heading(text: &'static str, p: Palette) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    ))
}

fn push_routing_action_lines(lines: &mut Vec<Line<'static>>, p: Palette, ui: &UiState) {
    lines.push(detail_heading(
        match ui.language {
            Language::Zh => "路由控制",
            Language::En => "Routing controls",
        },
        p,
    ));
    if ui.can_mutate_routing() {
        lines.extend(match ui.language {
            Language::Zh => [
                Line::from("Enter/m      首选与端点状态菜单"),
                Line::from("a/Backspace  新会话恢复自动分配"),
            ],
            Language::En => [
                Line::from("Enter/m      preference / endpoint state menu"),
                Line::from("a/Backspace  restore automatic allocation"),
            ],
        });
    } else {
        lines.push(Line::from(Span::styled(
            match (ui.language, ui.runtime_connection.is_remote_observer()) {
                (Language::Zh, true) => "远程观察模式：路由只读",
                (Language::En, true) => "Remote observer: routing is read-only",
                (Language::Zh, false) => "当前 daemon 状态不允许修改路由",
                (Language::En, false) => "The current daemon state blocks routing changes",
            },
            Style::default().fg(p.muted),
        )));
    }
    if ui.can_refresh_provider_balances() {
        lines.push(Line::from(match ui.language {
            Language::Zh => "g            强制全量刷新余额/额度",
            Language::En => "g            force-refresh all balances/quotas",
        }));
    }
    lines.push(Line::from(match ui.language {
        Language::Zh => "p 定位新会话首选    i 查看完整详情",
        Language::En => "p locate new-session preference    i full details",
    }));
}

fn push_routing_policy_lines(
    lines: &mut Vec<Line<'static>>,
    p: Palette,
    ui: &UiState,
    routing: &OperatorRoutingSummary,
    max_width: usize,
) {
    lines.push(detail_heading(
        match ui.language {
            Language::Zh => "路由策略",
            Language::En => "Routing policy",
        },
        p,
    ));
    let configured_target = configured_entry_target_label(routing, ui.language);
    lines.push(Line::from(Span::styled(
        shorten_middle(
            &match ui.language {
                Language::Zh => format!("配置目标={configured_target}"),
                Language::En => format!("config target={configured_target}"),
            },
            max_width,
        ),
        Style::default().fg(p.text),
    )));
    let new_session_preference =
        route_target_label(routing.new_session_preference.as_ref(), ui.language);
    lines.push(Line::from(Span::styled(
        shorten_middle(
            &match ui.language {
                Language::Zh => format!("新会话首选={new_session_preference}"),
                Language::En => format!("new-session preference={new_session_preference}"),
            },
            max_width,
        ),
        Style::default().fg(if routing.new_session_preference.is_some() {
            p.accent
        } else {
            p.good
        }),
    )));
    lines.push(Line::from(match ui.language {
        Language::Zh => format!(
            "策略={}  粘性={}",
            route_strategy_label(routing.entry_strategy),
            affinity_label(routing.affinity_policy)
        ),
        Language::En => format!(
            "policy={}  affinity={}",
            route_strategy_label(routing.entry_strategy),
            affinity_label(routing.affinity_policy)
        ),
    }));
    lines.push(Line::from(match ui.language {
        Language::Zh => format!("调度={}", routing.scheduling_preset.as_str()),
        Language::En => format!("scheduling={}", routing.scheduling_preset.as_str()),
    }));
    lines.push(Line::from(Span::styled(
        shorten_middle(&balance_refresh_status(ui), max_width),
        Style::default().fg(if ui.last_balance_refresh_error.is_some() {
            p.warn
        } else {
            p.muted
        }),
    )));
}

fn selected_candidate_detail_lines(
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    routing: &OperatorRoutingSummary,
    width: u16,
) -> Vec<Line<'static>> {
    let max_width = usize::from(width.saturating_sub(4)).max(1);
    let mut lines = Vec::new();
    push_routing_policy_lines(&mut lines, p, ui, routing, max_width);
    lines.push(Line::from(""));
    push_routing_action_lines(&mut lines, p, ui);
    lines.push(Line::from(""));
    lines.push(detail_heading(
        match ui.language {
            Language::Zh => "选中端点",
            Language::En => "Selected endpoint",
        },
        p,
    ));
    let Some(candidate) = routing.candidates.get(ui.selected_routing_candidate_idx) else {
        lines.push(Line::from(Span::styled(
            match ui.language {
                Language::Zh => "没有候选端点",
                Language::En => "No endpoint candidates",
            },
            Style::default().fg(p.muted),
        )));
        return lines;
    };
    let endpoint = endpoint_for_candidate(providers, candidate);
    let balance = balance_for_candidate(snapshot, candidate);
    let target = format!("{}.{}", candidate.provider_id, candidate.endpoint_id);
    let new_session_preferred = candidate_is_new_session_preference(routing, candidate);
    lines.push(Line::from(Span::styled(
        shorten_middle(
            &match ui.language {
                Language::Zh => format!("目标={target}"),
                Language::En => format!("target={target}"),
            },
            max_width,
        ),
        Style::default()
            .fg(if new_session_preferred {
                p.accent
            } else {
                p.text
            })
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(match ui.language {
        Language::Zh => format!(
            "顺序={}  组={}  优先={}",
            candidate.route_order.saturating_add(1),
            candidate.preference_group.saturating_add(1),
            endpoint
                .map(|endpoint| endpoint.priority.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
        Language::En => format!(
            "order={}  group={}  priority={}",
            candidate.route_order.saturating_add(1),
            candidate.preference_group.saturating_add(1),
            endpoint
                .map(|endpoint| endpoint.priority.to_string())
                .unwrap_or_else(|| "-".to_string())
        ),
    }));
    lines.push(Line::from(match ui.language {
        Language::Zh => format!(
            "状态={}  并发={}",
            endpoint_status_label(endpoint, ui.language),
            capacity_label(endpoint.map(|endpoint| &endpoint.capacity))
        ),
        Language::En => format!(
            "state={}  capacity={}",
            endpoint_status_label(endpoint, ui.language),
            capacity_label(endpoint.map(|endpoint| &endpoint.capacity))
        ),
    }));
    lines.push(Line::from(Span::styled(
        shorten_middle(
            &match ui.language {
                Language::Zh => format!(
                    "余额={}",
                    balance
                        .map(|balance| {
                            provider_balance_compact_lang(balance, max_width, ui.language)
                        })
                        .unwrap_or_else(|| "-".to_string())
                ),
                Language::En => format!(
                    "balance={}",
                    balance
                        .map(|balance| {
                            provider_balance_compact_lang(balance, max_width, ui.language)
                        })
                        .unwrap_or_else(|| "-".to_string())
                ),
            },
            max_width,
        ),
        candidate_style(p, endpoint, balance, new_session_preferred),
    )));
    let origin = endpoint
        .and_then(|endpoint| endpoint.origin.as_deref())
        .unwrap_or("-");
    lines.push(Line::from(Span::styled(
        shorten_middle(
            &match ui.language {
                Language::Zh => format!("来源={origin}"),
                Language::En => format!("origin={origin}"),
            },
            max_width,
        ),
        Style::default().fg(p.muted),
    )));
    let mut policy_actions = endpoint
        .map(|endpoint| {
            endpoint
                .policy_actions
                .iter()
                .take(3)
                .map(|action| {
                    let cooldown = match (action.cooldown_remaining_secs, ui.language) {
                        (Some(seconds), Language::Zh) => format!("  冷却={seconds}s"),
                        (Some(seconds), Language::En) => format!("  cooldown={seconds}s"),
                        (None, _) => String::new(),
                    };
                    format!("{}{cooldown}", action.code)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(endpoint) = endpoint {
        let remaining = endpoint
            .policy_actions
            .len()
            .saturating_sub(policy_actions.len());
        if remaining > 0 {
            policy_actions.push(format!("+{remaining}"));
        }
    }
    let has_active_cooldown = endpoint.is_some_and(|endpoint| {
        endpoint
            .policy_actions
            .iter()
            .any(|action| action.active_cooldown)
    });
    let policy_action_summary = if policy_actions.is_empty() {
        "-".to_string()
    } else {
        policy_actions.join(", ")
    };
    lines.push(Line::from(Span::styled(
        shorten_middle(
            &match ui.language {
                Language::Zh => format!("策略动作={policy_action_summary}"),
                Language::En => format!("policy action={policy_action_summary}"),
            },
            max_width,
        ),
        Style::default().fg(if has_active_cooldown { p.warn } else { p.muted }),
    )));
    let route_path = if candidate.route_path.is_empty() {
        "-".to_string()
    } else {
        candidate.route_path.join(" -> ")
    };
    lines.push(Line::from(Span::styled(
        shorten_middle(
            &match ui.language {
                Language::Zh => format!("路径={route_path}"),
                Language::En => format!("route={route_path}"),
            },
            max_width,
        ),
        Style::default().fg(p.muted),
    )));
    lines
}

fn render_routing_detail(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    routing: &OperatorRoutingSummary,
    area: Rect,
) {
    let selected_target = routing
        .candidates
        .get(ui.selected_routing_candidate_idx)
        .map(|candidate| format!("{}.{}", candidate.provider_id, candidate.endpoint_id));
    let title = match (ui.language, selected_target) {
        (Language::Zh, Some(target)) => format!(" 路由策略 / {target} "),
        (Language::En, Some(target)) => format!(" Routing policy / {target} "),
        (Language::Zh, None) => " 路由策略 / 端点详情 ".to_string(),
        (Language::En, None) => " Routing policy / endpoint details ".to_string(),
    };
    let block = Block::default()
        .title(Span::styled(
            shorten_middle(&title, usize::from(area.width.saturating_sub(2))),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let lines = selected_candidate_detail_lines(
        p,
        ui,
        snapshot,
        providers,
        routing,
        block.inner(area).width,
    );
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .style(Style::default().fg(p.text))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_legacy_provider_table(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    let lang = ui.language;
    let header = match lang {
        Language::Zh => Row::new(["提供商", "端点", "状态", "并发", "余额/额度"]),
        Language::En => Row::new([
            "Provider",
            "Endpoints",
            "State",
            "Capacity",
            "Balance/Quota",
        ]),
    }
    .style(Style::default().fg(p.muted).add_modifier(Modifier::BOLD));
    let rows = providers.iter().map(|provider| {
        let state = if provider.effective_enabled {
            match lang {
                Language::Zh => "可用",
                Language::En => "ready",
            }
        } else {
            match lang {
                Language::Zh => "停用",
                Language::En => "disabled",
            }
        };
        Row::new([
            provider.name.clone(),
            format!(
                "{}/{}",
                provider.routable_endpoints,
                provider.endpoints.len()
            ),
            state.to_string(),
            capacity_label(Some(&provider.capacity)),
            provider_balance_brief_lang(
                &snapshot.provider_balances,
                provider.name.as_str(),
                28,
                lang,
            ),
        ])
        .style(Style::default().fg(if provider.effective_enabled {
            p.text
        } else {
            p.muted
        }))
    });
    let block = Block::default()
        .title(match lang {
            Language::Zh => " 提供商（旧版只读数据） ",
            Language::En => " Providers (legacy read-only data) ",
        })
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border));
    let visible_rows = usize::from(block.inner(area).height.saturating_sub(1));
    ui.sync_providers_table_viewport(providers.len(), visible_rows);
    let table = Table::new(
        rows,
        [
            Constraint::Min(18),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
    .highlight_symbol("> ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, area, &mut ui.providers_table);
}

pub(super) fn render_routing_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    let Some(routing) = snapshot.routing.as_ref() else {
        render_legacy_provider_table(f, p, ui, snapshot, providers, area);
        return;
    };
    if use_master_detail_layout(area) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        render_candidate_table(
            f,
            p,
            ui,
            snapshot,
            providers,
            routing,
            columns[0],
            RoutingTableLayout::for_master_width(columns[0].width),
        );
        render_routing_detail(f, p, ui, snapshot, providers, routing, columns[1]);
        return;
    }

    let layout = RoutingTableLayout::for_width(area.width);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(3)])
        .split(area);
    let summary = Paragraph::new(routing_summary_lines(routing, ui, sections[0].width))
        .block(
            Block::default()
                .title(match ui.language {
                    Language::Zh => " 路由策略 ",
                    Language::En => " Routing policy ",
                })
                .borders(Borders::ALL)
                .border_style(Style::default().fg(p.border)),
        )
        .style(Style::default().fg(p.text));
    f.render_widget(summary, sections[0]);
    render_candidate_table(f, p, ui, snapshot, providers, routing, sections[1], layout);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(provider_id: &str, endpoint_id: &str) -> OperatorRouteCandidateSummary {
        OperatorRouteCandidateSummary {
            route_order: 0,
            provider_id: provider_id.to_string(),
            endpoint_id: endpoint_id.to_string(),
            preference_group: 0,
            route_path: vec!["main".to_string()],
        }
    }

    fn routing_summary(
        entry_target: Option<&str>,
        new_session_preference: Option<(&str, &str)>,
    ) -> OperatorRoutingSummary {
        OperatorRoutingSummary {
            route_graph_key: "route:v1:test".to_string(),
            control_revision: 1,
            provider_policy_revision: 1,
            entry: "main".to_string(),
            entry_strategy: crate::config::RouteStrategy::RoundRobin,
            entry_target: entry_target.map(str::to_string),
            new_session_preference: new_session_preference.map(|(provider_id, endpoint_id)| {
                OperatorRouteTargetSummary {
                    provider_id: provider_id.to_string(),
                    endpoint_id: endpoint_id.to_string(),
                }
            }),
            affinity_policy: crate::config::RouteAffinityPolicy::FallbackSticky,
            scheduling_preset: crate::config::SchedulingPreset::Balanced,
            fallback_ttl_ms: None,
            reprobe_preferred_after_ms: None,
            candidates: Vec::new(),
        }
    }

    #[test]
    fn routing_table_layout_has_stable_width_boundaries() {
        for width in [160, 132] {
            assert_eq!(
                RoutingTableLayout::for_width(width),
                RoutingTableLayout::Wide
            );
        }
        for width in [131, 118, 100] {
            assert_eq!(
                RoutingTableLayout::for_width(width),
                RoutingTableLayout::Regular
            );
        }
        for width in [99, 76, 72] {
            assert_eq!(
                RoutingTableLayout::for_width(width),
                RoutingTableLayout::Compact
            );
        }
        for width in [71, 60] {
            assert_eq!(
                RoutingTableLayout::for_width(width),
                RoutingTableLayout::Tiny
            );
        }
    }

    #[test]
    fn runtime_state_labels_preserve_operational_meaning() {
        assert_eq!(
            runtime_state_status_label(RuntimeConfigState::Draining, Language::Zh),
            Some("排空")
        );
        assert_eq!(
            runtime_state_status_label(RuntimeConfigState::BreakerOpen, Language::En),
            Some("breaker")
        );
        assert_eq!(
            runtime_state_status_label(RuntimeConfigState::HalfOpen, Language::Zh),
            Some("探测")
        );
        assert_eq!(
            runtime_state_status_label(RuntimeConfigState::Normal, Language::En),
            None
        );
    }

    #[test]
    fn new_session_preference_matches_full_endpoint_identity() {
        let routing = routing_summary(None, Some(("input", "default")));

        assert!(candidate_is_new_session_preference(
            &routing,
            &candidate("input", "default")
        ));
        assert!(!candidate_is_new_session_preference(
            &routing,
            &candidate("input", "backup")
        ));
        assert!(!candidate_is_new_session_preference(
            &routing,
            &candidate("ciii", "default")
        ));
    }

    #[test]
    fn route_order_marks_only_new_session_preference() {
        let selected = candidate("input", "default");
        let preferred = routing_summary(None, Some(("input", "default")));
        let configured_only = routing_summary(Some("input.default"), None);

        assert_eq!(route_order_label(&preferred, &selected), "1P");
        assert_eq!(route_order_label(&configured_only, &selected), "1 ");
    }

    #[test]
    fn table_headers_rows_and_constraints_have_matching_column_counts() {
        let routing = routing_summary(None, None);
        let candidate = candidate("input", "default");
        let snapshot = Snapshot::default();

        for layout in [
            RoutingTableLayout::Wide,
            RoutingTableLayout::Regular,
            RoutingTableLayout::Compact,
            RoutingTableLayout::Tiny,
        ] {
            let constraint_count = table_constraints(layout).len();
            assert_eq!(
                table_header_labels(layout, Language::Zh).len(),
                constraint_count
            );
            assert_eq!(
                table_header_labels(layout, Language::En).len(),
                constraint_count
            );
            assert_eq!(
                candidate_cells(&routing, &candidate, &snapshot, None, layout, Language::Zh,).len(),
                constraint_count
            );
        }

        assert_eq!(
            table_header_labels(RoutingTableLayout::Compact, Language::Zh),
            vec!["顺序", "组", "目标", "优先", "状态", "并发", "余额"]
        );
        assert_eq!(
            table_header_labels(RoutingTableLayout::Compact, Language::En),
            vec!["Order", "G", "Target", "Pri", "State", "Cap", "Balance"]
        );
    }

    #[test]
    fn newer_balance_breaks_equal_rank_ties() {
        let mut snapshot = Snapshot::default();
        let candidate = OperatorRouteCandidateSummary {
            route_order: 0,
            provider_id: "input".to_string(),
            endpoint_id: "default".to_string(),
            preference_group: 0,
            route_path: vec!["main".to_string()],
        };
        snapshot.provider_balances.insert(
            "input".to_string(),
            vec![
                ProviderBalanceSnapshot {
                    provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                        "codex", "input", "default",
                    ),
                    fetched_at_ms: 1,
                    status: BalanceSnapshotStatus::Ok,
                    ..ProviderBalanceSnapshot::default()
                },
                ProviderBalanceSnapshot {
                    provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                        "codex", "input", "default",
                    ),
                    fetched_at_ms: 2,
                    status: BalanceSnapshotStatus::Ok,
                    ..ProviderBalanceSnapshot::default()
                },
            ],
        );

        assert_eq!(
            balance_for_candidate(&snapshot, &candidate).map(|balance| balance.fetched_at_ms),
            Some(2)
        );
    }

    #[test]
    fn balance_rank_precedes_recency() {
        let mut snapshot = Snapshot::default();
        let candidate = OperatorRouteCandidateSummary {
            route_order: 0,
            provider_id: "input".to_string(),
            endpoint_id: "default".to_string(),
            preference_group: 0,
            route_path: Vec::new(),
        };
        snapshot.provider_balances.insert(
            "input".to_string(),
            vec![
                ProviderBalanceSnapshot {
                    provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                        "codex", "input", "default",
                    ),
                    fetched_at_ms: 1,
                    status: BalanceSnapshotStatus::Ok,
                    ..ProviderBalanceSnapshot::default()
                },
                ProviderBalanceSnapshot {
                    provider_endpoint: crate::runtime_identity::ProviderEndpointKey::new(
                        "codex", "input", "default",
                    ),
                    fetched_at_ms: 2,
                    status: BalanceSnapshotStatus::Error,
                    ..ProviderBalanceSnapshot::default()
                },
            ],
        );

        assert_eq!(
            balance_for_candidate(&snapshot, &candidate).map(|balance| balance.status),
            Some(BalanceSnapshotStatus::Ok)
        );
    }
}
