use std::collections::{BTreeMap, BTreeSet, HashMap};

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Cell, HighlightSpacing, Paragraph, Row, Table, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::dashboard_core::{
    StationRetryBoundary, StationRoutingCandidate, StationRoutingMode, StationRoutingPosture,
    StationRoutingPostureInput, StationRoutingSkipReason, StationRoutingSource,
    build_station_routing_posture, summarize_recent_retry_observations,
};
use crate::tui::i18n::{self, msg};
use crate::tui::model::{
    Palette, Snapshot, balance_amount_brief_lang, balance_snapshot_status_label_lang,
    balance_snapshot_status_style, format_age, now_ms, provider_balance_compact_lang, shorten,
    shorten_middle, station_balance_brief_lang,
};
use crate::tui::state::UiState;
use crate::tui::{Language, ProviderOption};

const ROUTING_BALANCE_COLUMN_WIDTH: u16 = 24;
const ROUTING_BALANCE_SUMMARY_WIDTH: usize = 32;

fn station_routing_posture(
    providers: &[ProviderOption],
    station_meta_overrides: &HashMap<String, (Option<bool>, Option<u8>)>,
    lb_view: &HashMap<String, crate::state::LbConfigView>,
    provider_balances: &HashMap<String, Vec<crate::state::ProviderBalanceSnapshot>>,
    session_override: Option<&str>,
    global_station_override: Option<&str>,
    retry: Option<&crate::config::ResolvedRetryConfig>,
) -> StationRoutingPosture {
    let candidates = providers
        .iter()
        .map(|provider| {
            let lb = lb_view.get(provider.name.as_str());
            let (enabled, level) =
                effective_station_enabled_level(provider, station_meta_overrides);
            StationRoutingCandidate {
                name: provider.name.clone(),
                alias: provider.alias.clone(),
                level,
                enabled,
                active: provider.active,
                upstreams: lb
                    .map(|view| view.upstreams.len())
                    .or(Some(provider.upstreams.len())),
                runtime_state: crate::state::RuntimeConfigState::Normal,
                has_cooldown: lb.is_some_and(|view| {
                    view.upstreams
                        .iter()
                        .any(|upstream| upstream.cooldown_remaining_secs.is_some())
                }),
                any_usage_exhausted: lb.is_some_and(|view| {
                    view.upstreams
                        .iter()
                        .any(|upstream| upstream.usage_exhausted)
                }),
                all_usage_exhausted: lb.is_some_and(|view| {
                    !view.upstreams.is_empty()
                        && view
                            .upstreams
                            .iter()
                            .all(|upstream| upstream.usage_exhausted)
                }),
                balance: crate::dashboard_core::StationRoutingBalanceSummary::from_snapshots(
                    provider_balances
                        .get(provider.name.as_str())
                        .map(Vec::as_slice),
                ),
            }
        })
        .collect::<Vec<_>>();
    let configured_active_station = providers
        .iter()
        .find(|provider| provider.active)
        .map(|provider| provider.name.as_str());

    build_station_routing_posture(StationRoutingPostureInput {
        stations: &candidates,
        session_station_override: session_override,
        global_station_override,
        configured_active_station,
        session_pin_count: 0,
        retry,
    })
}

fn effective_station_enabled_level(
    provider: &ProviderOption,
    station_meta_overrides: &HashMap<String, (Option<bool>, Option<u8>)>,
) -> (bool, u8) {
    let (enabled_override, level_override) = station_meta_overrides
        .get(provider.name.as_str())
        .copied()
        .unwrap_or((None, None));
    (
        enabled_override.unwrap_or(provider.enabled),
        level_override.unwrap_or(provider.level).clamp(1, 10),
    )
}

fn format_routing_source_lang(source: &StationRoutingSource, lang: Language) -> String {
    match source {
        StationRoutingSource::SessionPin(station) => match lang {
            Language::Zh => format!("会话 pin={station}"),
            Language::En => format!("session pin={station}"),
        },
        StationRoutingSource::GlobalPin(station) => match lang {
            Language::Zh => format!("全局 pin={station}"),
            Language::En => format!("global pin={station}"),
        },
        StationRoutingSource::ConfiguredActiveStation(station) => match lang {
            Language::Zh => format!("配置活跃站点={station}"),
            Language::En => format!("configured active={station}"),
        },
        StationRoutingSource::Auto => i18n::label(lang, "auto").to_string(),
    }
}

fn format_routing_mode_lang(mode: StationRoutingMode, lang: Language) -> &'static str {
    match mode {
        StationRoutingMode::PinnedStation => i18n::label(lang, "pinned"),
        StationRoutingMode::AutoLevelFallback => i18n::label(lang, "auto(level fallback)"),
        StationRoutingMode::AutoSingleLevelFallback => {
            i18n::label(lang, "auto(single-level fallback)")
        }
    }
}

#[cfg(test)]
fn format_routing_order_hint(mode: StationRoutingMode) -> &'static str {
    format_routing_order_hint_lang(mode, Language::En)
}

fn format_routing_order_hint_lang(mode: StationRoutingMode, lang: Language) -> &'static str {
    match mode {
        StationRoutingMode::PinnedStation => i18n::label(
            lang,
            "pinned target only; breaker_open / empty upstreams block.",
        ),
        StationRoutingMode::AutoLevelFallback => i18n::label(
            lang,
            "known fully exhausted stations are demoted by default; provider-level exceptions only show balance/quota.",
        ),
        StationRoutingMode::AutoSingleLevelFallback => i18n::label(
            lang,
            "known fully exhausted stations are demoted by default unless a provider opts out of routing trust.",
        ),
    }
}

