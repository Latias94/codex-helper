use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::storage::load_config;
use crate::dashboard_core::build_model_options_from_mgr;
use crate::state::ProxyState;
use crate::tui::i18n;
use crate::tui::model::{Snapshot, now_ms};
use crate::tui::state::UiState;
use crate::tui::types::{EffortChoice, Overlay, ServiceTierChoice};

pub(super) async fn load_model_options_for_service(
    service_name: &str,
) -> anyhow::Result<Vec<String>> {
    let cfg = load_config().await?;
    let mgr = match service_name {
        "claude" => &cfg.claude,
        _ => &cfg.codex,
    };
    Ok(build_model_options_from_mgr(mgr))
}

pub(super) fn selected_session_model_hint(snapshot: &Snapshot, ui: &UiState) -> Option<String> {
    snapshot.rows.get(ui.selected_session_idx).and_then(|row| {
        row.override_model
            .as_deref()
            .or(row
                .effective_model
                .as_ref()
                .map(|value| value.value.as_str()))
            .or(row.last_model.as_deref())
            .map(ToString::to_string)
    })
}

pub(super) fn add_model_option_if_missing(options: &mut Vec<String>, model: Option<&str>) {
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return;
    };
    if options.iter().all(|existing| existing != model) {
        options.push(model.to_string());
        options.sort();
    }
}

pub(super) fn current_model_override(snapshot: &Snapshot, ui: &UiState) -> Option<String> {
    snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|row| row.override_model.clone())
}

pub(super) fn selected_session_service_tier_hint(
    snapshot: &Snapshot,
    ui: &UiState,
) -> Option<String> {
    snapshot.rows.get(ui.selected_session_idx).and_then(|row| {
        row.override_service_tier
            .as_deref()
            .or(row
                .effective_service_tier
                .as_ref()
                .map(|value| value.value.as_str()))
            .or(row.last_service_tier.as_deref())
            .map(ToString::to_string)
    })
}

pub(super) fn current_service_tier_override(snapshot: &Snapshot, ui: &UiState) -> Option<String> {
    snapshot
        .rows
        .get(ui.selected_session_idx)
        .and_then(|row| row.override_service_tier.clone())
}

pub(super) async fn apply_effort_override(state: &ProxyState, sid: String, effort: Option<String>) {
    let now = now_ms();
    if let Some(eff) = effort {
        state.set_session_effort_override(sid, eff, now).await;
    } else {
        state.clear_session_effort_override(&sid).await;
    }
}

async fn apply_model_override(state: &ProxyState, sid: String, model: Option<String>) {
    let now = now_ms();
    if let Some(model) = model {
        state.set_session_model_override(sid, model, now).await;
    } else {
        state.clear_session_model_override(&sid).await;
    }
}

async fn apply_service_tier_override(
    state: &ProxyState,
    sid: String,
    service_tier: Option<String>,
) {
    let now = now_ms();
    if let Some(service_tier) = service_tier {
        state
            .set_session_service_tier_override(sid, service_tier, now)
            .await;
    } else {
        state.clear_session_service_tier_override(&sid).await;
    }
}

pub(super) async fn handle_key_effort_menu(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.effort_menu_idx = ui.effort_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.effort_menu_idx = (ui.effort_menu_idx + 1).min(4);
            true
        }
        KeyCode::Enter => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|r| r.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let choice = match ui.effort_menu_idx {
                1 => EffortChoice::Low,
                2 => EffortChoice::Medium,
                3 => EffortChoice::High,
                4 => EffortChoice::XHigh,
                _ => EffortChoice::Clear,
            };
            apply_effort_override(state, sid, choice.value().map(|s| s.to_string())).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((
                format!(
                    "{}: {}",
                    i18n::label(ui.language, "effort set"),
                    choice.label(ui.language)
                ),
                Instant::now(),
            ));
            true
        }
        _ => false,
    }
}

