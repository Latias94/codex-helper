use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};

use crate::proxy::ProxyService;
use crate::tui::i18n;
use crate::tui::model::{ProviderOption, RoutingSpecView, Snapshot, routing_leaf_provider_names};
use crate::tui::state::UiState;

use super::BalanceRefreshSender;
use super::routing::{
    apply_persisted_routing, refresh_route_graph_balances,
    request_provider_balance_refresh_after_control_change, set_provider_billing_tag,
    set_provider_enabled,
};

fn selected_routing_provider_name(ui: &UiState) -> Option<String> {
    ui.selected_routing_menu_provider_row().map(|row| row.name)
}

fn selected_routing_provider_enabled(ui: &UiState) -> Option<bool> {
    ui.selected_routing_menu_provider_row()
        .filter(|row| row.in_catalog)
        .map(|row| row.enabled)
}

pub(super) fn routing_spec_with_order(
    spec: &RoutingSpecView,
    order: Vec<String>,
    policy: crate::config::RoutingPolicyV4,
) -> RoutingSpecView {
    let mut next = spec.clone();
    {
        let node = next.entry_node_mut();
        node.strategy = policy;
        node.children = order;
        if !matches!(policy, crate::config::RoutingPolicyV4::ManualSticky) {
            node.target = None;
        }
        if !matches!(policy, crate::config::RoutingPolicyV4::TagPreferred) {
            node.prefer_tags.clear();
        }
    }
    next.sync_entry_compat_from_graph();
    next
}

pub(super) fn routing_entry_children(spec: &RoutingSpecView) -> Vec<String> {
    let children = spec
        .entry_node()
        .map(|node| node.children.clone())
        .unwrap_or_default();
    if children.is_empty() {
        routing_leaf_provider_names(spec)
    } else {
        children
    }
}

pub(super) fn routing_entry_is_flat_provider_list(spec: &RoutingSpecView) -> bool {
    let provider_names = spec
        .providers
        .iter()
        .map(|provider| provider.name.as_str())
        .collect::<BTreeSet<_>>();
    routing_entry_children(spec)
        .iter()
        .all(|name| provider_names.contains(name.as_str()))
}

pub(super) fn routing_spec_after_provider_enabled_change(
    spec: &RoutingSpecView,
    provider_name: &str,
    enabled: bool,
) -> Option<RoutingSpecView> {
    if enabled
        || !matches!(spec.policy, crate::config::RoutingPolicyV4::ManualSticky)
        || spec.target.as_deref() != Some(provider_name)
    {
        return None;
    }

    Some(routing_spec_with_order(
        spec,
        routing_entry_children(spec),
        crate::config::RoutingPolicyV4::OrderedFailover,
    ))
}