fn format_retry_boundary_lang(boundary: StationRetryBoundary, lang: Language) -> String {
    match boundary {
        StationRetryBoundary::Unknown => {
            i18n::label(lang, "resolved policy unavailable").to_string()
        }
        StationRetryBoundary::CrossStationBeforeFirstOutput {
            provider_max_attempts,
        } => match lang {
            Language::Zh => {
                format!("provider failover x{provider_max_attempts}；首个输出前允许跨站点")
            }
            Language::En => format!(
                "provider failover x{provider_max_attempts}; cross-station before first output"
            ),
        },
        StationRetryBoundary::CurrentStationFirst {
            provider_strategy,
            provider_max_attempts,
        } => match lang {
            Language::Zh => {
                format!("provider {provider_strategy:?} x{provider_max_attempts}；选中站点优先")
                    .to_ascii_lowercase()
            }
            Language::En => format!(
                "provider {provider_strategy:?} x{provider_max_attempts}; selected station first"
            )
            .to_ascii_lowercase(),
        },
        StationRetryBoundary::NextRequestOnly => match lang {
            Language::Zh => "provider x1；下次路由请求自动切换".to_string(),
            Language::En => "provider x1; auto switch on next routed request".to_string(),
        },
    }
}

#[cfg(test)]
fn format_routing_candidate(candidate: &StationRoutingCandidate) -> String {
    format_routing_candidate_lang(candidate, Language::En)
}

fn format_routing_candidate_lang(candidate: &StationRoutingCandidate, lang: Language) -> String {
    let mut parts = vec![format!("L{}", candidate.level.clamp(1, 10))];
    if candidate.active {
        parts.push(i18n::label(lang, "active").to_string());
    }
    match candidate.upstreams {
        Some(upstreams) => parts.push(format!("{}={upstreams}", i18n::label(lang, "upstreams"))),
        None => parts.push(format!("{}=?", i18n::label(lang, "upstreams"))),
    }
    if candidate.has_cooldown {
        parts.push(i18n::label(lang, "cooldown").to_string());
    }
    if candidate.all_usage_exhausted {
        parts.push(format!(
            "{}={}",
            i18n::label(lang, "quota"),
            i18n::label(lang, "all_exhausted")
        ));
    } else if candidate.any_usage_exhausted {
        parts.push(format!(
            "{}={}",
            i18n::label(lang, "quota"),
            i18n::label(lang, "partial_exhausted")
        ));
    }
    if !candidate.balance.is_empty() {
        parts.push(format_routing_balance_lang(candidate, lang));
    }

    format!("{} [{}]", candidate.name, parts.join(", "))
}

fn format_routing_balance_lang(candidate: &StationRoutingCandidate, lang: Language) -> String {
    if lang == Language::En {
        return format_routing_balance_en(candidate);
    }

    let balance = &candidate.balance;
    let mut parts = Vec::new();
    if balance.routing_snapshots == 0 {
        if balance.exhausted > 0 {
            parts.push("耗尽但不参与路由".to_string());
        }
    } else if balance.routing_exhausted == balance.routing_snapshots {
        parts.push("路由可见全部耗尽".to_string());
    } else if balance.routing_exhausted > 0 {
        parts.push(format!(
            "耗尽={}/{}",
            balance.routing_exhausted, balance.routing_snapshots
        ));
    }
    if balance.routing_ignored_exhausted > 0 {
        parts.push(format!(
            "路由忽略耗尽={}",
            balance.routing_ignored_exhausted
        ));
    }
    if balance.stale > 0 {
        parts.push(format!("{}={}", i18n::label(lang, "stale"), balance.stale));
    }
    let unknown = balance.unknown + balance.error;
    if unknown > 0 {
        parts.push(format!("{}={unknown}", i18n::label(lang, "unknown")));
    }
    if parts.is_empty() {
        format!(
            "{}={}({})",
            i18n::label(lang, "balance"),
            i18n::label(lang, "ok"),
            balance.snapshots
        )
    } else {
        format!("{}={}", i18n::label(lang, "balance"), parts.join("/"))
    }
}

fn format_routing_balance_en(candidate: &StationRoutingCandidate) -> String {
    let balance = &candidate.balance;
    let mut parts = Vec::new();
    if balance.routing_snapshots == 0 {
        if balance.exhausted > 0 {
            parts.push("exhausted_untrusted".to_string());
        }
    } else if balance.routing_exhausted == balance.routing_snapshots {
        parts.push("exhausted_all".to_string());
    } else if balance.routing_exhausted > 0 {
        parts.push(format!(
            "exhausted={}/{}",
            balance.routing_exhausted, balance.routing_snapshots
        ));
    }
    if balance.routing_ignored_exhausted > 0 {
        parts.push(format!(
            "ignored_for_routing={}",
            balance.routing_ignored_exhausted
        ));
    }
    if balance.stale > 0 {
        parts.push(format!("stale={}", balance.stale));
    }
    let unknown = balance.unknown + balance.error;
    if unknown > 0 {
        parts.push(format!("unknown={unknown}"));
    }
    if parts.is_empty() {
        format!("balance=ok({})", balance.snapshots)
    } else {
        format!("balance={}", parts.join("/"))
    }
}