pub(super) async fn handle_key_service_tier_menu(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.service_tier_menu_idx = ui.service_tier_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            ui.service_tier_menu_idx = (ui.service_tier_menu_idx + 1).min(4);
            true
        }
        KeyCode::Enter => {
            if ui.service_tier_menu_idx == 4 {
                ui.session_service_tier_input =
                    current_service_tier_override(snapshot, ui).unwrap_or_default();
                ui.session_service_tier_input_hint =
                    selected_session_service_tier_hint(snapshot, ui);
                ui.overlay = Overlay::ServiceTierInputSession;
                return true;
            }

            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let choice = match ui.service_tier_menu_idx {
                1 => ServiceTierChoice::Default,
                2 => ServiceTierChoice::Priority,
                3 => ServiceTierChoice::Flex,
                _ => ServiceTierChoice::Clear,
            };
            apply_service_tier_override(state, sid, choice.value().map(|s| s.to_string())).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((
                format!(
                    "{}: {}",
                    i18n::label(ui.language, "service_tier set"),
                    choice.label(ui.language)
                ),
                Instant::now(),
            ));
            true
        }
        _ => false,
    }
}

pub(super) async fn handle_key_service_tier_input(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::ServiceTierMenuSession;
            true
        }
        KeyCode::Enter => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let value = ui.session_service_tier_input.trim().to_string();
            let tier = if value.is_empty() { None } else { Some(value) };
            apply_service_tier_override(state, sid, tier.clone()).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((
                format!(
                    "{}: {}",
                    i18n::label(ui.language, "service_tier set"),
                    tier.as_deref()
                        .unwrap_or_else(|| i18n::label(ui.language, "<clear>"))
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Backspace => {
            ui.session_service_tier_input.pop();
            true
        }
        KeyCode::Delete => {
            ui.session_service_tier_input.clear();
            true
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            ui.session_service_tier_input.clear();
            true
        }
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            ui.session_service_tier_input.push(ch);
            true
        }
        _ => false,
    }
}

pub(super) async fn handle_key_model_menu(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::None;
            true
        }
        KeyCode::Up | KeyCode::Char('k') => {
            ui.model_menu_idx = ui.model_menu_idx.saturating_sub(1);
            true
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = ui.session_model_options.len() + 1;
            ui.model_menu_idx = (ui.model_menu_idx + 1).min(max);
            true
        }
        KeyCode::Enter => {
            if ui.model_menu_idx == ui.session_model_options.len() + 1 {
                ui.session_model_input = current_model_override(snapshot, ui).unwrap_or_default();
                ui.session_model_input_hint = selected_session_model_hint(snapshot, ui);
                ui.overlay = Overlay::ModelInputSession;
                return true;
            }

            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let model = if ui.model_menu_idx == 0 {
                None
            } else {
                ui.session_model_options.get(ui.model_menu_idx - 1).cloned()
            };
            apply_model_override(state, sid, model.clone()).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((
                format!(
                    "{}: {}",
                    i18n::label(ui.language, "model override"),
                    model
                        .as_deref()
                        .unwrap_or_else(|| i18n::label(ui.language, "<clear>"))
                ),
                Instant::now(),
            ));
            true
        }
        _ => false,
    }
}

pub(super) async fn handle_key_model_input(
    state: &ProxyState,
    ui: &mut UiState,
    snapshot: &Snapshot,
    key: KeyEvent,
) -> bool {
    match key.code {
        KeyCode::Esc => {
            ui.overlay = Overlay::ModelMenuSession;
            true
        }
        KeyCode::Enter => {
            let Some(sid) = snapshot
                .rows
                .get(ui.selected_session_idx)
                .and_then(|row| row.session_id.clone())
            else {
                ui.overlay = Overlay::None;
                return true;
            };
            let value = ui.session_model_input.trim().to_string();
            let model = if value.is_empty() { None } else { Some(value) };
            apply_model_override(state, sid, model.clone()).await;
            ui.overlay = Overlay::None;
            ui.toast = Some((
                format!(
                    "{}: {}",
                    i18n::label(ui.language, "model override"),
                    model
                        .as_deref()
                        .unwrap_or_else(|| i18n::label(ui.language, "<clear>"))
                ),
                Instant::now(),
            ));
            true
        }
        KeyCode::Backspace => {
            ui.session_model_input.pop();
            true
        }
        KeyCode::Delete => {
            ui.session_model_input.clear();
            true
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            ui.session_model_input.clear();
            true
        }
        KeyCode::Char(ch)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            ui.session_model_input.push(ch);
            true
        }
        _ => false,
    }
}
