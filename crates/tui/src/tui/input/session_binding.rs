use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::proxy::{OperatorSessionBindingCommand, OperatorSessionBindingMutationRequest};
use crate::tui::i18n;
use crate::tui::operator_actions::queue_session_binding_mutation;
use crate::tui::state::UiState;
use crate::tui::types::{
    Overlay, SessionBindingInputKind, SessionEffortChoice, SessionServiceTierChoice,
};

fn close_editor(ui: &mut UiState) {
    ui.overlay = Overlay::None;
    ui.session_binding_edit = None;
    ui.clear_profile_menu_snapshot();
    ui.session_binding_input.clear();
    ui.session_binding_input_hint = None;
}

fn queue_command(ui: &mut UiState, command: OperatorSessionBindingCommand) -> bool {
    let Some(edit) = ui.session_binding_edit.take() else {
        close_editor(ui);
        return true;
    };
    ui.overlay = Overlay::None;
    ui.clear_profile_menu_snapshot();
    ui.session_binding_input.clear();
    ui.session_binding_input_hint = None;
    let _ = queue_session_binding_mutation(
        ui,
        OperatorSessionBindingMutationRequest {
            session_key: edit.session_key,
            expected_binding_revision: edit.expected_revision,
            command,
        },
    );
    true
}

pub(super) fn handle_profile_menu(ui: &mut UiState, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            close_editor(ui);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.session_profile_menu_idx = ui.session_profile_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.session_profile_menu_idx =
                (ui.session_profile_menu_idx + 1).min(ui.profile_menu_options().len());
            true
        }
        KeyCode::Home => {
            ui.session_profile_menu_idx = 0;
            true
        }
        KeyCode::End => {
            ui.session_profile_menu_idx = ui.profile_menu_options().len();
            true
        }
        KeyCode::Enter => {
            let profile_name = ui
                .session_profile_menu_idx
                .checked_sub(1)
                .and_then(|index| ui.profile_menu_options().get(index))
                .map(|profile| profile.name.clone());
            queue_command(
                ui,
                OperatorSessionBindingCommand::SetProfile { profile_name },
            )
        }
        _ => false,
    }
}

pub(super) fn handle_model_menu(ui: &mut UiState, key: KeyEvent) -> bool {
    let custom_index = ui.session_model_options.len() + 1;
    match key.code {
        KeyCode::Esc => {
            close_editor(ui);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.session_model_menu_idx = ui.session_model_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.session_model_menu_idx = (ui.session_model_menu_idx + 1).min(custom_index);
            true
        }
        KeyCode::Home => {
            ui.session_model_menu_idx = 0;
            true
        }
        KeyCode::End => {
            ui.session_model_menu_idx = custom_index;
            true
        }
        KeyCode::Enter if ui.session_model_menu_idx == custom_index => {
            ui.session_binding_input_kind = SessionBindingInputKind::Model;
            ui.overlay = Overlay::SessionBindingInput;
            true
        }
        KeyCode::Enter => {
            let model = ui
                .session_model_menu_idx
                .checked_sub(1)
                .and_then(|index| ui.session_model_options.get(index))
                .cloned();
            queue_command(ui, OperatorSessionBindingCommand::SetModel { model })
        }
        _ => false,
    }
}

pub(super) fn handle_effort_menu(ui: &mut UiState, key: KeyEvent) -> bool {
    let choices = SessionEffortChoice::ALL;
    match key.code {
        KeyCode::Esc => {
            close_editor(ui);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.session_effort_menu_idx = ui.session_effort_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.session_effort_menu_idx = (ui.session_effort_menu_idx + 1).min(choices.len() - 1);
            true
        }
        KeyCode::Home => {
            ui.session_effort_menu_idx = 0;
            true
        }
        KeyCode::End => {
            ui.session_effort_menu_idx = choices.len() - 1;
            true
        }
        KeyCode::Enter => {
            let reasoning_effort = choices
                .get(ui.session_effort_menu_idx)
                .copied()
                .unwrap_or(SessionEffortChoice::Clear)
                .value()
                .map(ToOwned::to_owned);
            queue_command(
                ui,
                OperatorSessionBindingCommand::SetReasoningEffort { reasoning_effort },
            )
        }
        _ => false,
    }
}

pub(super) fn handle_service_tier_menu(ui: &mut UiState, key: KeyEvent) -> bool {
    let choices = SessionServiceTierChoice::ALL;
    let custom_index = choices.len();
    match key.code {
        KeyCode::Esc => {
            close_editor(ui);
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.session_service_tier_menu_idx = ui.session_service_tier_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.session_service_tier_menu_idx =
                (ui.session_service_tier_menu_idx + 1).min(custom_index);
            true
        }
        KeyCode::Home => {
            ui.session_service_tier_menu_idx = 0;
            true
        }
        KeyCode::End => {
            ui.session_service_tier_menu_idx = custom_index;
            true
        }
        KeyCode::Enter if ui.session_service_tier_menu_idx == custom_index => {
            ui.session_binding_input_kind = SessionBindingInputKind::ServiceTier;
            ui.overlay = Overlay::SessionBindingInput;
            true
        }
        KeyCode::Enter => {
            let service_tier = choices
                .get(ui.session_service_tier_menu_idx)
                .copied()
                .unwrap_or(SessionServiceTierChoice::Clear)
                .value()
                .map(ToOwned::to_owned);
            queue_command(
                ui,
                OperatorSessionBindingCommand::SetServiceTier { service_tier },
            )
        }
        _ => false,
    }
}

pub(super) fn handle_binding_input(ui: &mut UiState, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = match ui.session_binding_input_kind {
                SessionBindingInputKind::Model => Overlay::SessionModelMenu,
                SessionBindingInputKind::ServiceTier => Overlay::SessionServiceTierMenu,
            };
            true
        }
        KeyCode::Enter => {
            let value = ui.session_binding_input.trim();
            let value = (!value.is_empty()).then(|| value.to_string());
            match ui.session_binding_input_kind {
                SessionBindingInputKind::Model => {
                    queue_command(ui, OperatorSessionBindingCommand::SetModel { model: value })
                }
                SessionBindingInputKind::ServiceTier => queue_command(
                    ui,
                    OperatorSessionBindingCommand::SetServiceTier {
                        service_tier: value,
                    },
                ),
            }
        }
        KeyCode::Backspace => {
            ui.session_binding_input.pop();
            true
        }
        KeyCode::Delete => {
            ui.session_binding_input.clear();
            true
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            ui.session_binding_input.clear();
            true
        }
        KeyCode::Char(character)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            if ui.session_binding_input.len() < 512 && !character.is_control() {
                ui.session_binding_input.push(character);
            } else {
                ui.toast = Some((
                    i18n::label(ui.language, "session value is too long").to_string(),
                    Instant::now(),
                ));
            }
            true
        }
        _ => false,
    }
}
