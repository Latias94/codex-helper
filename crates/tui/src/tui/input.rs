mod history_bridge;
mod normal;
mod session_binding;
mod transcript;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::proxy::{
    OperatorDefaultProfileMutationRequest, OperatorDefaultProfileScope, OperatorEndpointMode,
    OperatorRoutingCommand, OperatorSessionAffinityCommand, OperatorSessionAffinityMutationRequest,
};

use super::model::{ProviderOption, Snapshot};
use super::operator_actions::{queue_routing_mutation, queue_session_affinity_mutation};
use super::state::UiState;
use super::types::{Overlay, RoutingActionChoice};
pub(in crate::tui) use normal::routing_mutation_request;
use normal::{apply_page_shortcuts, handle_key_normal, toggle_language};
use transcript::handle_key_session_transcript;

pub(in crate::tui) fn should_accept_key_event(event: &KeyEvent) -> bool {
    matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

pub(in crate::tui) fn open_provider_info(ui: &mut UiState) {
    ui.overlay = Overlay::ProviderInfo;
    ui.provider_info_scroll = 0;
}

pub(in crate::tui) fn handle_provider_info_key(ui: &mut UiState, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc | KeyCode::Char('i') => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.provider_info_scroll = ui.provider_info_scroll.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.provider_info_scroll = ui.provider_info_scroll.saturating_add(1);
            true
        }
        KeyCode::PageUp => {
            ui.provider_info_scroll = ui.provider_info_scroll.saturating_sub(10);
            true
        }
        KeyCode::PageDown => {
            ui.provider_info_scroll = ui.provider_info_scroll.saturating_add(10);
            true
        }
        KeyCode::Home | KeyCode::Char('g') => {
            ui.provider_info_scroll = 0;
            true
        }
        KeyCode::End | KeyCode::Char('G') => {
            ui.provider_info_scroll = u16::MAX;
            true
        }
        _ => false,
    }
}

pub(in crate::tui) fn handle_routing_confirmation_key(ui: &mut UiState, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Enter | KeyCode::Char('y') => {
            ui.overlay = Overlay::None;
            if let Some(request) = ui.routing_confirmation.take() {
                let _ = queue_routing_mutation(ui, request);
            }
            true
        }
        KeyCode::Esc | KeyCode::Char('n') => {
            ui.routing_confirmation = None;
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Char('L') => {
            toggle_language(ui);
            true
        }
        _ => false,
    }
}

pub(in crate::tui) fn handle_routing_actions_key(
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    let actions = RoutingActionChoice::ALL;
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.routing_action_selected_idx = ui
                .routing_action_selected_idx
                .checked_sub(1)
                .unwrap_or(actions.len() - 1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.routing_action_selected_idx = (ui.routing_action_selected_idx + 1) % actions.len();
            true
        }
        KeyCode::Home => {
            ui.routing_action_selected_idx = 0;
            true
        }
        KeyCode::End => {
            ui.routing_action_selected_idx = actions.len() - 1;
            true
        }
        KeyCode::Char('L') => {
            toggle_language(ui);
            true
        }
        KeyCode::Enter | KeyCode::Char('y') => {
            let Some(routing) = snapshot.routing.as_ref() else {
                ui.overlay = Overlay::None;
                return true;
            };
            let action = actions
                .get(ui.routing_action_selected_idx)
                .copied()
                .unwrap_or(RoutingActionChoice::PreferNewSessions);
            let command = match action {
                RoutingActionChoice::ClearNewSessionPreference => {
                    if routing.new_session_preference.is_none() {
                        ui.overlay = Overlay::None;
                        ui.toast = Some((
                            match ui.language {
                                super::Language::Zh => "当前已经使用自动路由".to_string(),
                                super::Language::En => {
                                    "automatic routing is already active".to_string()
                                }
                            },
                            std::time::Instant::now(),
                        ));
                        return true;
                    }
                    OperatorRoutingCommand::ClearNewSessionPreference
                }
                action => {
                    let Some(candidate) = ui.selected_routing_candidate(snapshot) else {
                        ui.overlay = Overlay::None;
                        return true;
                    };
                    match action {
                        RoutingActionChoice::PreferNewSessions => {
                            OperatorRoutingCommand::SetNewSessionPreference {
                                provider_id: candidate.provider_id.clone(),
                                endpoint_id: candidate.endpoint_id.clone(),
                            }
                        }
                        RoutingActionChoice::EnableEndpoint => {
                            OperatorRoutingCommand::SetEndpointMode {
                                provider_id: candidate.provider_id.clone(),
                                endpoint_id: candidate.endpoint_id.clone(),
                                mode: OperatorEndpointMode::Enabled,
                            }
                        }
                        RoutingActionChoice::DrainEndpoint => {
                            OperatorRoutingCommand::SetEndpointMode {
                                provider_id: candidate.provider_id.clone(),
                                endpoint_id: candidate.endpoint_id.clone(),
                                mode: OperatorEndpointMode::Draining,
                            }
                        }
                        RoutingActionChoice::DisableEndpoint => {
                            OperatorRoutingCommand::SetEndpointMode {
                                provider_id: candidate.provider_id.clone(),
                                endpoint_id: candidate.endpoint_id.clone(),
                                mode: OperatorEndpointMode::Disabled,
                            }
                        }
                        RoutingActionChoice::ClearNewSessionPreference => {
                            ui.overlay = Overlay::None;
                            return true;
                        }
                    }
                }
            };
            ui.routing_confirmation = Some(routing_mutation_request(routing, command));
            ui.overlay = Overlay::RoutingConfirmation;
            true
        }
        _ => false,
    }
}