fn format_skipped_station_lang(
    skipped: &crate::dashboard_core::StationRoutingSkipped,
    lang: Language,
) -> String {
    format!(
        "{}: {}",
        skipped.station_name,
        skipped
            .reasons
            .iter()
            .map(|reason| format_skip_reason_lang(reason, lang))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn format_skip_reason_lang(reason: &StationRoutingSkipReason, lang: Language) -> String {
    match reason {
        StationRoutingSkipReason::Disabled => i18n::label(lang, "disabled").to_string(),
        StationRoutingSkipReason::RuntimeState(state) => match lang {
            Language::Zh => format!("状态={state:?}").to_ascii_lowercase(),
            Language::En => format!("state={state:?}").to_ascii_lowercase(),
        },
        StationRoutingSkipReason::NoRoutableUpstreams => {
            i18n::label(lang, "no_upstreams").to_string()
        }
        StationRoutingSkipReason::MissingPinnedTarget => {
            i18n::label(lang, "missing_pinned_station").to_string()
        }
        StationRoutingSkipReason::BreakerOpenBlocksPinned => {
            i18n::label(lang, "breaker_open_blocks_pin").to_string()
        }
    }
}

fn format_runtime_selected_route(
    explain: &crate::routing_explain::RoutingExplainResponse,
) -> String {
    match explain.selected_route.as_ref() {
        Some(selected) => {
            let compatibility = format_runtime_compatibility(selected.compatibility.as_ref());
            format!(
                "selected={} endpoint={} {} path={}",
                selected.provider_id,
                selected.endpoint_id,
                compatibility,
                selected.route_path.join(" > ")
            )
        }
        None => "selected=<none>".to_string(),
    }
}

fn format_runtime_candidate(candidate: &crate::routing_explain::RoutingExplainCandidate) -> String {
    let marker = if candidate.selected { "*" } else { " " };
    let compatibility = format_runtime_compatibility(candidate.compatibility.as_ref());
    format!(
        "{} {} endpoint={} {} skip={}",
        marker,
        candidate.provider_id,
        candidate.endpoint_id,
        compatibility,
        format_runtime_skip_reasons(&candidate.skip_reasons)
    )
}

fn format_runtime_compatibility(
    compatibility: Option<&crate::routing_explain::RoutingExplainCompatibility>,
) -> String {
    compatibility
        .map(|compatibility| {
            format!(
                "compat_station={} upstream#{}",
                compatibility.station_name, compatibility.upstream_index
            )
        })
        .unwrap_or_else(|| "compatibility=-".to_string())
}

fn format_runtime_skip_reasons(
    reasons: &[crate::routing_explain::RoutingExplainSkipReason],
) -> String {
    if reasons.is_empty() {
        return "-".to_string();
    }
    reasons
        .iter()
        .map(crate::routing_explain::RoutingExplainSkipReason::code)
        .collect::<Vec<_>>()
        .join(",")
}

fn routing_policy_label(policy: crate::config::RoutingPolicyV4) -> &'static str {
    match policy {
        crate::config::RoutingPolicyV4::ManualSticky => "manual-sticky",
        crate::config::RoutingPolicyV4::OrderedFailover => "ordered-failover",
        crate::config::RoutingPolicyV4::TagPreferred => "tag-preferred",
        crate::config::RoutingPolicyV4::Conditional => "conditional",
    }
}

fn routing_exhausted_label(action: crate::config::RoutingExhaustedActionV4) -> &'static str {
    match action {
        crate::config::RoutingExhaustedActionV4::Continue => "continue",
        crate::config::RoutingExhaustedActionV4::Stop => "stop",
    }
}