pub(super) async fn handle_key_routing_menu(
    _providers: &mut [ProviderOption],
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    balance_refresh_tx: &BalanceRefreshSender,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Char('r') => {
            ui.overlay = crate::tui::types::Overlay::None;
            true
        }
        KeyCode::Char('g') => {
            refresh_route_graph_balances(ui, snapshot, proxy, balance_refresh_tx).await;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.routing_menu_idx = ui.routing_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = ui
                .routing_provider_count()
                .map(|len| len.saturating_sub(1))
                .unwrap_or(0);
            ui.routing_menu_idx = (ui.routing_menu_idx + 1).min(max);
            true
        }
        KeyCode::Char('[') | KeyCode::Char('u') => {
            let Some(spec) = ui.routing_spec.clone() else {
                return true;
            };
            if !routing_entry_is_flat_provider_list(&spec) {
                ui.toast = Some((
                    i18n::label(
                        ui.language,
                        "nested route graph: edit route nodes in TOML for grouped reorder",
                    )
                    .to_string(),
                    Instant::now(),
                ));
                return true;
            }
            let Some((order, next_idx)) = ui.reordered_routing_provider_order(-1) else {
                return true;
            };
            ui.routing_menu_idx = next_idx;
            let next = routing_spec_with_order(
                &spec,
                order,
                crate::config::RoutingPolicyV4::OrderedFailover,
            );
            match apply_persisted_routing(ui, snapshot, proxy, next, balance_refresh_tx).await {
                Ok(()) => {
                    ui.toast = Some((
                        i18n::label(ui.language, "routing: moved up").to_string(),
                        Instant::now(),
                    ))
                }
                Err(err) => {
                    ui.toast = Some((
                        format!(
                            "{}: {err}",
                            i18n::label(ui.language, "routing: move failed")
                        ),
                        Instant::now(),
                    ))
                }
            }
            true
        }
        KeyCode::Char(']') | KeyCode::Char('d') => {
            let Some(spec) = ui.routing_spec.clone() else {
                return true;
            };
            if !routing_entry_is_flat_provider_list(&spec) {
                ui.toast = Some((
                    i18n::label(
                        ui.language,
                        "nested route graph: edit route nodes in TOML for grouped reorder",
                    )
                    .to_string(),
                    Instant::now(),
                ));
                return true;
            }
            let Some((order, next_idx)) = ui.reordered_routing_provider_order(1) else {
                return true;
            };
            ui.routing_menu_idx = next_idx;
            let next = routing_spec_with_order(
                &spec,
                order,
                crate::config::RoutingPolicyV4::OrderedFailover,
            );
            match apply_persisted_routing(ui, snapshot, proxy, next, balance_refresh_tx).await {
                Ok(()) => {
                    ui.toast = Some((
                        i18n::label(ui.language, "routing: moved down").to_string(),
                        Instant::now(),
                    ))
                }
                Err(err) => {
                    ui.toast = Some((
                        format!(
                            "{}: {err}",
                            i18n::label(ui.language, "routing: move failed")
                        ),
                        Instant::now(),
                    ))
                }
            }
            true
        }
        KeyCode::Enter => {
            let Some(spec) = ui.routing_spec.clone() else {
                return true;
            };
            let Some(target) = selected_routing_provider_name(ui) else {
                return true;
            };
            let mut next = spec.clone();
            {
                let node = next.entry_node_mut();
                node.strategy = crate::config::RoutingPolicyV4::ManualSticky;
                node.target = Some(target.clone());
                node.children = routing_entry_children(&spec);
                if !node.children.iter().any(|name| name == &target) {
                    node.children.insert(0, target.clone());
                }
                node.prefer_tags.clear();
                node.on_exhausted = crate::config::RoutingExhaustedActionV4::Continue;
            }
            next.sync_entry_compat_from_graph();
            match apply_persisted_routing(ui, snapshot, proxy, next, balance_refresh_tx).await {
                Ok(()) => {
                    ui.toast = Some((
                        format!("{} {target}", i18n::label(ui.language, "routing: pinned")),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        format!("{}: {err}", i18n::label(ui.language, "routing: pin failed")),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('a') => {
            let Some(spec) = ui.routing_spec.clone() else {
                return true;
            };
            let order = routing_entry_children(&spec);
            let next = routing_spec_with_order(
                &spec,
                order,
                crate::config::RoutingPolicyV4::OrderedFailover,
            );
            match apply_persisted_routing(ui, snapshot, proxy, next, balance_refresh_tx).await {
                Ok(()) => {
                    ui.toast = Some((
                        i18n::label(ui.language, "routing: ordered-failover").to_string(),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        format!(
                            "{}: {err}",
                            i18n::label(ui.language, "routing: apply failed")
                        ),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('f') => {
            let Some(spec) = ui.routing_spec.clone() else {
                return true;
            };
            let mut next = spec.clone();
            {
                let node = next.entry_node_mut();
                node.strategy = crate::config::RoutingPolicyV4::TagPreferred;
                node.children = routing_entry_children(&spec);
                node.target = None;
                node.prefer_tags = vec![BTreeMap::from([(
                    "billing".to_string(),
                    "monthly".to_string(),
                )])];
                node.on_exhausted = crate::config::RoutingExhaustedActionV4::Continue;
            }
            next.sync_entry_compat_from_graph();
            match apply_persisted_routing(ui, snapshot, proxy, next, balance_refresh_tx).await {
                Ok(()) => {
                    ui.toast = Some((
                        i18n::label(ui.language, "routing: prefer billing=monthly").to_string(),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        format!(
                            "{}: {err}",
                            i18n::label(ui.language, "routing: apply failed")
                        ),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('e') => {
            let Some(provider_name) = selected_routing_provider_name(ui) else {
                return true;
            };
            let Some(enabled) = selected_routing_provider_enabled(ui) else {
                ui.toast = Some((
                    format!(
                        "{} {provider_name}: {}",
                        i18n::label(ui.language, "provider"),
                        i18n::label(ui.language, "not in catalog")
                    ),
                    Instant::now(),
                ));
                return true;
            };
            let next_enabled = !enabled;
            let original_spec = ui.routing_spec.clone();
            match set_provider_enabled(ui, proxy, provider_name.as_str(), next_enabled).await {
                Ok(()) => {
                    let mut suffix = String::new();
                    let mut balance_refresh_requested = false;
                    if let Some(next_routing) = original_spec.as_ref().and_then(|spec| {
                        routing_spec_after_provider_enabled_change(
                            spec,
                            provider_name.as_str(),
                            next_enabled,
                        )
                    }) {
                        match apply_persisted_routing(
                            ui,
                            snapshot,
                            proxy,
                            next_routing,
                            balance_refresh_tx,
                        )
                        .await
                        {
                            Ok(()) => {
                                suffix = "; routing=ordered-failover".to_string();
                                balance_refresh_requested = true;
                            }
                            Err(err) => {
                                suffix = format!(
                                    "; {}: {err}",
                                    i18n::label(ui.language, "routing update failed")
                                );
                            }
                        }
                    }
                    if !balance_refresh_requested {
                        request_provider_balance_refresh_after_control_change(
                            ui,
                            snapshot,
                            proxy,
                            balance_refresh_tx,
                        );
                    }
                    let label = if next_enabled {
                        i18n::label(ui.language, "enabled")
                    } else {
                        i18n::label(ui.language, "disabled")
                    };
                    ui.toast = Some((
                        format!(
                            "{} {provider_name}: {label}{suffix}",
                            i18n::label(ui.language, "provider")
                        ),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        format!(
                            "{}: {err}",
                            i18n::label(ui.language, "provider enable failed")
                        ),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('s') => {
            let Some(spec) = ui.routing_spec.clone() else {
                return true;
            };
            let mut next = spec.clone();
            let on_exhausted = match spec.on_exhausted {
                crate::config::RoutingExhaustedActionV4::Continue => {
                    crate::config::RoutingExhaustedActionV4::Stop
                }
                crate::config::RoutingExhaustedActionV4::Stop => {
                    crate::config::RoutingExhaustedActionV4::Continue
                }
            };
            next.entry_node_mut().on_exhausted = on_exhausted;
            next.sync_entry_compat_from_graph();
            match apply_persisted_routing(ui, snapshot, proxy, next, balance_refresh_tx).await {
                Ok(()) => {
                    let label = match ui.routing_spec.as_ref().map(|spec| spec.on_exhausted) {
                        Some(crate::config::RoutingExhaustedActionV4::Continue) => "continue",
                        Some(crate::config::RoutingExhaustedActionV4::Stop) => "stop",
                        None => "-",
                    };
                    ui.toast = Some((
                        format!(
                            "routing: {}={label}",
                            i18n::label(ui.language, "on_exhausted")
                        ),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        format!(
                            "{}: {err}",
                            i18n::label(ui.language, "routing: apply failed")
                        ),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        KeyCode::Char('1') | KeyCode::Char('2') | KeyCode::Char('0') => {
            let Some(provider_name) = selected_routing_provider_name(ui) else {
                return true;
            };
            let value = match key.code {
                KeyCode::Char('1') => Some("monthly"),
                KeyCode::Char('2') => Some("paygo"),
                KeyCode::Char('0') => None,
                _ => unreachable!(),
            };
            match set_provider_billing_tag(ui, proxy, provider_name.as_str(), value).await {
                Ok(()) => {
                    request_provider_balance_refresh_after_control_change(
                        ui,
                        snapshot,
                        proxy,
                        balance_refresh_tx,
                    );
                    let label = value.unwrap_or_else(|| i18n::label(ui.language, "<clear>"));
                    ui.toast = Some((
                        format!(
                            "{} {provider_name}: billing={label}",
                            i18n::label(ui.language, "provider")
                        ),
                        Instant::now(),
                    ));
                }
                Err(err) => {
                    ui.toast = Some((
                        format!("{}: {err}", i18n::label(ui.language, "provider tag failed")),
                        Instant::now(),
                    ));
                }
            }
            true
        }
        _ => false,
    }
}