pub(in crate::tui) fn handle_session_affinity_confirmation_key(
    ui: &mut UiState,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Enter | KeyCode::Char('y') => {
            ui.overlay = Overlay::None;
            if let Some(request) = ui.session_affinity_confirmation.take() {
                let _ = queue_session_affinity_mutation(ui, request);
            }
            true
        }
        KeyCode::Esc | KeyCode::Char('n') => {
            ui.session_affinity_confirmation = None;
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Char('L') => {
            toggle_language(ui);
            true
        }
        _ => false,
    }
}

pub(in crate::tui) fn handle_session_affinity_actions_key(
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    let action_count = snapshot
        .routing
        .as_ref()
        .map(|routing| {
            if routing.entry_strategy == crate::config::RouteStrategy::Conditional {
                1
            } else {
                routing.candidates.len().saturating_add(1)
            }
        })
        .unwrap_or(1);
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.session_affinity_action_selected_idx = ui
                .session_affinity_action_selected_idx
                .checked_sub(1)
                .unwrap_or(action_count - 1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.session_affinity_action_selected_idx =
                (ui.session_affinity_action_selected_idx + 1) % action_count;
            true
        }
        KeyCode::Home => {
            ui.session_affinity_action_selected_idx = 0;
            true
        }
        KeyCode::End => {
            ui.session_affinity_action_selected_idx = action_count - 1;
            true
        }
        KeyCode::Char('L') => {
            toggle_language(ui);
            true
        }
        KeyCode::Enter | KeyCode::Char('y') => {
            let Some(row) = snapshot.rows.get(ui.selected_session_idx) else {
                ui.overlay = Overlay::None;
                return true;
            };
            let Some(session_key) = row.session_id.as_ref() else {
                ui.overlay = Overlay::None;
                return true;
            };
            let Some(affinity) = row.route_affinity.as_ref() else {
                ui.overlay = Overlay::None;
                return true;
            };
            let Some(routing) = snapshot.routing.as_ref() else {
                ui.overlay = Overlay::None;
                return true;
            };
            let command = if ui.session_affinity_action_selected_idx == 0 {
                OperatorSessionAffinityCommand::Clear
            } else {
                if routing.entry_strategy == crate::config::RouteStrategy::Conditional {
                    ui.overlay = Overlay::None;
                    return true;
                }
                let Some(candidate) = routing
                    .candidates
                    .get(ui.session_affinity_action_selected_idx - 1)
                else {
                    ui.overlay = Overlay::None;
                    return true;
                };
                OperatorSessionAffinityCommand::Rebind {
                    provider_id: candidate.provider_id.clone(),
                    endpoint_id: candidate.endpoint_id.clone(),
                }
            };
            ui.session_affinity_confirmation = Some(OperatorSessionAffinityMutationRequest {
                session_key: session_key.clone(),
                expected_affinity_revision: Some(affinity.revision.clone()),
                command,
            });
            ui.overlay = Overlay::SessionAffinityConfirmation;
            true
        }
        _ => false,
    }
}

pub(in crate::tui) fn handle_default_profile_menu_key(ui: &mut UiState, key: KeyEvent) -> bool {
    let scope = match ui.overlay {
        Overlay::ConfiguredDefaultProfileMenu => OperatorDefaultProfileScope::Configured,
        Overlay::RuntimeDefaultProfileMenu => OperatorDefaultProfileScope::Runtime,
        _ => return false,
    };
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            ui.clear_profile_menu_snapshot();
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.settings_profile_menu_idx = ui.settings_profile_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.settings_profile_menu_idx =
                (ui.settings_profile_menu_idx + 1).min(ui.profile_menu_options().len());
            true
        }
        KeyCode::Home => {
            ui.settings_profile_menu_idx = 0;
            true
        }
        KeyCode::End => {
            ui.settings_profile_menu_idx = ui.profile_menu_options().len();
            true
        }
        KeyCode::Enter => {
            let request = if let Some(snapshot) = ui.profile_menu_snapshot.as_ref() {
                OperatorDefaultProfileMutationRequest {
                    scope,
                    profile_name: ui
                        .settings_profile_menu_idx
                        .checked_sub(1)
                        .and_then(|index| snapshot.options.get(index))
                        .map(|profile| profile.name.clone()),
                    expected_profile_catalog_key: snapshot.catalog_key.clone(),
                    expected_control_revision: snapshot.control_revision,
                    expected_configured_profile: snapshot.configured_default_profile.clone(),
                    expected_runtime_profile: snapshot.runtime_default_profile_override.clone(),
                }
            } else {
                OperatorDefaultProfileMutationRequest {
                    scope,
                    profile_name: ui
                        .settings_profile_menu_idx
                        .checked_sub(1)
                        .and_then(|index| ui.profile_options.get(index))
                        .map(|profile| profile.name.clone()),
                    expected_profile_catalog_key: ui.profile_catalog_key.clone(),
                    expected_control_revision: ui.default_profile_control_revision,
                    expected_configured_profile: ui.configured_default_profile.clone(),
                    expected_runtime_profile: ui.runtime_default_profile_override.clone(),
                }
            };
            ui.overlay = Overlay::None;
            ui.clear_profile_menu_snapshot();
            super::operator_actions::queue_default_profile_mutation(ui, request);
            true
        }
        _ => false,
    }
}

