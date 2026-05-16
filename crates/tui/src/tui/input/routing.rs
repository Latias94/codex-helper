use std::time::Instant;

use crate::config::{PersistedProviderSpec, PersistedProvidersCatalog};
use crate::proxy::ProxyService;
use crate::routing_ir::RouteRequestContext;
use crate::state::ProxyState;
use crate::tui::Language;
use crate::tui::i18n;
use crate::tui::model::{ProviderOption, RoutingSpecUpsertView, RoutingSpecView, Snapshot, now_ms};
use crate::tui::state::UiState;
use crate::tui::types::{Overlay, Page};

use super::{BalanceRefreshMode, BalanceRefreshSender, request_provider_balance_refresh};

pub(super) async fn apply_global_route_target_pin(
    state: &ProxyState,
    providers: &[ProviderOption],
    target: Option<String>,
) -> anyhow::Result<()> {
    if let Some(name) = target.as_deref() {
        if !providers.iter().any(|provider| provider.name == name) {
            anyhow::bail!("unknown route target: {name}");
        }
        state
            .set_global_route_target_override(name.to_string(), now_ms())
            .await;
    } else {
        state.clear_global_route_target_override().await;
    }
    Ok(())
}

pub(in crate::tui) async fn refresh_routing_control_state(
    ui: &mut UiState,
    proxy: &ProxyService,
) -> anyhow::Result<()> {
    let response = RoutingSpecView::from(proxy.persisted_routing_spec().await?);
    ui.routing_explain = proxy
        .routing_explain(RouteRequestContext::default(), None)
        .await
        .ok();
    ui.routing_spec = Some(response);
    ui.clamp_routing_menu_selection();
    ui.last_routing_control_refresh_at = Some(Instant::now());
    Ok(())
}

pub(super) fn invalidate_route_target_preview(ui: &mut UiState) {
    if !ui.uses_route_graph_routing() {
        return;
    }
    ui.routing_explain = None;
    ui.last_routing_control_refresh_at = None;
    ui.needs_snapshot_refresh = true;
}

pub(super) async fn open_routing_editor(
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    reason: &str,
    balance_refresh_tx: &BalanceRefreshSender,
) {
    let balance_started = request_provider_balance_refresh(
        ui,
        snapshot,
        proxy,
        BalanceRefreshMode::Auto,
        balance_refresh_tx,
    );
    if ui.page == Page::Stations {
        ui.sync_routing_menu_with_station_selection();
    }
    match refresh_routing_control_state(ui, proxy).await {
        Ok(()) => {
            ui.overlay = Overlay::RoutingMenu;
            ui.toast = Some((
                if balance_started {
                    format!(
                        "{reason}; {}",
                        i18n::label(ui.language, "balance refresh started")
                    )
                } else {
                    reason.to_string()
                },
                Instant::now(),
            ));
        }
        Err(err) => {
            ui.toast = Some((
                format!(
                    "{}: {err}",
                    i18n::label(ui.language, "routing: load failed")
                ),
                Instant::now(),
            ));
        }
    }
}

pub(super) fn request_provider_balance_refresh_after_control_change(
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    balance_refresh_tx: &BalanceRefreshSender,
) -> bool {
    request_provider_balance_refresh(
        ui,
        snapshot,
        proxy,
        BalanceRefreshMode::ControlChanged,
        balance_refresh_tx,
    )
}

pub(super) async fn refresh_route_graph_balances(
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    balance_refresh_tx: &BalanceRefreshSender,
) {
    let balance_started = request_provider_balance_refresh(
        ui,
        snapshot,
        proxy,
        BalanceRefreshMode::Force,
        balance_refresh_tx,
    );
    match refresh_routing_control_state(ui, proxy).await {
        Ok(()) => {
            ui.toast = Some((
                if balance_started {
                    match ui.language {
                        Language::Zh => "routing: 已刷新；余额刷新已开始",
                        Language::En => "routing: refreshed; balance refresh started",
                    }
                } else {
                    i18n::label(ui.language, "balance refresh already requested")
                }
                .to_string(),
                Instant::now(),
            ));
        }
        Err(err) => {
            ui.toast = Some((
                format!(
                    "{}: {err}",
                    i18n::label(ui.language, "routing: refresh failed")
                ),
                Instant::now(),
            ));
        }
    }
}

pub(super) async fn apply_persisted_routing(
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    mut routing: RoutingSpecView,
    balance_refresh_tx: &BalanceRefreshSender,
) -> anyhow::Result<()> {
    routing.providers.clear();
    routing.sync_entry_compat_from_graph();
    let payload = RoutingSpecUpsertView::from(&routing);
    let response =
        RoutingSpecView::from(proxy.upsert_persisted_routing_spec(payload.into()).await?);
    ui.routing_spec = Some(response);
    ui.clamp_routing_menu_selection();
    ui.last_routing_control_refresh_at = Some(Instant::now());
    ui.needs_snapshot_refresh = true;
    ui.needs_config_refresh = true;
    request_provider_balance_refresh_after_control_change(ui, snapshot, proxy, balance_refresh_tx);
    Ok(())
}

async fn load_provider_specs(proxy: &ProxyService) -> anyhow::Result<PersistedProvidersCatalog> {
    proxy.persisted_provider_specs().await.map_err(Into::into)
}

async fn apply_provider_spec(
    proxy: &ProxyService,
    provider: PersistedProviderSpec,
) -> anyhow::Result<()> {
    proxy
        .upsert_persisted_provider_spec(provider.name.clone(), provider)
        .await?;
    Ok(())
}

pub(super) async fn set_provider_billing_tag(
    ui: &mut UiState,
    proxy: &ProxyService,
    provider_name: &str,
    billing: Option<&str>,
) -> anyhow::Result<()> {
    let catalog = load_provider_specs(proxy).await?;
    let mut provider = catalog
        .providers
        .into_iter()
        .find(|provider| provider.name == provider_name)
        .ok_or_else(|| anyhow::anyhow!("provider '{provider_name}' not found"))?;
    match billing {
        Some(value) => {
            provider
                .tags
                .insert("billing".to_string(), value.to_string());
        }
        None => {
            provider.tags.remove("billing");
        }
    }
    apply_provider_spec(proxy, provider).await?;
    refresh_routing_control_state(ui, proxy).await?;
    ui.needs_snapshot_refresh = true;
    ui.needs_config_refresh = true;
    Ok(())
}

pub(super) async fn set_provider_enabled(
    ui: &mut UiState,
    proxy: &ProxyService,
    provider_name: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    let catalog = load_provider_specs(proxy).await?;
    let mut provider = catalog
        .providers
        .into_iter()
        .find(|provider| provider.name == provider_name)
        .ok_or_else(|| anyhow::anyhow!("provider '{provider_name}' not found"))?;
    provider.enabled = enabled;
    apply_provider_spec(proxy, provider).await?;
    refresh_routing_control_state(ui, proxy).await?;
    ui.needs_snapshot_refresh = true;
    ui.needs_config_refresh = true;
    Ok(())
}
