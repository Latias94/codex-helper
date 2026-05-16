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

pub(in crate::tui) async fn handle_key_event(
    state: Arc<ProxyState>,
    providers: &mut Vec<ProviderOption>,
    ui: &mut UiState,
    snapshot: &Snapshot,
    proxy: &ProxyService,
    balance_refresh_tx: BalanceRefreshSender,
    key: KeyEvent,
) -> bool {
    if ui.overlay == Overlay::None && apply_page_shortcuts(ui, key.code) {
        return true;
    }

    match ui.overlay {
        Overlay::None => {
            handle_key_normal(
                &state,
                providers,
                ui,
                snapshot,
                proxy,
                &balance_refresh_tx,
                key,
            )
            .await
        }
        Overlay::Help => match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                ui.overlay = Overlay::None;
                true
            }
            KeyCode::Char('L') => {
                toggle_language(ui).await;
                true
            }
            _ => false,
        },
        Overlay::SessionTranscript => handle_key_session_transcript(ui, key).await,
        Overlay::StationInfo => match key.code {
            KeyCode::Esc | KeyCode::Char('i') => {
                ui.overlay = Overlay::None;
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                ui.station_info_scroll = ui.station_info_scroll.saturating_sub(1);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                ui.station_info_scroll = ui.station_info_scroll.saturating_add(1);
                true
            }
            KeyCode::PageUp => {
                ui.station_info_scroll = ui.station_info_scroll.saturating_sub(10);
                true
            }
            KeyCode::PageDown => {
                ui.station_info_scroll = ui.station_info_scroll.saturating_add(10);
                true
            }
            KeyCode::Home | KeyCode::Char('g') => {
                ui.station_info_scroll = 0;
                true
            }
            KeyCode::End | KeyCode::Char('G') => {
                ui.station_info_scroll = u16::MAX;
                true
            }
            KeyCode::Char('L') => {
                toggle_language(ui).await;
                true
            }
            _ => false,
        },
        Overlay::EffortMenu => handle_key_effort_menu(&state, ui, snapshot, key).await,
        Overlay::ModelMenuSession => handle_key_model_menu(&state, ui, snapshot, key).await,
        Overlay::ModelInputSession => handle_key_model_input(&state, ui, snapshot, key).await,
        Overlay::ServiceTierMenuSession => {
            handle_key_service_tier_menu(&state, ui, snapshot, key).await
        }
        Overlay::ServiceTierInputSession => {
            handle_key_service_tier_input(&state, ui, snapshot, key).await
        }
        Overlay::ProfileMenuSession
        | Overlay::ProfileMenuDefaultRuntime
        | Overlay::ProfileMenuDefaultPersisted => {
            handle_key_profile_menu(&state, ui, snapshot, proxy, key).await
        }
        Overlay::ProviderMenuSession | Overlay::ProviderMenuGlobal => {
            handle_key_provider_menu(&state, providers, ui, snapshot, proxy, key).await
        }
        Overlay::RoutingMenu => {
            handle_key_routing_menu(providers, ui, snapshot, proxy, &balance_refresh_tx, key).await
        }
    }
}

#[cfg(test)]
mod tests;