fn routing_tags_label(tags: &BTreeMap<String, String>, max_width: usize) -> String {
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

fn routing_prefer_tags_label(filters: &[BTreeMap<String, String>], max_width: usize) -> String {
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

fn route_graph_tree_text_lines(
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
fn push_route_graph_ref_text_lines(
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
fn push_route_graph_children_text_lines(
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

fn route_graph_node_brief(node: &crate::config::RoutingNodeV4, lang: Language) -> String {
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

fn route_graph_condition_label(condition: Option<&crate::config::RoutingConditionV4>) -> String {
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

fn routing_provider_matches_preference(
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

fn routing_provider_balance_snapshots<'a>(
    snapshot: &'a Snapshot,
    provider_name: &str,
) -> Vec<&'a crate::state::ProviderBalanceSnapshot> {
    let mut matches = snapshot
        .provider_balances
        .iter()
        .flat_map(|(key, balances)| {
            balances.iter().filter(move |balance| {
                balance.provider_id == provider_name
                    || (balance.provider_id.trim().is_empty() && key == provider_name)
            })
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        left.upstream_index
            .cmp(&right.upstream_index)
            .then_with(|| right.fetched_at_ms.cmp(&left.fetched_at_ms))
    });
    matches
}

fn routing_provider_balance_brief_lang(
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

fn routing_provider_balance_cell_lang(
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

fn routing_provider_display_label(
    provider: Option<&crate::tui::state::RoutingProviderRow>,
    provider_name: &str,
) -> String {
    provider
        .map(crate::tui::state::RoutingProviderRow::display_label)
        .unwrap_or_else(|| provider_name.to_string())
}

fn route_target_summary_line(
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

fn push_wrapped_segments<'a>(
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

fn push_wrapped_segment_body<'a>(
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

fn folded_route_chain_segments(
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

fn route_chain_summary(segments: &[String], selected: Option<&str>, lang: Language) -> String {
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

fn push_route_chain<'a>(
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

fn routing_provider_marker(
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

fn render_route_graph_routing_page(
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

pub(super) fn render_stations_page(
    f: &mut Frame<'_>,
    p: Palette,
    ui: &mut UiState,
    snapshot: &Snapshot,
    providers: &[ProviderOption],
    area: Rect,
) {
    if ui.uses_route_graph_routing() {
        render_route_graph_routing_page(f, p, ui, snapshot, area);
        return;
    }
    let lang = ui.language;
    let l = |text| i18n::label(lang, text);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);
    let now = now_ms();

    let selected_session = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|r| r.session_id.as_deref())
        .unwrap_or("-");
    let session_override = snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|r| r.override_station_name.as_deref());
    let global_station_override = snapshot.global_station_override.as_deref();

    let left_block = Block::default()
        .title(Span::styled(
            format!("{}  ({}: {selected_session})", l("Stations"), l("session")),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(p.border))
        .style(Style::default().bg(p.panel));

    let header = Row::new([
        l("Lvl"),
        l("Name"),
        l("On"),
        l("Up"),
        l("Balance/Quota"),
        l("Health"),
    ])
    .style(Style::default().fg(p.muted))
    .height(1);

    let rows = providers
        .iter()
        .map(|cfg| {
            let (enabled_ovr, level_ovr) = snapshot
                .station_meta_overrides
                .get(cfg.name.as_str())
                .copied()
                .unwrap_or((None, None));
            let enabled = enabled_ovr.unwrap_or(cfg.enabled);
            let level = level_ovr.unwrap_or(cfg.level).clamp(1, 10);

            let mut name = cfg.name.clone();
            if cfg.active {
                name = format!("* {name}");
            }

            let on = if enabled { l("on") } else { l("off") };
            let up = cfg.upstreams.len().to_string();
            let balance = station_balance_brief_lang(
                &snapshot.provider_balances,
                cfg.name.as_str(),
                18,
                lang,
            );
            let health = if let Some(st) = snapshot.health_checks.get(cfg.name.as_str())
                && !st.done
            {
                if st.cancel_requested {
                    format!("{} {}/{}", l("cancel"), st.completed, st.total.max(1))
                } else {
                    format!("{} {}/{}", l("run"), st.completed, st.total.max(1))
                }
            } else if let Some(st) = snapshot.health_checks.get(cfg.name.as_str())
                && st.done
                && st.canceled
            {
                l("canceled").to_string()
            } else {
                snapshot
                    .station_health
                    .get(cfg.name.as_str())
                    .map(|h| {
                        let total = h.upstreams.len().max(1);
                        let ok = h.upstreams.iter().filter(|u| u.ok == Some(true)).count();
                        let best_ms = h
                            .upstreams
                            .iter()
                            .filter(|u| u.ok == Some(true))
                            .filter_map(|u| u.latency_ms)
                            .min();
                        if ok > 0 {
                            if let Some(ms) = best_ms {
                                format!("{ok}/{total} {ms}ms")
                            } else {
                                format!("{ok}/{total} {}", l("ok"))
                            }
                        } else {
                            let status = h.upstreams.iter().filter_map(|u| u.status_code).next();
                            if let Some(code) = status {
                                format!("{} {code}", l("err"))
                            } else {
                                l("err").to_string()
                            }
                        }
                    })
                    .unwrap_or_else(|| "-".to_string())
            };

            let mut style = Style::default().fg(if enabled { p.text } else { p.muted });
            if global_station_override == Some(cfg.name.as_str()) {
                style = style.fg(p.accent).add_modifier(Modifier::BOLD);
            }
            if session_override == Some(cfg.name.as_str()) {
                style = style.fg(p.focus).add_modifier(Modifier::BOLD);
            }

            Row::new([
                format!("L{level}"),
                name,
                on.to_string(),
                up,
                balance,
                health,
            ])
            .style(style)
            .height(1)
        })
        .collect::<Vec<_>>();

    let table_visible_rows = usize::from(left_block.inner(columns[0]).height.saturating_sub(1));
    ui.sync_stations_table_viewport(providers.len(), table_visible_rows);

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(18),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(left_block)
    .row_highlight_style(Style::default().bg(Color::Rgb(32, 39, 48)).fg(p.text))
    .highlight_symbol("  ")
    .highlight_spacing(HighlightSpacing::Always);
    f.render_stateful_widget(table, columns[0], &mut ui.stations_table);

    let selected = providers.get(ui.selected_station_idx);
    let right_title = selected
        .map(|c| {
            format!(
                "{}: {} (L{})",
                l("Station details"),
                c.name,
                c.level.clamp(1, 10)
            )
        })
        .unwrap_or_else(|| l("Station details").to_string());

    let mut lines = Vec::new();
    if let Some(cfg) = selected {
        let (enabled_ovr, level_ovr) = snapshot
            .station_meta_overrides
            .get(cfg.name.as_str())
            .copied()
            .unwrap_or((None, None));
        let enabled = enabled_ovr.unwrap_or(cfg.enabled);
        let level = level_ovr.unwrap_or(cfg.level).clamp(1, 10);
        let level_note = if level_ovr.is_some() {
            match lang {
                Language::Zh => "（覆盖）",
                Language::En => " (override)",
            }
        } else {
            ""
        };
        let enabled_note = if enabled_ovr.is_some() {
            match lang {
                Language::Zh => "（覆盖）",
                Language::En => " (override)",
            }
        } else {
            ""
        };

        if let Some(alias) = cfg.alias.as_deref()
            && !alias.trim().is_empty()
        {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("alias")), Style::default().fg(p.muted)),
                Span::styled(alias.to_string(), Style::default().fg(p.text)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("enabled")), Style::default().fg(p.muted)),
            Span::styled(
                format!("{}{enabled_note}", if enabled { l("yes") } else { l("no") }),
                Style::default().fg(if enabled { p.good } else { p.warn }),
            ),
            Span::raw("   "),
            Span::styled(format!("{}: ", l("Lvl")), Style::default().fg(p.muted)),
            Span::styled(
                format!("L{level}{level_note}"),
                Style::default().fg(p.muted),
            ),
            Span::raw("   "),
            Span::styled(format!("{}: ", l("active")), Style::default().fg(p.muted)),
            Span::styled(
                if cfg.active { l("yes") } else { l("no") },
                Style::default().fg(if cfg.active { p.accent } else { p.muted }),
            ),
        ]));

        let routing = station_routing_posture(
            providers,
            &snapshot.station_meta_overrides,
            &snapshot.lb_view,
            &snapshot.provider_balances,
            session_override,
            global_station_override,
            ui.last_runtime_retry.as_ref(),
        );
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("routing")), Style::default().fg(p.muted)),
            Span::styled(
                format!(
                    "{} · {}",
                    format_routing_source_lang(&routing.source, lang),
                    format_routing_mode_lang(routing.mode, lang)
                ),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("{}: ", l("retry")), Style::default().fg(p.muted)),
            Span::styled(
                shorten_middle(
                    &format_retry_boundary_lang(routing.retry_boundary, lang),
                    96,
                ),
                Style::default().fg(p.muted),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", l("order_rule")),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                shorten_middle(format_routing_order_hint_lang(routing.mode, lang), 96),
                Style::default().fg(p.muted),
            ),
        ]));
        let observations = summarize_recent_retry_observations(&snapshot.recent);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", l("Recent sample")),
                Style::default().fg(p.muted),
            ),
            Span::styled(
                format!(
                    "retry={} same={} cross={} fast={}",
                    observations.recent_retried_requests,
                    observations.recent_same_station_retries,
                    observations.recent_cross_station_failovers,
                    observations.recent_fast_mode_requests
                ),
                Style::default().fg(p.muted),
            ),
        ]));
        if !routing.eligible_candidates.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("order")), Style::default().fg(p.muted)),
                Span::styled(
                    shorten_middle(
                        &routing
                            .eligible_candidates
                            .iter()
                            .map(|candidate| format_routing_candidate_lang(candidate, lang))
                            .collect::<Vec<_>>()
                            .join(" > "),
                        96,
                    ),
                    Style::default().fg(p.text),
                ),
            ]));
        }
        if !routing.skipped.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("skipped")), Style::default().fg(p.muted)),
                Span::styled(
                    shorten_middle(
                        &routing
                            .skipped
                            .iter()
                            .map(|skipped| format_skipped_station_lang(skipped, lang))
                            .collect::<Vec<_>>()
                            .join(" | "),
                        96,
                    ),
                    Style::default().fg(p.muted),
                ),
            ]));
        }
        if let Some(explain) = ui.routing_explain.as_ref() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}: ", l("Runtime route")),
                    Style::default().fg(p.muted),
                ),
                Span::styled(
                    shorten_middle(&format_runtime_selected_route(explain), 96),
                    Style::default().fg(p.text),
                ),
            ]));
            let candidates = explain
                .candidates
                .iter()
                .map(format_runtime_candidate)
                .collect::<Vec<_>>()
                .join(" | ");
            if !candidates.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{}: ", l("Runtime candidates")),
                        Style::default().fg(p.muted),
                    ),
                    Span::styled(
                        shorten_middle(&candidates, 96),
                        Style::default().fg(p.muted),
                    ),
                ]));
            }
        }

        if let Some(st) = snapshot.health_checks.get(cfg.name.as_str()) {
            let status = if !st.done {
                if st.cancel_requested {
                    format!("{} {}/{}", l("cancel"), st.completed, st.total.max(1))
                } else {
                    format!("{} {}/{}", l("running"), st.completed, st.total.max(1))
                }
            } else if st.canceled {
                l("canceled").to_string()
            } else {
                l("done").to_string()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}: ", l("health_check")),
                    Style::default().fg(p.muted),
                ),
                Span::styled(
                    status,
                    Style::default().fg(if st.done && !st.canceled {
                        p.good
                    } else {
                        p.warn
                    }),
                ),
            ]));
            if let Some(e) = st.last_error.as_deref()
                && !e.trim().is_empty()
            {
                lines.push(Line::from(vec![
                    Span::raw("             "),
                    Span::styled(shorten(e, 80), Style::default().fg(p.muted)),
                ]));
            }
        }

        if let Some(health) = snapshot.station_health.get(cfg.name.as_str()) {
            let age = format_age(now, Some(health.checked_at_ms));
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("health")), Style::default().fg(p.muted)),
                Span::styled(
                    match lang {
                        crate::tui::Language::Zh => format!("{age} 前检查"),
                        crate::tui::Language::En => format!("checked {age} ago"),
                    },
                    Style::default().fg(p.muted).add_modifier(Modifier::DIM),
                ),
            ]));
            for (idx, u) in health.upstreams.iter().enumerate() {
                let ok = u.ok.unwrap_or(false);
                let status = u
                    .status_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "-".to_string());
                let ms = u
                    .latency_ms
                    .map(|c| format!("{c}ms"))
                    .unwrap_or_else(|| "-".to_string());
                let head = format!("{idx:>2}. ");
                lines.push(Line::from(vec![
                    Span::styled(head, Style::default().fg(p.muted)),
                    Span::styled(
                        if ok { l("ok") } else { l("err") },
                        Style::default().fg(if ok { p.good } else { p.warn }),
                    ),
                    Span::raw("  "),
                    Span::styled(status, Style::default().fg(p.muted)),
                    Span::raw("  "),
                    Span::styled(ms, Style::default().fg(p.muted)),
                    Span::raw("  "),
                    Span::styled(shorten_middle(&u.base_url, 60), Style::default().fg(p.text)),
                ]));
                if !ok
                    && let Some(e) = u.error.as_deref()
                    && !e.trim().is_empty()
                {
                    lines.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(shorten(e, 80), Style::default().fg(p.muted)),
                    ]));
                }
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled(format!("{}: ", l("health")), Style::default().fg(p.muted)),
                Span::styled(
                    i18n::text(lang, msg::NOT_CHECKED),
                    Style::default().fg(p.muted).add_modifier(Modifier::DIM),
                ),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Balance / quota"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if let Some(balances) = snapshot.provider_balances.get(cfg.name.as_str()) {
            if balances.is_empty() {
                lines.push(Line::from(Span::styled(
                    i18n::text(lang, msg::NONE_PARENS),
                    Style::default().fg(p.muted),
                )));
            } else {
                for balance in balances.iter().take(12) {
                    let idx = balance
                        .upstream_index
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    lines.push(Line::from(vec![
                        Span::styled(format!("{idx:>2}. "), Style::default().fg(p.muted)),
                        Span::styled(
                            shorten_middle(&balance.provider_id, 20),
                            Style::default().fg(p.text),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            balance_snapshot_status_label_lang(balance, lang),
                            balance_snapshot_status_style(p, balance),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            shorten_middle(
                                &balance_amount_brief_lang(balance, lang)
                                    .unwrap_or_else(|| balance.amount_summary()),
                                56,
                            ),
                            Style::default().fg(p.muted),
                        ),
                    ]));
                    if let Some(err) = balance.error.as_deref()
                        && !err.trim().is_empty()
                    {
                        lines.push(Line::from(vec![
                            Span::raw("     "),
                            Span::styled(
                                format!("{}: {}", l("balance lookup failed"), shorten(err, 56)),
                                Style::default().fg(p.muted),
                            ),
                        ]));
                    }
                }
                if balances.len() > 12 {
                    lines.push(Line::from(Span::styled(
                        format!("… +{} more", balances.len() - 12),
                        Style::default().fg(p.muted),
                    )));
                }
            }
        } else {
            lines.push(Line::from(Span::styled(
                i18n::text(lang, msg::NONE_PARENS),
                Style::default().fg(p.muted),
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Upstreams"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        if cfg.upstreams.is_empty() {
            lines.push(Line::from(Span::styled(
                i18n::text(lang, msg::NONE_PARENS),
                Style::default().fg(p.muted),
            )));
        } else {
            for (idx, u) in cfg.upstreams.iter().enumerate() {
                let pid = u.provider_id.as_deref().unwrap_or("-");
                lines.push(Line::from(vec![
                    Span::styled(format!("{idx:>2}. "), Style::default().fg(p.muted)),
                    Span::styled(pid.to_string(), Style::default().fg(p.muted)),
                    Span::raw("  "),
                    Span::styled(u.base_url.clone(), Style::default().fg(p.text)),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            l("Actions"),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(i18n::text(
            ui.language,
            msg::ROUTING_ACTION_PROVIDER_DETAILS,
        )));
        lines.extend(match lang {
            crate::tui::Language::Zh => vec![
                Line::from("  Enter        设置全局 pin"),
                Line::from("  Backspace    清除全局 pin（auto）"),
                Line::from("  r            routing 编辑器（策略/顺序/标签）"),
                Line::from("  o            将会话覆盖设置为选中站点"),
                Line::from("  O            清除会话覆盖"),
            ],
            crate::tui::Language::En => vec![
                Line::from("  Enter        set global pin"),
                Line::from("  Backspace    clear global pin (auto)"),
                Line::from("  r            routing editor (policy/order/tags)"),
                Line::from("  o            set session override to selected station"),
                Line::from("  O            clear session override"),
            ],
        });
        lines.extend(match lang {
            crate::tui::Language::Zh => vec![
                Line::from("  h            检查选中站点健康"),
                Line::from("  H            检查全部站点健康"),
                Line::from("  c            取消健康检查（选中）"),
                Line::from("  C            取消健康检查（全部）"),
            ],
            crate::tui::Language::En => vec![
                Line::from("  h            health check selected station"),
                Line::from("  H            health check all stations"),
                Line::from("  c            cancel health check (selected)"),
                Line::from("  C            cancel health check (all)"),
            ],
        });
    } else {
        lines.push(Line::from(Span::styled(
            l("No stations available."),
            Style::default().fg(p.muted),
        )));
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use crate::state::BalanceSnapshotStatus;
    use crate::tui::UpstreamSummary;
    use crate::tui::model::station_balance_brief;
    use crate::tui::types::Page;

    fn provider(
        name: &str,
        enabled: bool,
        level: u8,
        active: bool,
        upstreams: usize,
    ) -> ProviderOption {
        ProviderOption {
            name: name.to_string(),
            alias: None,
            enabled,
            level,
            active,
            upstreams: (0..upstreams)
                .map(|idx| UpstreamSummary {
                    base_url: format!("https://{name}-{idx}.example/v1"),
                    ..UpstreamSummary::default()
                })
                .collect(),
        }
    }

    fn empty_snapshot(
        provider_balances: HashMap<String, Vec<crate::state::ProviderBalanceSnapshot>>,
        global_route_target_override: Option<String>,
    ) -> Snapshot {
        Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            model_overrides: HashMap::new(),
            overrides: HashMap::new(),
            station_overrides: HashMap::new(),
            route_target_overrides: HashMap::new(),
            service_tier_overrides: HashMap::new(),
            global_station_override: None,
            global_route_target_override,
            station_meta_overrides: HashMap::new(),
            usage_rollup: crate::state::UsageRollupView::default(),
            provider_balances,
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
            refreshed_at: Instant::now(),
        }
    }

    fn routing_provider(name: &str) -> crate::tui::model::RoutingProviderRef {
        crate::tui::model::RoutingProviderRef {
            name: name.to_string(),
            alias: None,
            enabled: true,
            tags: BTreeMap::new(),
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

    fn render_stations_text(
        width: u16,
        height: u16,
        ui: &mut UiState,
        snapshot: &Snapshot,
    ) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let frame = terminal
            .draw(|frame| {
                render_stations_page(frame, Palette::default(), ui, snapshot, &[], frame.area());
            })
            .expect("draw");
        buffer_text(frame.buffer)
    }

    #[test]
    fn station_routing_preview_uses_single_level_fallback_order() {
        let providers = vec![
            provider("alpha", true, 1, false, 1),
            provider("beta", true, 1, true, 1),
            provider("disabled", false, 1, false, 1),
        ];
        let lb_view = HashMap::new();

        let provider_balances = HashMap::new();

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );

        assert_eq!(preview.mode, StationRoutingMode::AutoSingleLevelFallback);
        assert_eq!(preview.eligible_candidates[0].name, "beta");
        assert_eq!(preview.eligible_candidates[1].name, "alpha");
        assert_eq!(preview.skipped[0].station_name, "disabled");
        assert_eq!(
            preview.skipped[0].reasons,
            vec![StationRoutingSkipReason::Disabled]
        );
    }

    #[test]
    fn station_routing_preview_sorts_multi_level_and_active_tiebreak() {
        let providers = vec![
            provider("alpha", true, 2, false, 1),
            provider("beta", true, 1, false, 1),
            provider("zeta", true, 2, true, 1),
        ];
        let lb_view = HashMap::new();

        let provider_balances = HashMap::new();

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );

        assert_eq!(preview.mode, StationRoutingMode::AutoLevelFallback);
        assert_eq!(preview.eligible_candidates[0].name, "beta");
        assert_eq!(preview.eligible_candidates[1].name, "zeta");
        assert_eq!(preview.eligible_candidates[2].name, "alpha");
    }

    #[test]
    fn station_routing_preview_applies_runtime_meta_overrides() {
        let providers = vec![
            provider("alpha", true, 3, false, 1),
            provider("beta", true, 3, false, 1),
        ];
        let overrides = HashMap::from([
            ("alpha".to_string(), (Some(false), Some(1))),
            ("beta".to_string(), (None, Some(2))),
        ]);
        let lb_view = HashMap::new();

        let provider_balances = HashMap::new();

        let preview = station_routing_posture(
            &providers,
            &overrides,
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );

        assert_eq!(preview.eligible_candidates[0].name, "beta");
        assert_eq!(preview.eligible_candidates[0].level, 2);
        assert_eq!(preview.skipped[0].station_name, "alpha");
        assert_eq!(
            preview.skipped[0].reasons,
            vec![StationRoutingSkipReason::Disabled]
        );
    }

    #[test]
    fn station_routing_preview_marks_pinned_targets() {
        let providers = vec![provider("alpha", false, 1, false, 0)];
        let lb_view = HashMap::new();
        let provider_balances = HashMap::new();

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            Some("alpha"),
            None,
            None,
        );

        assert_eq!(preview.mode, StationRoutingMode::PinnedStation);
        assert!(matches!(
            preview.source,
            StationRoutingSource::SessionPin(ref station) if station == "alpha"
        ));
        assert!(preview.eligible_candidates.is_empty());
        assert_eq!(
            preview.skipped[0].reasons,
            vec![StationRoutingSkipReason::NoRoutableUpstreams]
        );
    }

    #[test]
    fn station_routing_preview_marks_balance_warnings() {
        let providers = vec![provider("alpha", true, 1, true, 1)];
        let lb_view = HashMap::new();
        let provider_balances = HashMap::from([(
            "alpha".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Exhausted,
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]);

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );
        let label = format_routing_candidate(&preview.eligible_candidates[0]);

        assert!(label.contains("balance=exhausted_all"));
    }

    #[test]
    fn station_routing_preview_marks_ignored_routing_exhaustion() {
        let providers = vec![provider("alpha", true, 1, true, 1)];
        let lb_view = HashMap::new();
        let provider_balances = HashMap::from([(
            "alpha".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Exhausted,
                exhaustion_affects_routing: false,
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]);

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );
        let label = format_routing_candidate(&preview.eligible_candidates[0]);

        assert!(label.contains("balance=exhausted_untrusted"));
        assert!(label.contains("ignored_for_routing=1"));
    }

    #[test]
    fn station_routing_preview_does_not_treat_unknown_balance_as_ok() {
        let providers = vec![provider("alpha", true, 1, true, 1)];
        let lb_view = HashMap::new();
        let provider_balances = HashMap::from([(
            "alpha".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Unknown,
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]);

        let preview = station_routing_posture(
            &providers,
            &HashMap::new(),
            &lb_view,
            &provider_balances,
            None,
            None,
            None,
        );
        let label = format_routing_candidate(&preview.eligible_candidates[0]);

        assert!(label.contains("balance=unknown=1"));
        assert!(!label.contains("balance=ok"));
    }

    #[test]
    fn routing_order_hint_explains_balance_demotion() {
        let text = format_routing_order_hint(StationRoutingMode::AutoLevelFallback);

        assert!(text.contains("demoted by default"));
        assert!(text.contains("provider-level exceptions"));
    }

    #[test]
    fn route_graph_tree_text_lines_show_nested_routes_and_missing_refs() {
        let spec = crate::tui::model::RoutingSpecView {
            entry: "main".to_string(),
            routes: BTreeMap::from([
                (
                    "main".to_string(),
                    crate::config::RoutingNodeV4 {
                        strategy: crate::config::RoutingPolicyV4::OrderedFailover,
                        children: vec!["monthly_pool".to_string(), "missing_provider".to_string()],
                        ..crate::config::RoutingNodeV4::default()
                    },
                ),
                (
                    "monthly_pool".to_string(),
                    crate::config::RoutingNodeV4 {
                        strategy: crate::config::RoutingPolicyV4::TagPreferred,
                        children: vec!["monthly_a".to_string(), "paygo_b".to_string()],
                        prefer_tags: vec![BTreeMap::from([(
                            "billing".to_string(),
                            "monthly".to_string(),
                        )])],
                        ..crate::config::RoutingNodeV4::default()
                    },
                ),
            ]),
            policy: crate::config::RoutingPolicyV4::OrderedFailover,
            order: Vec::new(),
            target: None,
            prefer_tags: Vec::new(),
            chain: Vec::new(),
            pools: BTreeMap::new(),
            on_exhausted: crate::config::RoutingExhaustedActionV4::Continue,
            entry_strategy: crate::config::RoutingPolicyV4::OrderedFailover,
            expanded_order: Vec::new(),
            entry_target: None,
            providers: vec![
                crate::tui::model::RoutingProviderRef {
                    name: "monthly_a".to_string(),
                    alias: None,
                    enabled: true,
                    tags: BTreeMap::from([("billing".to_string(), "monthly".to_string())]),
                },
                crate::tui::model::RoutingProviderRef {
                    name: "paygo_b".to_string(),
                    alias: None,
                    enabled: false,
                    tags: BTreeMap::new(),
                },
            ],
        };

        let lines = route_graph_tree_text_lines(&spec, Language::En);

        assert!(lines.iter().any(|line| line.contains("entry route main")));
        assert!(lines.iter().any(|line| line.contains("route monthly_pool")));
        assert!(lines.iter().any(|line| line.contains("provider monthly_a")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("provider paygo_b [off"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("missing ref missing_provider"))
        );
    }

    #[test]
    fn station_balance_brief_shows_single_amount() {
        let provider_balances = HashMap::from([(
            "alpha".to_string(),
            vec![crate::state::ProviderBalanceSnapshot {
                status: BalanceSnapshotStatus::Ok,
                total_balance_usd: Some("3.50".to_string()),
                ..crate::state::ProviderBalanceSnapshot::default()
            }],
        )]);

        assert_eq!(
            station_balance_brief(&provider_balances, "alpha", 18),
            "left $3.50"
        );
    }

    #[test]
    fn routing_provider_balance_brief_preserves_subscription_amount_in_narrow_table() {
        let snapshot = Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            model_overrides: HashMap::new(),
            overrides: HashMap::new(),
            station_overrides: HashMap::new(),
            route_target_overrides: HashMap::new(),
            service_tier_overrides: HashMap::new(),
            global_station_override: None,
            global_route_target_override: None,
            station_meta_overrides: HashMap::new(),
            usage_rollup: crate::state::UsageRollupView::default(),
            provider_balances: HashMap::from([(
                "input".to_string(),
                vec![crate::state::ProviderBalanceSnapshot {
                    provider_id: "input".to_string(),
                    status: BalanceSnapshotStatus::Ok,
                    plan_name: Some("CodeX Pro Annual".to_string()),
                    subscription_balance_usd: Some("165.08".to_string()),
                    ..crate::state::ProviderBalanceSnapshot::default()
                }],
            )]),
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
            refreshed_at: std::time::Instant::now(),
        };

        let brief = routing_provider_balance_brief_lang(
            &snapshot,
            "input",
            usize::from(ROUTING_BALANCE_COLUMN_WIDTH),
            Language::En,
        );

        assert!(brief.contains("$165.08"), "{brief}");
        assert!(!brief.contains('…'), "{brief}");
    }

    #[test]
    fn routing_provider_balance_brief_fits_lazy_quota_in_zh_table_cell() {
        let snapshot = Snapshot {
            rows: Vec::new(),
            recent: Vec::new(),
            model_overrides: HashMap::new(),
            overrides: HashMap::new(),
            station_overrides: HashMap::new(),
            route_target_overrides: HashMap::new(),
            service_tier_overrides: HashMap::new(),
            global_station_override: None,
            global_route_target_override: None,
            station_meta_overrides: HashMap::new(),
            usage_rollup: crate::state::UsageRollupView::default(),
            provider_balances: HashMap::from([(
                "input".to_string(),
                vec![crate::state::ProviderBalanceSnapshot {
                    provider_id: "input".to_string(),
                    status: BalanceSnapshotStatus::Exhausted,
                    exhausted: Some(true),
                    exhaustion_affects_routing: false,
                    quota_period: Some("daily".to_string()),
                    quota_remaining_usd: Some("0".to_string()),
                    quota_limit_usd: Some("300".to_string()),
                    ..crate::state::ProviderBalanceSnapshot::default()
                }],
            )]),
            station_health: HashMap::new(),
            health_checks: HashMap::new(),
            lb_view: HashMap::new(),
            stats_5m: crate::dashboard_core::WindowStats::default(),
            stats_1h: crate::dashboard_core::WindowStats::default(),
            pricing_catalog: crate::pricing::bundled_model_price_catalog_snapshot(),
            refreshed_at: std::time::Instant::now(),
        };

        let brief = routing_provider_balance_brief_lang(
            &snapshot,
            "input",
            usize::from(ROUTING_BALANCE_COLUMN_WIDTH),
            Language::Zh,
        );

        assert!(
            unicode_width::UnicodeWidthStr::width(brief.as_str())
                <= usize::from(ROUTING_BALANCE_COLUMN_WIDTH),
            "{brief}"
        );
        assert_eq!(brief, "不降级 daily $0/$300.00");
        assert!(!brief.ends_with(" / $"), "{brief}");
    }

    #[test]
    fn wrapped_route_order_keeps_provider_names_intact() {
        let mut lines = Vec::new();
        let order = [
            "input",
            "input1",
            "input2",
            "input3",
            "input4",
            "input-light",
            "centos",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();

        push_wrapped_segments(&mut lines, Palette::default(), "order", &order, " > ", 36);

        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("input4"), "{text}");
        assert!(text.contains("input-light"), "{text}");
        assert!(!text.contains('…'), "{text}");
    }

    #[test]
    fn folded_route_order_keeps_selected_provider_visible() {
        let order = [
            "input",
            "input1",
            "input2",
            "input3",
            "input4",
            "input-light",
            "centos",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();

        let folded = folded_route_chain_segments(&order, Some("input-light"), 6);
        let text = folded.join(" > ");

        assert!(text.contains("*input-light"), "{text}");
        assert!(text.contains("input"), "{text}");
        assert!(text.contains("centos"), "{text}");
        assert!(text.contains("... +"), "{text}");
        assert!(!text.contains("inp…ght"), "{text}");
    }

    #[test]
    fn route_graph_routing_render_folds_long_order_and_keeps_target_balance_visible() {
        let order = [
            "input",
            "input1",
            "input2",
            "input3",
            "input4",
            "input-light",
            "centos",
            "超级中转套餐年度输入提供商",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
        let spec = crate::tui::model::RoutingSpecView {
            entry: "main".to_string(),
            routes: BTreeMap::from([(
                "main".to_string(),
                crate::config::RoutingNodeV4 {
                    strategy: crate::config::RoutingPolicyV4::OrderedFailover,
                    children: order.clone(),
                    ..crate::config::RoutingNodeV4::default()
                },
            )]),
            policy: crate::config::RoutingPolicyV4::OrderedFailover,
            order: Vec::new(),
            target: None,
            prefer_tags: Vec::new(),
            chain: Vec::new(),
            pools: BTreeMap::new(),
            on_exhausted: crate::config::RoutingExhaustedActionV4::Continue,
            entry_strategy: crate::config::RoutingPolicyV4::OrderedFailover,
            expanded_order: order.clone(),
            entry_target: None,
            providers: order.iter().map(|name| routing_provider(name)).collect(),
        };
        let snapshot = empty_snapshot(
            HashMap::from([(
                "input-light".to_string(),
                vec![crate::state::ProviderBalanceSnapshot {
                    provider_id: "input-light".to_string(),
                    status: BalanceSnapshotStatus::Exhausted,
                    exhausted: Some(true),
                    exhaustion_affects_routing: false,
                    quota_period: Some("daily".to_string()),
                    quota_remaining_usd: Some("0".to_string()),
                    quota_limit_usd: Some("300".to_string()),
                    ..crate::state::ProviderBalanceSnapshot::default()
                }],
            )]),
            Some("input-light".to_string()),
        );
        let mut ui = UiState {
            page: Page::Stations,
            config_version: Some(5),
            routing_spec: Some(spec),
            selected_station_idx: 5,
            language: Language::Zh,
            ..UiState::default()
        };

        let text = render_stations_text(84, 28, &mut ui, &snapshot);

        assert!(text.contains("provider") && text.contains("#6/8"), "{text}");
        assert!(text.contains("*input-light"), "{text}");
        assert!(text.contains("$0/$300.00"), "{text}");
        assert!(
            text.contains("不") && text.contains("降") && text.contains("级"),
            "{text}"
        );
        assert!(text.contains("超") && text.contains("级"), "{text}");
        assert!(!text.contains("inp…ght"), "{text}");
        assert!(!text.contains("$0/$│"), "{text}");
    }

    #[test]
    fn station_balance_brief_prefers_usable_snapshot_and_keeps_warning() {
        let provider_balances = HashMap::from([(
            "alpha".to_string(),
            vec![
                crate::state::ProviderBalanceSnapshot {
                    status: BalanceSnapshotStatus::Exhausted,
                    ..crate::state::ProviderBalanceSnapshot::default()
                },
                crate::state::ProviderBalanceSnapshot {
                    status: BalanceSnapshotStatus::Ok,
                    total_balance_usd: Some("1.00".to_string()),
                    ..crate::state::ProviderBalanceSnapshot::default()
                },
            ],
        )]);

        assert_eq!(
            station_balance_brief(&provider_balances, "alpha", 18),
            "left $1.00 exh 1"
        );
    }
}
