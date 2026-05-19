mod balance;
mod health;
mod history_bridge;
mod normal;
mod profile;
mod routing;
mod routing_menu;
mod session_overrides;
mod transcript;

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::proxy::ProxyService;
use crate::state::ProxyState;

use super::model::{ProviderOption, Snapshot};
use super::state::UiState;
use super::types::Overlay;
pub(in crate::tui) use balance::{
    BalanceRefreshMode, BalanceRefreshOutcome, BalanceRefreshSender,
    request_provider_balance_refresh,
};
use normal::{apply_page_shortcuts, handle_key_normal, handle_key_provider_menu, toggle_language};
use profile::handle_key_profile_menu;
pub(in crate::tui) use profile::refresh_profile_control_state;
use routing_menu::handle_key_routing_menu;
use session_overrides::{
    handle_key_effort_menu, handle_key_model_input, handle_key_model_menu,
    handle_key_service_tier_input, handle_key_service_tier_menu,
};
use transcript::handle_key_session_transcript;

pub(in crate::tui) use routing::refresh_routing_control_state;

#[cfg(test)]
use balance::should_request_provider_balance_refresh;
#[cfg(test)]
use profile::default_profile_menu_idx;
#[cfg(test)]
use routing_menu::{
    routing_entry_children, routing_entry_is_flat_provider_list,
    routing_spec_after_provider_enabled_change, routing_spec_with_order,
};

pub(in crate::tui) fn should_accept_key_event(event: &KeyEvent) -> bool {
    matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

pub(in crate::tui) struct KeyEventContext<'a> {
    pub(in crate::tui) state: &'a Arc<ProxyState>,
    pub(in crate::tui) providers: &'a mut Vec<ProviderOption>,
    pub(in crate::tui) ui: &'a mut UiState,
    pub(in crate::tui) snapshot: &'a Snapshot,
    pub(in crate::tui) proxy: &'a ProxyService,
    pub(in crate::tui) balance_refresh_tx: &'a BalanceRefreshSender,
    pub(in crate::tui) codex_relay_diagnostics_tx:
        &'a crate::tui::codex_relay_diagnostics::CodexRelayDiagnosticsSender,
    pub(in crate::tui) codex_relay_live_smoke_tx:
        &'a crate::tui::codex_relay_live_smoke::CodexRelayLiveSmokeSender,
}

pub(in crate::tui) async fn handle_key_event(ctx: KeyEventContext<'_>, key: KeyEvent) -> bool {
    if ctx.ui.overlay == Overlay::None && apply_page_shortcuts(ctx.ui, key.code) {
        return true;
    }

    match ctx.ui.overlay {
        Overlay::None => handle_key_normal(ctx, key).await,
        Overlay::Help => match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                ctx.ui.overlay = Overlay::None;
                true
            }
            KeyCode::Char('L') => {
                toggle_language(ctx.ui).await;
                true
            }
            _ => false,
        },
        Overlay::SessionTranscript => handle_key_session_transcript(ctx.ui, key).await,
        Overlay::StartupAlert => match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                ctx.ui.startup_readiness = None;
                ctx.ui.overlay = Overlay::None;
                true
            }
            KeyCode::Char('L') => {
                toggle_language(ctx.ui).await;
                true
            }
            _ => false,
        },
        Overlay::StationInfo => match key.code {
            KeyCode::Esc | KeyCode::Char('i') => {
                ctx.ui.overlay = Overlay::None;
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                ctx.ui.station_info_scroll = ctx.ui.station_info_scroll.saturating_sub(1);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                ctx.ui.station_info_scroll = ctx.ui.station_info_scroll.saturating_add(1);
                true
            }
            KeyCode::PageUp => {
                ctx.ui.station_info_scroll = ctx.ui.station_info_scroll.saturating_sub(10);
                true
            }
            KeyCode::PageDown => {
                ctx.ui.station_info_scroll = ctx.ui.station_info_scroll.saturating_add(10);
                true
            }
            KeyCode::Home | KeyCode::Char('g') => {
                ctx.ui.station_info_scroll = 0;
                true
            }
            KeyCode::End | KeyCode::Char('G') => {
                ctx.ui.station_info_scroll = u16::MAX;
                true
            }
            KeyCode::Char('L') => {
                toggle_language(ctx.ui).await;
                true
            }
            _ => false,
        },
        Overlay::EffortMenu => handle_key_effort_menu(ctx.state, ctx.ui, ctx.snapshot, key).await,
        Overlay::ModelMenuSession => {
            handle_key_model_menu(ctx.state, ctx.ui, ctx.snapshot, key).await
        }
        Overlay::ModelInputSession => {
            handle_key_model_input(ctx.state, ctx.ui, ctx.snapshot, key).await
        }
        Overlay::ServiceTierMenuSession => {
            handle_key_service_tier_menu(ctx.state, ctx.ui, ctx.snapshot, key).await
        }
        Overlay::ServiceTierInputSession => {
            handle_key_service_tier_input(ctx.state, ctx.ui, ctx.snapshot, key).await
        }
        Overlay::ProfileMenuSession
        | Overlay::ProfileMenuDefaultRuntime
        | Overlay::ProfileMenuDefaultPersisted => {
            handle_key_profile_menu(ctx.state, ctx.ui, ctx.snapshot, ctx.proxy, key).await
        }
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            handle_key_provider_menu(
                ctx.state,
                ctx.providers,
                ctx.ui,
                ctx.snapshot,
                ctx.proxy,
                key,
            )
            .await
        }
        Overlay::RoutingMenu => {
            handle_key_routing_menu(
                ctx.providers,
                ctx.ui,
                ctx.snapshot,
                ctx.proxy,
                ctx.balance_refresh_tx,
                key,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests;