pub(in crate::tui) struct KeyEventContext<'a> {
    pub(in crate::tui) providers: &'a mut Vec<ProviderOption>,
    pub(in crate::tui) ui: &'a mut UiState,
    pub(in crate::tui) snapshot: &'a Snapshot,
}

async fn persist_host_local_language_change_with<F, Fut>(
    ui: &mut UiState,
    previous: super::Language,
    persist: F,
) where
    F: FnOnce(super::Language) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    if ui.language == previous || ui.runtime_connection.is_remote_observer() {
        return;
    }

    let selected = ui.language;
    let result = persist(selected).await;
    ui.toast = Some((
        match result {
            Ok(_) => super::i18n::format_language_saved(ui.language, selected),
            Err(error) => super::i18n::format_language_save_failed(ui.language, selected, &error),
        },
        std::time::Instant::now(),
    ));
}

async fn persist_host_local_language_change(ui: &mut UiState, previous: super::Language) {
    persist_host_local_language_change_with(ui, previous, |selected| async move {
        crate::config::mutate_helper_config(|config| {
            config.ui.language = Some(super::i18n::storage_code(selected).to_string());
            Ok(())
        })
        .await
        .map(|_| ())
    })
    .await;
}

pub(in crate::tui) async fn handle_key_event(ctx: KeyEventContext<'_>, key: KeyEvent) -> bool {
    if ctx.ui.overlay == Overlay::None && apply_page_shortcuts(ctx.ui, key.code) {
        return true;
    }

    let previous_language = ctx.ui.language;
    let handled = match ctx.ui.overlay {
        Overlay::None => {
            handle_key_normal(
                KeyEventContext {
                    providers: &mut *ctx.providers,
                    ui: &mut *ctx.ui,
                    snapshot: ctx.snapshot,
                },
                key,
            )
            .await
        }
        Overlay::Help => match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                ctx.ui.overlay = Overlay::None;
                ctx.ui.help_scroll = 0;
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                ctx.ui.help_scroll = ctx.ui.help_scroll.saturating_sub(1);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                ctx.ui.help_scroll = ctx.ui.help_scroll.saturating_add(1);
                true
            }
            KeyCode::PageUp => {
                ctx.ui.help_scroll = ctx.ui.help_scroll.saturating_sub(8);
                true
            }
            KeyCode::PageDown => {
                ctx.ui.help_scroll = ctx.ui.help_scroll.saturating_add(8);
                true
            }
            KeyCode::Home | KeyCode::Char('g') => {
                ctx.ui.help_scroll = 0;
                true
            }
            KeyCode::End | KeyCode::Char('G') => {
                ctx.ui.help_scroll = u16::MAX;
                true
            }
            KeyCode::Char('L') => {
                toggle_language(ctx.ui);
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
                toggle_language(ctx.ui);
                true
            }
            _ => false,
        },
        Overlay::RoutingActions => handle_routing_actions_key(ctx.ui, ctx.snapshot, key),
        Overlay::RoutingConfirmation => handle_routing_confirmation_key(ctx.ui, key),
        Overlay::SessionAffinityActions => {
            handle_session_affinity_actions_key(ctx.ui, ctx.snapshot, key)
        }
        Overlay::SessionAffinityConfirmation => {
            handle_session_affinity_confirmation_key(ctx.ui, key)
        }
        Overlay::SessionProfileMenu => session_binding::handle_profile_menu(ctx.ui, key),
        Overlay::SessionModelMenu => session_binding::handle_model_menu(ctx.ui, key),
        Overlay::SessionEffortMenu => session_binding::handle_effort_menu(ctx.ui, key),
        Overlay::SessionServiceTierMenu => session_binding::handle_service_tier_menu(ctx.ui, key),
        Overlay::SessionBindingInput => session_binding::handle_binding_input(ctx.ui, key),
        Overlay::ConfiguredDefaultProfileMenu | Overlay::RuntimeDefaultProfileMenu => {
            handle_default_profile_menu_key(ctx.ui, key)
        }
        Overlay::ProviderInfo => {
            if handle_provider_info_key(ctx.ui, key) {
                true
            } else if key.code == KeyCode::Char('L') {
                toggle_language(ctx.ui);
                true
            } else {
                false
            }
        }
    };
    if handled {
        persist_host_local_language_change(ctx.ui, previous_language).await;
    }
    handled
}

#[cfg(test)]
mod tests;
